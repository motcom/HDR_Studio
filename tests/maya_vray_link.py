"""Exercise the dome connection and file reload in an isolated Maya process."""

import os
import sys
import json
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "tests" / "output" / "maya-vray" / Path(sys.executable).parent.parent.name
OUT.mkdir(parents=True, exist_ok=True)
os.environ["MAYA_APP_DIR"] = str(OUT / "maya-vray-profile")
os.environ["MAYA_SKIP_USERSETUP_PY"] = "1"

import maya.standalone

maya.standalone.initialize()

from maya import cmds, mel

sys.path.insert(0, str(ROOT / "scripts"))
import maya_hdr_studio
from maya_hdr_studio import attach, reload_delay_ms, set_reload_delay_ms, start, stop


def publish(path, rgbe):
    temporary = path.with_suffix(".tmp")
    temporary.write_bytes(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n" + rgbe * 8)
    temporary.replace(path)


try:
    path = OUT / "maya-linked.hdr"
    publish(path, bytes((128, 0, 0, 130)))
    cmds.file(new=True, force=True)
    cmds.select(clear=True)
    with patch.object(maya_hdr_studio, "_copy_snapshot", side_effect=OSError("copy failed")):
        try:
            attach(path)
        except OSError as error:
            assert str(error) == "copy failed"
        else:
            raise AssertionError("Expected the failed copy to prevent linking")
    assert not cmds.ls(type="VRayLightDomeShape")
    link = attach(path)
    dome, = cmds.ls(type="VRayLightDomeShape", long=True)
    texture, = cmds.listConnections(dome + ".domeTex", source=True, destination=False)
    placement, = cmds.listConnections(texture + ".uv", source=True, destination=False,
                                      type="VRayPlaceEnvTex")
    assert cmds.getAttr(placement + ".mappingType") == 2
    assert cmds.getAttr(dome + ".useDomeTex") and cmds.getAttr(dome + ".domeSpherical")
    filename = texture + ".fileTextureName"
    first_snapshot = Path(cmds.getAttr(filename))
    assert first_snapshot != path and first_snapshot.read_bytes() == path.read_bytes()
    assert first_snapshot.stem.endswith("-a")
    assert cmds.getAttr(texture + ".ignoreColorSpaceFileRules")

    other, = cmds.ls(cmds.group(empty=True, name="UnrelatedSelection"), long=True)
    cmds.setAttr(dome + ".intensity", 2.5)
    cmds.select(other)
    link = attach(path)
    assert len(cmds.ls(type="VRayLightDomeShape")) == 1
    assert cmds.listConnections(dome + ".domeTex", source=True, destination=False) == [texture]
    assert cmds.getAttr(dome + ".intensity") == 2.5
    assert cmds.ls(selection=True, long=True) == [other]
    second_snapshot = Path(cmds.getAttr(filename))
    assert second_snapshot != first_snapshot and second_snapshot.read_bytes() == path.read_bytes()
    assert second_snapshot.stem.endswith("-b")

    stamp = link.stamp
    assert link.poll() and link.stamp == stamp
    publish(path, bytes((0, 128, 0, 130)))
    cmds.undoInfo(stateWithoutFlush=True)
    undo_name = cmds.undoInfo(query=True, undoName=True)
    with patch.object(mel, "eval", wraps=mel.eval) as evaluate:
        assert link.poll() and link.stamp != stamp
        evaluate.assert_any_call(f'AEfileTextureReloadCmd("{filename}");')
    assert cmds.undoInfo(query=True, undoName=True) == undo_name
    snapshot = Path(cmds.getAttr(filename))
    assert snapshot == first_snapshot and snapshot.read_bytes() == path.read_bytes()
    assert second_snapshot.read_bytes() != snapshot.read_bytes()
    for value, expected in enumerate((second_snapshot, first_snapshot, second_snapshot), start=64):
        publish(path, bytes((value, 128, 0, 130)))
        assert link.poll()
        snapshot = Path(cmds.getAttr(filename))
        assert snapshot == expected
        assert snapshot.read_bytes() == path.read_bytes()

    old_snapshot = first_snapshot.with_name(first_snapshot.stem[:-1] + "g.hdr")
    old_snapshot.write_bytes(snapshot.read_bytes())
    cmds.setAttr(filename, old_snapshot.as_posix(), type="string")
    link = attach(path)
    snapshot = Path(cmds.getAttr(filename))
    assert snapshot == first_snapshot and snapshot.read_bytes() == path.read_bytes()
    assert cmds.listConnections(dome + ".domeTex", source=True, destination=False) == [texture]

    previous_link = link
    new_path = OUT / "maya-relinked.hdr"
    publish(new_path, bytes((0, 0, 128, 130)))
    link = attach(new_path)
    new_texture, = cmds.listConnections(dome + ".domeTex", source=True, destination=False)
    assert new_texture != texture
    assert Path(cmds.getAttr(filename)) == snapshot
    new_snapshot = Path(cmds.getAttr(new_texture + ".fileTextureName"))
    assert new_snapshot != new_path and new_snapshot.read_bytes() == new_path.read_bytes()
    assert cmds.ls(selection=True, long=True) == [other]
    assert not previous_link.poll()

    with patch.object(cmds, "about", return_value=False):
        with patch.object(cmds, "confirmDialog", return_value="Cancel") as confirm:
            assert start(new_path) is None
            assert dome in confirm.call_args.kwargs["message"]
        assert cmds.getAttr(new_texture + ".fileTextureName") == new_snapshot.as_posix()
        second_transform = cmds.createNode("transform", name="OtherDome", skipSelect=True)
        cmds.createNode("VRayLightDomeShape", parent=second_transform, skipSelect=True)
        with patch.object(cmds, "confirmDialog") as confirm:
            try:
                start(new_path)
            except ValueError as error:
                assert "2 V-Ray Dome Lights" in str(error)
            else:
                raise AssertionError("Expected Start Sync to block multiple Dome Lights")
            confirm.assert_not_called()
        assert len(cmds.ls(type="VRayLightDomeShape")) == 2
        assert cmds.getAttr(new_texture + ".fileTextureName") == new_snapshot.as_posix()
        cmds.delete(second_transform)

    # The renderer's refresh procedure also works with no IPR session running.
    assert not mel.eval("vrend -q;")
    mel.eval("vrayRefreshIprImage();")

    try:
        from PySide6 import QtCore
    except ImportError:
        from PySide2 import QtCore

    app = QtCore.QCoreApplication.instance() or QtCore.QCoreApplication([])
    assert reload_delay_ms() == 500
    set_reload_delay_ms(2000)
    event_loop = QtCore.QEventLoop()
    evaluate_mel = mel.eval
    refreshed_snapshots = []
    reloaded_snapshots = []
    reload_command = f'AEfileTextureReloadCmd("{new_texture}.fileTextureName");'
    edit_timer = QtCore.QTimer()
    edits = range(64, 76)
    edits_left = iter(edits)
    final_hdr = bytes((edits[-1], 128, 128, 130)) * 8

    def edit():
        value = next(edits_left)
        publish(new_path, bytes((value, 128, 128, 130)))
        if value == edits[-1]:
            edit_timer.stop()

    def observe_refresh(command):
        result = evaluate_mel(command)
        if command in (reload_command, "vrayRefreshIprImage();"):
            refreshed = Path(cmds.getAttr(new_texture + ".fileTextureName"))
            contents = refreshed.read_bytes()
            if command == reload_command:
                reloaded_snapshots.append((refreshed, contents))
            else:
                refreshed_snapshots.append(contents)
                if contents.endswith(final_hdr):
                    if reload_delay_ms() == 2000:
                        assert live_link.reload_timer.isActive()
                        set_reload_delay_ms(250)
                        assert live_link.reload_timer.interval() == 250
                    if reloaded_snapshots:
                        event_loop.quit()
        return result

    with patch.object(cmds, "about", return_value=False), \
            patch.object(cmds, "confirmDialog", return_value="Use Existing") as confirm:
        with patch.object(mel, "eval", wraps=mel.eval) as evaluate:
            live_link = start(new_path)
            confirm.assert_called_once()
            assert cmds.ls(type="VRayLightDomeShape", long=True) == [dome]
            assert all(call.args != (reload_command,) for call in evaluate.call_args_list)
    assert live_link.reload_timer.interval() == 2000
    try:
        with patch.object(mel, "eval", side_effect=observe_refresh):
            edit_timer.timeout.connect(edit)
            edit_timer.start(70)
            QtCore.QTimer.singleShot(3000, event_loop.quit)
            event_loop.exec_()
        assert len(refreshed_snapshots) >= 2
        final_contents = new_path.read_bytes()
        assert refreshed_snapshots[0] != final_contents
        assert refreshed_snapshots[-1] == final_contents
        assert reload_delay_ms() == 250
        assert reloaded_snapshots == [(live_link.snapshots[live_link.current], final_contents)]
    finally:
        edit_timer.stop()
        stop()

    cmds.file(new=True, force=True)
    assert not live_link.poll()
    assert not live_link.reload_current()

    # The same per-user path is used by both tools, with an optional final HDR target.
    local_data = (OUT / f"automatic-{os.getpid()}").resolve()
    with patch.dict(os.environ, {"LOCALAPPDATA": str(local_data)}), \
            patch.object(cmds, "about", return_value=False):
        automatic = maya_hdr_studio.default_sync_path()
        assert automatic == local_data / "HDR STUDIO" / "vray-sync" / "hdr_studio_vray.hdr"
        try:
            start()
        except ValueError as error:
            assert "wait for its first save" in str(error)
        else:
            raise AssertionError("Expected Start Sync to wait for HDR STUDIO's first file")
        automatic.parent.mkdir(parents=True, exist_ok=True)
        publish(automatic, bytes((128, 0, 0, 130)))
        target = automatic.with_name("target.json")
        target.write_text(json.dumps({"rendered_hdr": None}), encoding="utf-8")
        link = start()
        dome, = cmds.ls(type="VRayLightDomeShape", long=True)
        texture, = cmds.listConnections(dome + ".domeTex", source=True, destination=False)
        filename = texture + ".fileTextureName"
        saved_hdr = local_data / "Final 照明.hdr"
        publish(saved_hdr, bytes((0, 128, 0, 130)))
        target.write_text(json.dumps({"rendered_hdr": str(saved_hdr)}), encoding="utf-8")
        assert link.poll() and cmds.getAttr(filename) == saved_hdr.as_posix()
        assert link.reload_current()
        publish(automatic, bytes((0, 0, 128, 130)))
        assert link.poll() and cmds.getAttr(filename) == saved_hdr.as_posix()
        stop()
        with patch.object(cmds, "confirmDialog", return_value="Use Existing"):
            link = start()
        assert cmds.listConnections(dome + ".domeTex", source=True, destination=False) == [texture]
        assert cmds.getAttr(filename) == saved_hdr.as_posix()
        target.write_text(json.dumps({"rendered_hdr": None}), encoding="utf-8")
        assert link.poll() and link.rendered_path is None
        assert Path(cmds.getAttr(filename)).read_bytes() == automatic.read_bytes()
        stop()
    print("V-Ray dome attachment, live delay changes, single reload, IPR refresh, and scene change verified.")
finally:
    maya.standalone.uninitialize()
