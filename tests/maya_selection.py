"""Selection-only export regression: nested objects and camera-parent siblings."""
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT/'tests'/'output'
OUT.mkdir(parents=True, exist_ok=True)
os.environ['MAYA_APP_DIR'] = str(OUT/'maya-profile')
os.environ['MAYA_SKIP_USERSETUP_PY'] = '1'
import maya.standalone
maya.standalone.initialize(name='python')
import maya.cmds as cmds
sys.path.insert(0, str(ROOT/'scripts'))
import maya_hdr_studio

try:
    cmds.file(new=True, force=True)
    root = cmds.group(empty=True, name='sceneRoot')
    cmds.setAttr(root+'.translateX', 10)
    cmds.setAttr(root+'.translateY', 2)
    group = cmds.group(empty=True, name='selectedGroup', parent=root)
    chosen = cmds.polyCube(name='chosen', width=2, height=2, depth=2)[0]
    cmds.parent(chosen, group, relative=True)
    cmds.setAttr(chosen+'.translateX', 3)
    other = cmds.polyCube(name='excludedSibling')[0]
    cmds.parent(other, root, relative=True)
    cmds.setAttr(other+'.translateX', 100)
    rig = cmds.group(empty=True, name='cameraRig', parent=root)
    camera, shape = cmds.camera(name='shotCamera')
    cmds.parent(camera, rig, relative=True)
    cmds.setAttr(camera+'.translateZ', 8)
    extra = cmds.polyCube(name='excludedCameraSibling')[0]
    cmds.parent(extra, rig, relative=True)
    cmds.select(chosen, replace=True)
    before = cmds.ls(selection=True, long=True)
    maya_hdr_studio.export_selected(OUT/'maya_selected.abc', camera=camera)
    assert cmds.ls(selection=True, long=True) == before, 'Selection was not restored'
    cmds.select(group, replace=True)
    maya_hdr_studio.export_selected(OUT/'maya_selected_group.abc', camera=camera)
    assert cmds.ls(selection=True, long=True) == ['|sceneRoot|selectedGroup']
    cmds.select(clear=True)
    try:
        maya_hdr_studio.export_selected(OUT/'must_not_export.abc', camera=camera)
        raise AssertionError('Empty selection must fail')
    except ValueError:
        pass
    assert not (OUT/'must_not_export.abc').exists()
    print('MAYA_SELECTION_OK: selected subtree, parent transforms, selection restoration, empty-selection guard')
finally:
    maya.standalone.uninitialize()
