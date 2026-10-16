#!/usr/bin/env python3
"""OBJ/glTF meshes with guarded external resources in private Weston/Niri."""
import json,struct
from pathlib import Path
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    resources=root/'resources';resources.mkdir()
    (resources/'red material.mtl').write_text('newmtl Red\nKd 1 0 0\n')
    (root/'a-triangle.obj').write_text('mtllib resources/red material.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Red\nf 1 2 3\n')
    (resources/'geometry.bin').write_bytes(struct.pack('<9f',0,0,0,1,0,0,0,1,0))
    Image.new('RGBA',(2,2),(255,0,0,255)).save(resources/'texture.png')
    document={'asset':{'version':'2.0'},'buffers':[{'uri':'resources/geometry.bin','byteLength':36}],'bufferViews':[{'buffer':0,'byteLength':36}],'accessors':[{'bufferView':0,'componentType':5126,'count':3,'type':'VEC3','min':[0,0,0],'max':[1,1,0]}],'meshes':[{'primitives':[{'attributes':{'POSITION':0}}]}],'nodes':[{'mesh':0}],'scenes':[{'nodes':[0]}],'scene':0,'images':[{'uri':'resources/texture.png'}]}
    (root/'b-triangle.gltf').write_text(json.dumps(document))
    (root/'notes.txt').write_text('Ordinary text beside guarded model previews.\n')


def exercise(session,root):
    session.observe('01-mesh-rows',['a-triangle','b-triangle','Material colors'])
    session.click(848,201)
    session.observe('02-mixed-rows',['a-triangle','b-triangle','notes.txt','1 triangles'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'));output,root=media_fixture_directory(args);fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'Guarded OBJ/glTF external-resource import and mixed-list previews')


if __name__=='__main__':
    raise SystemExit(main())
