"""Maya Script Editor (Python): export an Ogawa ABC and matching render cameras.

import sys
sys.path.insert(0, r'C:/path/to/HDR_STUDIO/scripts')
import maya_export
maya_export.export(r'C:/temp/lighting.abc', camera='renderCam')

Exports the current frame only. Scene geometry remains in Maya's internal cm.
Camera JSON uses OpenGL column-vector matrices and the final render aspect.
"""
import json
import math
from pathlib import Path


def _flat(matrix):
    # Maya row-major/row-vector -> column-major/column-vector: same flat storage.
    return list(matrix)


def _camera_record(path, width, height, pixel_aspect):
    import maya.api.OpenMaya as om
    selection = om.MSelectionList()
    selection.add(path)
    dag = selection.getDagPath(0)
    if dag.apiType() != om.MFn.kCamera:
        dag.extendToShape()
    fn = om.MFnCamera(dag)
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
    projection = om.MMatrix(values) * om.MMatrix(fn.postProjectionMatrix())
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
    import maya.cmds as cmds
    import maya.api.OpenMaya as om
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
    seconds = om.MTime(cmds.currentTime(query=True), om.MTime.uiUnit()).asUnits(om.MTime.kSeconds)
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
        original = om.MGlobal.getActiveSelectionList()
        temporary = om.MSelectionList()
        for obj in sorted(set(export_selection)):
            temporary.add(obj)
        try:
            om.MGlobal.setActiveSelectionList(temporary)
            cmds.AbcExport(j=job + ' -selection')
        finally:
            om.MGlobal.setActiveSelectionList(original)
    info = path.stat()
    data = {'version': 1, 'sample_seconds': seconds, 'abc_size': info.st_size,
            'abc_mtime_ns': info.st_mtime_ns, 'cameras': records}
    if not all(math.isfinite(v) for c in records for key in ('view', 'projection') for v in c[key]):
        raise ValueError('Non-finite camera matrices')
    path.with_suffix('.camera.json').write_text(json.dumps(data, indent=2), encoding='utf-8')
    print('HDR STUDIO exported: {} (sample {} seconds)'.format(path, seconds))
    return data
