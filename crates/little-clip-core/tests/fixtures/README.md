The 8×6 red-to-blue gradient fixtures were generated for this repository using
ImageMagick. `gradient.heic` also has EXIF Make=LittleScreenClip,
Model=CodecFixture, and DateTimeOriginal=2026:09:30 12:00:00 added using ExifTool.
They test real HEVC/AV1 decoding and HEIF EXIF extraction without external tools
at test runtime. These fixtures share the repository's license.

`panels.jxl` and `panels-alpha16.jxl` are 64×32 two-panel images created for
this repository with `cjxl` 0.12.0 (`-d 0 -e 1 --num_threads=1`). The left
panel is RGB (30, 120, 210), and the right is (180, 60, 40). The first uses
an 8-bit RGB PPM input and `--container=0`; the second uses a 16-bit RGBA
PAM input and `--container=1`, with half opacity on the left. They exercise
both JPEG XL signatures, lossless pixel values and premultiplied transparency
without requiring an encoder at test runtime. They share the repository's license.
`gray16.jxl` (8×6, luminance 16384) and `float.jxl` (8×6, RGB 0.25/0.5/0.75)
use the same lossless encoder flags with 16-bit PGM and little-endian float PFM
inputs, respectively, to verify native sample depths.
