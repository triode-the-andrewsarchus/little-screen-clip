import os
from gi.repository import Gimp, Gio
image = Gimp.file_load(Gimp.RunMode.NONINTERACTIVE, Gio.File.new_for_path('/input.xcf'))
if image is None:
    raise RuntimeError('GIMP could not load the XCF document')
if any(layer.get_visible() for layer in image.get_layers()):
    image.merge_visible_layers(Gimp.MergeType.CLIP_TO_IMAGE)
else:
    if image.get_base_type() != Gimp.ImageBaseType.RGB:
        image.convert_rgb()
    canvas = Gimp.Layer.new(image, 'Render canvas', image.get_width(), image.get_height(), Gimp.ImageType.RGBA_IMAGE, 100, Gimp.LayerMode.NORMAL)
    image.insert_layer(canvas, None, 0)
    canvas.fill(Gimp.FillType.TRANSPARENT)
if image.get_base_type() != Gimp.ImageBaseType.RGB:
    image.convert_rgb()
if image.get_precision() != Gimp.Precision.U8_NON_LINEAR:
    image.convert_precision(Gimp.Precision.U8_NON_LINEAR)
image.convert_color_profile(Gimp.ColorProfile.new_rgb_srgb(), Gimp.ColorRenderingIntent.RELATIVE_COLORIMETRIC, True)
width = int(os.environ.get('LITTLE_CLIP_XCF_WIDTH', image.get_width()))
height = int(os.environ.get('LITTLE_CLIP_XCF_HEIGHT', image.get_height()))
if (width, height) != (image.get_width(), image.get_height()):
    image.scale(width, height)
procedure = Gimp.get_pdb().lookup_procedure('file-png-export')
config = procedure.create_config()
config.set_property('run-mode', Gimp.RunMode.NONINTERACTIVE)
config.set_property('image', image)
config.set_property('file', Gio.File.new_for_path('/tmp/composite.png'))
config.set_property('include-color-profile', False)
config.set_property('include-exif', False)
config.set_property('include-iptc', False)
config.set_property('include-xmp', False)
config.set_property('include-thumbnail', False)
config.set_property('include-comment', False)
result = procedure.run(config)
if result.index(0) != Gimp.PDBStatusType.SUCCESS:
    raise RuntimeError('GIMP could not export the rendered canvas')
with open('/tmp/composite.png', 'rb') as rendered:
    while True:
        block = rendered.read(65536)
        if not block:
            break
        view = memoryview(block)
        while view:
            view = view[os.write(1, view):]
image.delete()
