"""Run with mayapy; only writes disposable fixtures under tests/output."""
import os
import sys
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'tests' / 'output'
OUT.mkdir(parents=True, exist_ok=True)
os.environ['MAYA_APP_DIR'] = str(OUT / 'maya-profile')
os.environ['MAYA_SKIP_USERSETUP_PY'] = '1'
import maya.standalone
maya.standalone.initialize(name='python')
import maya.cmds as cmds
import maya.api.OpenMaya as om
sys.path.insert(0, str(ROOT / 'scripts'))
import maya_export

try:
    cmds.file(new=True, force=True)
    cmds.currentUnit(linear='cm',time='film')
    cmds.currentTime(1)
    root = cmds.group(empty=True, name='asset')
    cube = cmds.polyCube(name='cube', width=2, height=3, depth=2)[0]
    cmds.parent(cube, root)
    cmds.setAttr(root+'.translateX', 2)
    camera, shape = cmds.camera(name='renderCam')
    cmds.setAttr(camera+'.translateX', 2)
    cmds.setAttr(camera+'.translateZ', 8)
    for existing in cmds.ls(type='camera'):
        cmds.setAttr(existing+'.renderable',existing==shape)
    cmds.setAttr(shape+'.focalLength', 50)
    cmds.setAttr('defaultResolution.width',1920)
    cmds.setAttr('defaultResolution.height',1080)
    cmds.setAttr('defaultResolution.pixelAspect',1)
    cases = []
    for fit in range(4):
        cmds.setAttr(shape+'.filmFit',fit)
        cmds.setAttr(shape+'.horizontalFilmOffset',0.07)
        cmds.setAttr(shape+'.verticalFilmOffset',-0.03)
        cmds.setAttr(shape+'.lensSqueezeRatio',1.2)
        cmds.setAttr(shape+'.filmRollValue', 7 if fit==3 else 0)
        path = OUT / ('maya_fit_{}.abc'.format(fit))
        data = maya_export.export(path,camera=camera)
        record=data['cameras'][0]
        # Independent Maya MPoint multiplication checks the Rust matrix layout/order.
        view=om.MMatrix(record['view'])
        projection=om.MMatrix(record['projection'])
        probes=[]
        for p in [(2,0,0),(1,-1.5,-1),(3,1.5,1)]:
            q=om.MPoint(*p)*view*projection
            probes.append({'world':list(p),'ndc':[q.x/q.w,q.y/q.w,q.z/q.w]})
        cases.append({'file':path.name,'probes':probes})
    cmds.setAttr(shape+'.filmRollValue',0)
    cmds.setAttr(shape+'.orthographic',1)
    cmds.setAttr(shape+'.orthographicWidth',12)
    data=maya_export.export(OUT/'maya_ortho.abc',camera=camera)
    record=data['cameras'][0]
    p=om.MMatrix(record['projection'])
    assert abs(p[15]-1)<1e-6, 'Orthographic projection must keep w=1'
    # Non-square pixels must affect the displayed gate, not the window size.
    cmds.setAttr(shape+'.orthographic',0)
    cmds.setAttr('defaultResolution.pixelAspect',1.5)
    maya_export.export(OUT/'maya_pixel_aspect.abc',camera=camera)
    (OUT/'maya_reference.json').write_text(json.dumps(cases,indent=2),encoding='utf-8')
    # An actual Maya Software render is an independent oracle for image framing.
    cmds.delete(root)
    cmds.setAttr('defaultResolution.width',640)
    cmds.setAttr('defaultResolution.height',360)
    cmds.setAttr('defaultResolution.pixelAspect',1)
    cmds.setAttr('defaultRenderGlobals.currentRenderer','mayaSoftware',type='string')
    cmds.setAttr('defaultRenderGlobals.imageFormat',32)  # PNG
    cmds.setAttr('defaultRenderGlobals.extensionPadding',0)
    project=OUT/'render-project'
    project.mkdir(exist_ok=True)
    cmds.workspace(str(project),newWorkspace=True)
    cmds.workspace(fileRule=['images',str(OUT)])
    marker_positions=[(1.1,-0.4,0),(2,0.5,0),(2.9,-0.3,0)]
    for index,position in enumerate(marker_positions):
        marker=cmds.polyPlane(name='marker'+str(index),width=0.12,height=0.12,subdivisionsX=1,subdivisionsY=1,axis=(0,0,1))[0]
        cmds.xform(marker,worldSpace=True,translation=position)
        shader=cmds.shadingNode('surfaceShader',asShader=True)
        color=[0,0,0];color[index]=1
        cmds.setAttr(shader+'.outColor',*color,type='double3')
        group=cmds.sets(renderable=True,noSurfaceShader=True,empty=True)
        cmds.connectAttr(shader+'.outColor',group+'.surfaceShader',force=True)
        cmds.sets(marker,edit=True,forceElement=group)
    render_cases=[]
    for fit in range(4):
        cmds.setAttr(shape+'.filmFit',fit)
        cmds.setAttr(shape+'.filmRollValue',7 if fit==3 else 0)
        name='maya_marker_'+str(fit)
        maya_export.export(OUT/(name+'.abc'),camera=camera)
        cmds.setAttr('defaultRenderGlobals.imageFilePrefix',name,type='string')
        rendered=cmds.render(camera,x=640,y=360)
        render_cases.append({'abc':name+'.abc','image':str(rendered),'positions':marker_positions})
    (OUT/'maya_render_reference.json').write_text(json.dumps(render_cases,indent=2),encoding='utf-8')
    print('MAYA_EXPORT_OK: four Film Fit modes, offsets, squeeze, roll, orthographic, pixel aspect')

    cmds.delete(cmds.ls('marker*', type='transform'))
    cmds.setAttr(shape+'.filmFit', 0)
    cmds.setAttr(shape+'.filmRollValue', 0)
    cmds.setAttr(shape+'.horizontalFilmOffset', 0)
    cmds.setAttr(shape+'.verticalFilmOffset', 0)
    cmds.setAttr(shape+'.lensSqueezeRatio', 1)
    cmds.setAttr(camera+'.translateX', 0)
    cmds.setAttr(camera+'.translateZ', 14)
    smooth = cmds.polySphere(name='smoothSphere', subdivisionsX=16, subdivisionsY=12)[0]
    cmds.polySoftEdge(smooth, angle=180, constructionHistory=False)
    cmds.setAttr(smooth+'.translateX', -1.25)
    hard = cmds.polySphere(name='hardSphere', subdivisionsX=16, subdivisionsY=12)[0]
    cmds.polySoftEdge(hard, angle=0, constructionHistory=False)
    cmds.setAttr(hard+'.translateX', 1.25)
    maya_export.export(OUT/'maya_shading.abc', camera=camera)
finally:
    maya.standalone.uninitialize()
