#!/usr/bin/env python3
"""Create owned XCF layer/group/mask fixtures with installed GIMP 3 in isolation."""
from pathlib import Path
import shutil
import subprocess
import tempfile

BATCH = '''
from gi.repository import Gimp, Gio, Gegl
image=Gimp.Image.new(96,64,Gimp.ImageBaseType.RGB)
base=Gimp.Layer.new(image,'Paper',96,64,Gimp.ImageType.RGBA_IMAGE,100,Gimp.LayerMode.NORMAL)
image.insert_layer(base,None,0)
Gimp.context_set_foreground(Gegl.Color.new('#e6e6e6'))
base.fill(Gimp.FillType.FOREGROUND)
group=Gimp.GroupLayer.new(image,'Panel group')
image.insert_layer(group,None,0)
blue=Gimp.Layer.new(image,'Blue panel',48,32,Gimp.ImageType.RGBA_IMAGE,100,Gimp.LayerMode.NORMAL)
image.insert_layer(blue,group,0)
blue.set_offsets(24,16)
Gimp.context_set_foreground(Gegl.Color.new('#1964dc'))
blue.fill(Gimp.FillType.FOREGROUND)
mask=blue.create_mask(Gimp.AddMaskType.WHITE)
blue.add_mask(mask)
image.select_rectangle(Gimp.ChannelOps.REPLACE,24,16,24,32)
Gimp.context_set_foreground(Gegl.Color.new('black'))
mask.edit_fill(Gimp.FillType.FOREGROUND)
Gimp.Selection.none(image)
hidden=Gimp.Layer.new(image,'Hidden red',96,64,Gimp.ImageType.RGBA_IMAGE,100,Gimp.LayerMode.NORMAL)
image.insert_layer(hidden,None,0)
Gimp.context_set_foreground(Gegl.Color.new('red'))
hidden.fill(Gimp.FillType.FOREGROUND)
hidden.set_visible(False)
assert Gimp.file_save(Gimp.RunMode.NONINTERACTIVE,image,Gio.File.new_for_path('/output/layered.xcf'))
image.convert_precision(Gimp.Precision.FLOAT_LINEAR)
assert Gimp.file_save(Gimp.RunMode.NONINTERACTIVE,image,Gio.File.new_for_path('/output/layered-float.xcf'))
for layer in image.get_layers():
    layer.set_visible(False)
assert Gimp.file_save(Gimp.RunMode.NONINTERACTIVE,image,Gio.File.new_for_path('/output/hidden.xcf'))
image.delete()
'''


def main():
    destination=Path(__file__).resolve().parents[1]/'crates/little-clip-core/tests/fixtures'
    with tempfile.TemporaryDirectory(prefix='xcf-fixture-') as temporary:
        subprocess.run(['bwrap','--die-with-parent','--unshare-all','--new-session','--ro-bind','/usr','/usr',
                        '--symlink','usr/bin','/bin','--symlink','usr/lib','/lib','--symlink','usr/lib','/lib64',
                        '--dir','/etc','--ro-bind','/etc/fonts','/etc/fonts','--ro-bind','/etc/ld.so.cache','/etc/ld.so.cache',
                        '--proc','/proc','--dev','/dev','--tmpfs','/tmp','--setenv','HOME','/tmp',
                        '--setenv','XDG_CONFIG_HOME','/tmp/config','--setenv','XDG_CACHE_HOME','/tmp/cache',
                        '--setenv','XDG_DATA_HOME','/tmp/data','--setenv','GEGL_USE_OPENCL','no','--setenv','GEGL_THREADS','2',
                        '--bind',temporary,'/output','/usr/bin/gimp-console','--new-instance','--no-data','--no-fonts',
                        '--quit','--batch-interpreter=python-fu-eval','--batch',BATCH],check=True,timeout=30,capture_output=True)
        for name in ['layered.xcf','layered-float.xcf','hidden.xcf']:
            shutil.copyfile(Path(temporary)/name,destination/name)


if __name__=='__main__':
    main()
