# Renderer attribution

The pressure/speed/tilt pen models and default text-layout metrics are adapted
from [Scratchydisk/remarkable-rm](https://github.com/Scratchydisk/remarkable-rm/tree/403c345411dc451b6496339a617f2950bb5549fa/src/renderer),
by Stewart McSporran and contributors (MIT), building on Rick Lupton's rmscene
and rmc. The original notices are preserved in
[remarkable-rm-MIT.txt](licenses/remarkable-rm-MIT.txt) and
[rmscene-MIT.txt](licenses/rmscene-MIT.txt).

Rust adaptations include local segment rendering through tiny-skia, explicit
RGBA fields, typed glyph ranges, character-level CRDT ordering with compressed
tombstone intervals, and text rasterization through resvg.

[Bundled Noto Sans fonts](fonts/README.md) retain their original SIL Open Font
License. No system fonts or network access are required for rendering.
