"""HDR STUDIO shelf tool for Maya with Alembic export and V-Ray sync."""

import json
import math
import os
import re
import tempfile
from pathlib import Path

from maya import OpenMayaUI, cmds, mel
from maya.api import OpenMaya

try:
    from PySide6 import QtCore, QtGui, QtWidgets
    from shiboken6 import wrapInstance
except ImportError:
    from PySide2 import QtCore, QtGui, QtWidgets
    from shiboken2 import wrapInstance


def _flat(matrix):
    # Maya row-major/row-vector -> column-major/column-vector: same flat storage.
    return list(matrix)


def _camera_record(path, width, height, pixel_aspect):
    selection = OpenMaya.MSelectionList()
    selection.add(path)
    dag = selection.getDagPath(0)
    if dag.apiType() != OpenMaya.MFn.kCamera:
        dag.extendToShape()
    fn = OpenMaya.MFnCamera(dag)
    near, far = fn.nearClippingPlane, fn.farClippingPlane
    aspect = width * pixel_aspect / height
    left, right, bottom, top = fn.getRenderingFrustum(aspect)
    if fn.isOrtho():
        # getRenderingFrustum also returns the orthographic render extents.
        values = [2/(right-left),0,0,0, 0,2/(top-bottom),0,0,
                  0,0,-2/(far-near),0,
                  -(right+left)/(right-left),-(top+bottom)/(top-bottom),-(far+near)/(far-near),1]
    else:
        values = [2*near/(right-left),0,0,0, 0,2*near/(top-bottom),0,0,
                  (right+left)/(right-left),(top+bottom)/(top-bottom),-(far+near)/(far-near),-1,
                  0,0,-2*far*near/(far-near),0]
    # Apply Maya's film roll/translate/scale after the render frustum.
    projection = OpenMaya.MMatrix(values) * OpenMaya.MMatrix(fn.postProjectionMatrix())
    return {
        'path': dag.fullPathName().replace('|', '/'),
        'view': _flat(dag.inclusiveMatrixInverse()),
        'projection': _flat(projection),
        'width': width, 'height': height, 'pixel_aspect': pixel_aspect,
        'focal_mm': fn.focalLength,
    }


def export_selected(filename, camera=None):
    """Export selected objects and their descendants, plus the camera."""
    return export(filename, camera=camera, selected=True)


def export(filename, camera=None, roots=None, selected=False):
    """Export geometry roots and camera at current time, with render-gate metadata.

    roots: optional full DAG paths. Default: root assemblies containing meshes.
    camera: transform or camera shape; default: all renderable cameras.
    selected: export selected DAG subtrees only; cannot be combined with roots.
    """
    export_selection = None
    if selected:
        if roots is not None:
            raise ValueError('Use selected=True or roots, not both')
        objects = cmds.ls(selection=True, long=True, objectsOnly=True) or []
        if not objects:
            raise ValueError('Select the model objects to export first')
        dag_objects = cmds.ls(objects, dag=True, long=True) or []
        if not any(cmds.nodeType(o) == 'mesh' and not cmds.getAttr(o+'.intermediateObject')
                   for o in dag_objects):
            raise ValueError('The selection must contain a polygon mesh or a group containing meshes')
        export_selection = dag_objects
        roots = sorted({'|' + o.split('|')[1] for o in dag_objects})
    path = Path(filename).resolve()
    if path.suffix.lower() != '.abc':
        raise ValueError('Output filename must end in .abc')
    if '"' in str(path):
        raise ValueError('Quotes are not supported in the output path')
    if camera:
        cameras = cmds.ls(camera, long=True) or []
        if len(cameras) != 1:
            raise ValueError('Specify one unambiguous camera DAG path')
        if cmds.nodeType(cameras[0]) != 'camera':
            cameras = cmds.listRelatives(cameras[0], shapes=True, type='camera', fullPath=True) or []
    else:
        cameras = [c for c in cmds.ls(type='camera', long=True)
                   if cmds.getAttr(c + '.renderable')]
    if not cameras:
        raise ValueError('No camera; pass camera="renderCam" explicitly')
    roots = list(roots) if roots is not None else sorted({
        '|' + m.split('|')[1] for m in cmds.ls(type='mesh', long=True, noIntermediate=True)
    })
    for c in cameras:
        roots.append('|' + c.split('|')[1])
    if export_selection is not None:
        # Only camera shapes; unrelated siblings beneath their parents are excluded.
        # AbcExport -selection adds ancestors, preserving transforms and paths.
        export_selection.extend(cameras)
    roots = sorted(set(roots))
    roots = [r for r in roots if not any(r.startswith(p + '|') for p in roots if p != r)]
    width = cmds.getAttr('defaultResolution.width')
    height = cmds.getAttr('defaultResolution.height')
    pixel_aspect = cmds.getAttr('defaultResolution.pixelAspect')
    records = [_camera_record(c, width, height, pixel_aspect) for c in cameras]
    seconds = OpenMaya.MTime(cmds.currentTime(query=True), OpenMaya.MTime.uiUnit()).asUnits(OpenMaya.MTime.kSeconds)
    frame = cmds.currentTime(query=True)
    path.parent.mkdir(parents=True, exist_ok=True)
    if not cmds.pluginInfo('AbcExport', query=True, loaded=True):
        cmds.loadPlugin('AbcExport')
    root_args = ' '.join('-root "{}"'.format(r) for r in roots)
    job = '-frameRange {0} {0} -dataFormat ogawa -writeVisibility {1} -file "{2}"'.format(
        frame, root_args, path.as_posix())
    if export_selection is None:
        cmds.AbcExport(j=job)
    else:
        original = OpenMaya.MGlobal.getActiveSelectionList()
        temporary = OpenMaya.MSelectionList()
        for obj in sorted(set(export_selection)):
            temporary.add(obj)
        try:
            OpenMaya.MGlobal.setActiveSelectionList(temporary)
            cmds.AbcExport(j=job + ' -selection')
        finally:
            OpenMaya.MGlobal.setActiveSelectionList(original)
    info = path.stat()
    data = {'version': 1, 'sample_seconds': seconds, 'abc_size': info.st_size,
            'abc_mtime_ns': info.st_mtime_ns, 'cameras': records}
    if not all(math.isfinite(v) for c in records for key in ('view', 'projection') for v in c[key]):
        raise ValueError('Non-finite camera matrices')
    path.with_suffix('.camera.json').write_text(json.dumps(data, indent=2), encoding='utf-8')
    print('HDR STUDIO exported: {} (sample {} seconds)'.format(path, seconds))
    return data


_active_link = None
_DOME = "VRayLightDomeShape"
_SNAPSHOT_SLOTS = "ab"
_LINK_SOURCE = "hdrStudioSyncSource"
_TARGET_FILE = "target.json"
_POLL_INTERVAL_MS = 100
_IPR_REFRESH_INTERVAL_MS = 250
MIN_RELOAD_DELAY_MS = 250
MAX_RELOAD_DELAY_MS = 2000
DEFAULT_RELOAD_DELAY_MS = 500
_reload_delay_ms = DEFAULT_RELOAD_DELAY_MS


def default_sync_path():
    root = Path(os.environ.get("LOCALAPPDATA", ""))
    if not root.is_absolute():
        raise OSError("Windows Local App Data is unavailable.")
    return root / "HDR STUDIO" / "vray-sync" / "hdr_studio_vray.hdr"


def _dome_shapes(nodes):
    shapes = []
    for node in nodes:
        if cmds.nodeType(node) == _DOME:
            shapes.append(node)
        else:
            shapes.extend(cmds.listRelatives(node, shapes=True, fullPath=True, type=_DOME) or [])
    return list(dict.fromkeys(shapes))


def _scene_dome():
    shapes = cmds.ls(type=_DOME, dag=True, long=True, allPaths=True) or []
    if len(shapes) > 1:
        raise ValueError(
            f"This scene has {len(shapes)} V-Ray Dome Lights. "
            "Start Sync requires a scene with at most one Dome Light."
        )
    return shapes[0] if shapes else None


def _handle(node):
    selection = OpenMaya.MSelectionList()
    selection.add(node)
    return OpenMaya.MObjectHandle(selection.getDependNode(0))


def _stamp(info):
    return info.st_ino, info.st_mtime_ns, info.st_size


def _read_target(path, previous=None):
    if path is None:
        return previous, None
    try:
        if _stamp(path.stat()) == previous:
            return previous, None
        with path.open("r", encoding="utf-8") as source:
            stamp = _stamp(os.fstat(source.fileno()))
            data = json.load(source)
    except FileNotFoundError:
        return previous, None
    except PermissionError as error:
        if getattr(error, "winerror", None) in (32, 33):
            return previous, None
        raise
    except ValueError as error:
        raise RuntimeError("Invalid HDR STUDIO target file: " + str(error)) from error
    if not isinstance(data, dict) or "rendered_hdr" not in data:
        raise RuntimeError("Invalid HDR STUDIO target file.")
    target = data["rendered_hdr"]
    if target is None:
        return stamp, None
    if not isinstance(target, str):
        raise RuntimeError("Invalid rendered HDR path.")
    rendered = Path(target)
    if not rendered.is_absolute() or rendered.suffix.lower() != ".hdr":
        raise RuntimeError("Invalid rendered HDR path.")
    return stamp, rendered


def _snapshot_paths(path, dome_id):
    prefix = f"{path.stem}.maya-{os.getpid()}-{dome_id}"
    return tuple(path.with_name(f"{prefix}-{slot}.hdr") for slot in _SNAPSHOT_SLOTS)


def _is_link_path(filename, path, dome_id):
    if not filename:
        return False
    current = Path(filename)
    # Scenes saved with the seven-file link can still reuse their File node.
    return current == path or (
        current.parent == path.parent
        and re.fullmatch(
            rf"{re.escape(path.stem)}\.maya-\d+-{dome_id}-[a-g]\.hdr", current.name
        )
        is not None
    )


def _copy_snapshot(source, destination):
    temporary = None
    try:
        with source.open("rb") as hdr:
            stamp = _stamp(os.fstat(hdr.fileno()))
            data = hdr.read()
        with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as output:
            temporary = Path(output.name)
            output.write(data)
        temporary.replace(destination)
        temporary = None
        return stamp
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _reload_texture(filename):
    if not mel.eval('exists "AEfileTextureReloadCmd"'):
        mel.eval('source "AEfileTemplate.mel";')
    mel.eval(f'AEfileTextureReloadCmd("{filename}");')


def _update_texture(filename, target=None, *, reload=True):
    undo_enabled = cmds.undoInfo(query=True, state=True)
    try:
        if undo_enabled:
            cmds.undoInfo(stateWithoutFlush=False)
        if target is not None:
            cmds.setAttr(filename, target.as_posix(), type="string")
        if reload:
            _reload_texture(filename)
    finally:
        if undo_enabled:
            cmds.undoInfo(stateWithoutFlush=True)


class DomeLink:
    def __init__(self, path, dome, texture, snapshots, current, stamp, defer_reload):
        self.path = path
        self.dome = _handle(dome)
        self.texture = _handle(texture)
        self.snapshots = snapshots
        self.current = current
        self.stamp = stamp
        self.target_file = None
        self.target_stamp = None
        self.rendered_path = None
        self.revision = 0
        self.defer_reload = defer_reload
        self.timer = None
        self.reload_timer = None

    def _linked_filename(self):
        if not all(handle.isAlive() and handle.isValid() for handle in (self.dome, self.texture)):
            return None
        dome = OpenMaya.MFnDagNode(self.dome.object()).fullPathName()
        texture = OpenMaya.MFnDependencyNode(self.texture.object()).name()
        filename = texture + ".fileTextureName"
        if not cmds.isConnected(texture + ".outColor", dome + ".domeTex"):
            return None
        expected = self.rendered_path or self.snapshots[self.current]
        if cmds.getAttr(filename) != expected.as_posix():
            return None
        return filename

    def reload_current(self):
        filename = self._linked_filename()
        if filename is None:
            return False
        _update_texture(filename)
        return True

    def poll(self):
        """Apply a changed snapshot. Return False when the connection ends."""
        filename = self._linked_filename()
        if filename is None:
            return False
        target_stamp, rendered = _read_target(self.target_file, self.target_stamp)
        if rendered is not None:
            self.target_stamp = target_stamp
            if rendered.is_file():
                _update_texture(filename, rendered, reload=not self.defer_reload)
                self.rendered_path = rendered
                self.revision += 1
            else:
                cmds.warning("HDR STUDIO rendered HDR is unavailable: " + rendered.as_posix())
            return True
        if self.rendered_path is not None and target_stamp == self.target_stamp:
            return True
        try:
            stamp = _stamp(self.path.stat())
        except OSError:
            return True
        if stamp != self.stamp or self.rendered_path is not None:
            next_snapshot = (self.current + 1) % len(self.snapshots)
            target = self.snapshots[next_snapshot]
            try:
                stamp = _copy_snapshot(self.path, target)
            except FileNotFoundError:
                return True
            except PermissionError as error:
                if getattr(error, "winerror", None) in (32, 33):
                    return True
                raise
            _update_texture(filename, target, reload=not self.defer_reload)
            self.current = next_snapshot
            self.stamp = stamp
            self.rendered_path = None
            self.revision += 1
        self.target_stamp = target_stamp
        return True


def attach(path, dome=None, *, defer_reload=False, confirm_existing=False, target_file=None):
    """Attach an HDR to the scene's only dome, creating one in an empty scene."""
    path = Path(path).expanduser().resolve()
    if path.suffix.lower() != ".hdr" or not path.is_file():
        raise ValueError("Choose the .hdr file created by HDR STUDIO's V-Ray link.")
    target_stamp, rendered = _read_target(target_file)
    if rendered is not None and not rendered.is_file():
        cmds.warning("HDR STUDIO rendered HDR is unavailable: " + rendered.as_posix())
        rendered = None
    if not cmds.pluginInfo("vrayformaya", query=True, loaded=True):
        cmds.loadPlugin("vrayformaya", quiet=True)
    shape = _scene_dome()
    if dome is not None and (shape is None or _dome_shapes(cmds.ls(dome, long=True) or []) != [shape]):
        raise ValueError("The specified transform or shape is not the scene's V-Ray Dome Light.")
    if confirm_existing and shape is not None:
        existing = _handle(shape)
        answer = cmds.confirmDialog(
            title="HDR STUDIO — Use Existing Dome Light?",
            message=f"Use the existing V-Ray Dome Light?\n\n{shape}\n\n"
                    "Sync will replace its Dome Tex connection.",
            button=["Use Existing", "Cancel"], defaultButton="Cancel",
            cancelButton="Cancel", dismissString="Cancel",
        )
        if answer != "Use Existing":
            return None
        if not existing.isAlive() or not existing.isValid() or _scene_dome() != shape:
            raise ValueError("The scene's Dome Light changed. Start Sync again to confirm the target.")

    selection = OpenMaya.MGlobal.getActiveSelectionList()
    created_transform = None
    cmds.undoInfo(openChunk=True, chunkName="Link HDR STUDIO V-Ray Dome")
    try:
        if shape is None:
            created_transform = cmds.createNode("transform", name="HDRStudioDome", skipSelect=True)
            shape = cmds.createNode(_DOME, name="HDRStudioDomeShape",
                                    parent=created_transform, skipSelect=True)
        slot = shape + ".domeTex"
        dome_id, = cmds.ls(shape, uuid=True)
        snapshots = _snapshot_paths(path, dome_id)
        switches = [shape + "." + attr for attr in ("useDomeTex", "domeSpherical")
                    if not cmds.getAttr(shape + "." + attr)]
        for switch in switches:
            if not cmds.getAttr(switch, settable=True):
                raise RuntimeError("Unlock " + switch + " before linking.")
        source = cmds.connectionInfo(slot, sourceFromDestination=True)
        texture = source.rsplit(".", 1)[0]
        current_path = (cmds.getAttr(texture + ".fileTextureName")
                        if source.endswith(".outColor") and cmds.nodeType(texture) == "file"
                        else None)
        source_attr = texture + "." + _LINK_SOURCE
        same_file = _is_link_path(current_path, path, dome_id) or (
            current_path is not None and cmds.objExists(source_attr)
            and cmds.getAttr(source_attr) == path.as_posix()
        )
        previous = Path(current_path) if current_path else None
        current = (snapshots.index(previous) + 1) % len(snapshots) if previous in snapshots else 0
        target = (rendered or snapshots[current]).as_posix()
        if not same_file:
            if cmds.getAttr(slot, lock=True):
                raise RuntimeError("Unlock " + slot + " before linking.")
        stamp = _stamp(path.stat()) if rendered is not None else _copy_snapshot(path, snapshots[current])
        if not same_file:
            texture = cmds.shadingNode("file", asTexture=True, isColorManaged=True,
                                       name="HDRStudioDomeFile")
            cmds.setAttr(texture + ".ignoreColorSpaceFileRules", True)
            cmds.setAttr(texture + ".fileTextureName", target, type="string")
            spaces = cmds.colorManagementPrefs(query=True, inputSpaceNames=True) or []
            for space in ("scene-linear Rec.709-sRGB", "Utility - Linear - sRGB",
                          "Linear Rec.709 (sRGB)", "Raw"):
                if space in spaces:
                    cmds.setAttr(texture + ".colorSpace", space, type="string")
                    break

            # V-Ray creates its spherical VRayPlaceEnvTex on this connection.
            cmds.connectAttr(texture + ".outColor", slot, force=True)
        else:
            cmds.setAttr(texture + ".fileTextureName", target, type="string")
        source_attr = texture + "." + _LINK_SOURCE
        if not cmds.objExists(source_attr):
            cmds.addAttr(texture, longName=_LINK_SOURCE, dataType="string")
        cmds.setAttr(source_attr, path.as_posix(), type="string")
        for switch in switches:
            cmds.setAttr(switch, True)
        if not defer_reload:
            _reload_texture(texture + ".fileTextureName")
        link = DomeLink(path, shape, texture, snapshots, current, stamp, defer_reload)
        link.target_file = target_file
        link.target_stamp = target_stamp
        link.rendered_path = rendered
        return link
    except (OSError, RuntimeError, ValueError):
        if created_transform is not None:
            cmds.delete(created_transform)
        raise
    finally:
        cmds.undoInfo(closeChunk=True)
        OpenMaya.MGlobal.setActiveSelectionList(selection)


def start(path=None, dome=None):
    """Start the link in Maya, asking before reusing an existing Dome Light."""
    global _active_link
    if cmds.about(batch=True):
        raise RuntimeError("Use attach() and poll() in batch Maya.")
    target_file = None
    if path is None:
        path = default_sync_path()
        if not path.is_file():
            raise ValueError("Start Link Maya V-Ray in HDR STUDIO and wait for its first save.")
        target_file = path.with_name(_TARGET_FILE)

    link = attach(path, dome, defer_reload=True, confirm_existing=True, target_file=target_file)
    if link is None:
        return None
    stop()
    _active_link = link
    link.timer = QtCore.QTimer()
    ipr_refresh = QtCore.QTimer(link.timer)
    ipr_refresh.setSingleShot(True)
    ipr_refresh.setInterval(_IPR_REFRESH_INTERVAL_MS)
    link.reload_timer = QtCore.QTimer(link.timer)
    link.reload_timer.setSingleShot(True)
    link.reload_timer.setInterval(_reload_delay_ms)

    def refresh_ipr():
        if _active_link is link:
            try:
                # V-Ray's registered refresh procedure only runs when IPR is active.
                mel.eval("vrayRefreshIprImage();")
            except RuntimeError as error:
                cmds.warning("HDR STUDIO could not refresh V-Ray IPR: " + str(error))

    def delayed_reload():
        if _active_link is not link:
            return
        try:
            if not link.reload_current():
                stop()
                return
        except RuntimeError as error:
            cmds.warning("HDR STUDIO could not reload the HDR: " + str(error))
            return
        ipr_refresh.stop()
        refresh_ipr()

    def on_update():
        try:
            previous_revision = link.revision
            if not link.poll():
                stop()
            elif link.revision != previous_revision:
                if not ipr_refresh.isActive():
                    ipr_refresh.start()
                link.reload_timer.start()
        except (OSError, RuntimeError) as error:
            stop()
            cmds.warning("HDR STUDIO V-Ray link stopped: " + str(error))

    link.timer.timeout.connect(on_update)
    link.timer.start(_POLL_INTERVAL_MS)
    ipr_refresh.timeout.connect(refresh_ipr)
    ipr_refresh.start()
    link.reload_timer.timeout.connect(delayed_reload)
    link.reload_timer.start()
    print("HDR STUDIO V-Ray link: " + link.path.as_posix())
    return link


def reload_delay_ms():
    return _reload_delay_ms


def set_reload_delay_ms(delay_ms):
    global _reload_delay_ms
    _reload_delay_ms = max(MIN_RELOAD_DELAY_MS, min(MAX_RELOAD_DELAY_MS, int(delay_ms)))
    if _active_link is not None:
        # Qt restarts a running timer when its interval changes.
        _active_link.reload_timer.setInterval(_reload_delay_ms)


def active_path():
    """Return the path being watched, or None if synchronization has stopped."""
    return _active_link.path if _active_link is not None else None


def stop():
    """Stop automatic reloads. The dome keeps the last published HDR path."""
    global _active_link
    if _active_link is not None:
        _active_link.timer.stop()
        _active_link.timer.deleteLater()
        _active_link = None


_SHELF = "HDRStudio"
_BUTTON_TAG = "HDRStudio/VrayLink"
_ICON = Path(__file__).resolve().with_name("hdr_studio_maya.png")
_dialog = None


class VrayPanel(QtWidgets.QWidget):
    def __init__(self, parent):
        super().__init__(parent)
        self.path = QtWidgets.QLineEdit()
        self.path.setReadOnly(True)
        path_label = QtWidgets.QLabel("Automatic sync HDR")
        path_label.setBuddy(self.path)
        path_row = QtWidgets.QHBoxLayout()
        path_row.addWidget(path_label)
        path_row.addWidget(self.path, 1)

        self.reload_delay = QtWidgets.QSlider(QtCore.Qt.Horizontal)
        self.reload_delay.setRange(
            MIN_RELOAD_DELAY_MS, MAX_RELOAD_DELAY_MS
        )
        self.reload_delay.setSingleStep(50)
        self.reload_delay.setPageStep(250)
        self.reload_delay.setValue(reload_delay_ms())
        delay_label = QtWidgets.QLabel("Reload delay")
        delay_label.setBuddy(self.reload_delay)
        self.delay_value = QtWidgets.QLabel(f"{self.reload_delay.value()} ms")
        self.delay_value.setMinimumWidth(
            self.delay_value.fontMetrics().horizontalAdvance(f"{self.reload_delay.maximum()} ms")
        )
        delay_row = QtWidgets.QHBoxLayout()
        delay_row.addWidget(delay_label)
        delay_row.addWidget(self.reload_delay, 1)
        delay_row.addWidget(self.delay_value)

        target = QtWidgets.QLabel(
            "Start Sync asks before using the scene's existing V-Ray Dome Light. "
            "If the scene has none, it creates one."
        )
        target.setWordWrap(True)
        self.start_button = QtWidgets.QPushButton("Start Sync")
        self.stop_button = QtWidgets.QPushButton("Stop Sync")
        self.status = QtWidgets.QLabel()
        action_row = QtWidgets.QHBoxLayout()
        action_row.addWidget(self.status, 1)
        action_row.addWidget(self.start_button)
        action_row.addWidget(self.stop_button)

        layout = QtWidgets.QVBoxLayout(self)
        layout.addLayout(path_row)
        layout.addLayout(delay_row)
        layout.addWidget(target)
        layout.addLayout(action_row)

        self.reload_delay.valueChanged.connect(self.set_reload_delay)
        self.start_button.clicked.connect(self.start_link)
        self.stop_button.clicked.connect(self.stop_link)

        self.refresh_timer = QtCore.QTimer(self)
        self.refresh_timer.setInterval(250)
        self.refresh_timer.timeout.connect(self.refresh)
        self.refresh()

    def set_reload_delay(self, delay_ms):
        self.delay_value.setText(f"{delay_ms} ms")
        set_reload_delay_ms(delay_ms)

    def showEvent(self, event):
        super().showEvent(event)
        self.refresh()
        self.refresh_timer.start()

    def hideEvent(self, event):
        self.refresh_timer.stop()
        super().hideEvent(event)

    def refresh(self):
        watching = active_path()
        active = watching is not None
        try:
            source = watching or default_sync_path()
            if self.path.text() != source.as_posix():
                self.path.setText(source.as_posix())
            self.start_button.setEnabled(not active)
            if active and _active_link.rendered_path is not None:
                self.status.setText("Rendered HDR in use")
            else:
                self.status.setText("Watching HDR file" if active else "Sync stopped")
        except OSError as error:
            self.path.clear()
            self.start_button.setEnabled(False)
            self.status.setText(str(error))
        self.stop_button.setEnabled(active)

    def start_link(self):
        try:
            start()
        except (OSError, RuntimeError, ValueError) as error:
            QtWidgets.QMessageBox.warning(self, "HDR STUDIO — V-Ray Link", str(error))
        self.refresh()

    def stop_link(self):
        stop()
        self.refresh()


class AlembicPanel(QtWidgets.QWidget):
    def __init__(self, parent):
        super().__init__(parent)
        self.export_path = "lighting.abc"
        self.scope = QtWidgets.QComboBox()
        self.scope.addItem("Selected objects", True)
        self.scope.addItem("Scene meshes", False)
        self.camera = QtWidgets.QComboBox()
        self.camera.setSizeAdjustPolicy(QtWidgets.QComboBox.AdjustToMinimumContentsLengthWithIcon)
        self.camera.setMinimumContentsLength(18)
        refresh_button = QtWidgets.QPushButton("Refresh")
        refresh_button.clicked.connect(self.refresh_cameras)
        camera_row = QtWidgets.QHBoxLayout()
        camera_row.addWidget(self.camera, 1)
        camera_row.addWidget(refresh_button)

        form = QtWidgets.QFormLayout()
        form.addRow("Scope", self.scope)
        form.addRow("Camera", camera_row)
        self.export_button = QtWidgets.QPushButton("Export Alembic…")
        self.export_button.clicked.connect(self.choose_export_path)
        self.status = QtWidgets.QLabel()
        self.status.setWordWrap(True)
        layout = QtWidgets.QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(self.export_button, 0, QtCore.Qt.AlignRight)
        layout.addWidget(self.status)
        layout.addStretch(1)

    def showEvent(self, event):
        super().showEvent(event)
        self.refresh_cameras()

    def refresh_cameras(self):
        selected = self.camera.currentData()
        self.camera.clear()
        self.camera.addItem("Renderable cameras", None)
        for camera in sorted(cmds.ls(type="camera", long=True) or []):
            self.camera.addItem(camera, camera)
        self.camera.setCurrentIndex(max(0, self.camera.findData(selected)))

    def choose_export_path(self):
        dialog = QtWidgets.QFileDialog(self, "Export Alembic")
        dialog.setAcceptMode(QtWidgets.QFileDialog.AcceptSave)
        dialog.setFileMode(QtWidgets.QFileDialog.AnyFile)
        dialog.setNameFilter("Alembic (*.abc)")
        dialog.setDefaultSuffix("abc")
        dialog.selectFile(self.export_path)
        if not dialog.exec_():
            return

        path = dialog.selectedFiles()[0]
        try:
            if self.scope.currentData():
                export_selected(path, camera=self.camera.currentData())
            else:
                export(path, camera=self.camera.currentData())
        except (OSError, RuntimeError, ValueError) as error:
            self.status.clear()
            QtWidgets.QMessageBox.warning(self, "HDR STUDIO — Alembic Export", str(error))
            return
        self.export_path = path
        abc = Path(path)
        self.status.setText(f"Saved {abc.name} and {abc.with_suffix('.camera.json').name}")


class StudioDialog(QtWidgets.QDialog):
    def __init__(self, parent):
        super().__init__(parent)
        self.setObjectName("HDRStudioMayaDialog")
        self.setWindowTitle("HDR STUDIO for Maya")
        self.setMinimumWidth(480)
        if _ICON.is_file():
            self.setWindowIcon(QtGui.QIcon(_ICON.as_posix()))
        self.tabs = QtWidgets.QTabWidget()
        self.vray = VrayPanel(self.tabs)
        self.alembic = AlembicPanel(self.tabs)
        self.tabs.addTab(self.vray, "V-Ray Sync")
        self.tabs.addTab(self.alembic, "Alembic Export")
        layout = QtWidgets.QVBoxLayout(self)
        layout.addWidget(self.tabs)


def show():
    """Show one modeless dialog parented to the Maya main window."""
    global _dialog
    window = OpenMayaUI.MQtUtil.mainWindow()
    if not window:
        raise RuntimeError("Open the HDR STUDIO dialog in interactive Maya.")
    if _dialog is None:
        parent = wrapInstance(int(window), QtWidgets.QWidget)
        _dialog = StudioDialog(parent)
    _dialog.showNormal()
    _dialog.raise_()
    _dialog.activateWindow()
    return _dialog


def install_shelf():
    """Add or update the HDR STUDIO button on the HDRStudio shelf."""
    top = mel.eval("$hdrStudioShelfTop = $gShelfTopLevel")
    if not top:
        raise RuntimeError("Install the HDR STUDIO shelf in interactive Maya.")

    shelf = top + "|" + _SHELF
    if not cmds.shelfLayout(shelf, exists=True):
        mel.eval('addNewShelfTab "HDRStudio"')

    scripts = Path(__file__).resolve().parent.as_posix()
    command = (
        "import sys\n"
        f"_hdr_studio_scripts = {scripts!r}\n"
        "if _hdr_studio_scripts not in sys.path:\n"
        "    sys.path.insert(0, _hdr_studio_scripts)\n"
        "import maya_hdr_studio\n"
        "maya_hdr_studio.show()"
    )
    options = dict(label="HDR STUDIO", annotation="Open HDR STUDIO for Maya",
                   image1=_ICON.as_posix() if _ICON.is_file() else "pythonFamily.png",
                   imageOverlayLabel="", useAlpha=True,
                   docTag=_BUTTON_TAG, sourceType="python", command=command)
    for child in cmds.shelfLayout(shelf, query=True, childArray=True) or []:
        control = shelf + "|" + child
        if (cmds.shelfButton(control, exists=True)
                and cmds.shelfButton(control, query=True, docTag=True) == _BUTTON_TAG):
            cmds.shelfButton(control, edit=True, **options)
            break
    else:
        cmds.shelfButton(parent=shelf, **options)

    cmds.shelfTabLayout(top, edit=True, selectTab=_SHELF)
    path = Path(cmds.internalVar(userPrefDir=True)) / "shelves" / ("shelf_" + _SHELF)
    if not cmds.saveShelf(shelf, path.as_posix()):
        raise RuntimeError("Could not save the HDRStudio shelf.")
