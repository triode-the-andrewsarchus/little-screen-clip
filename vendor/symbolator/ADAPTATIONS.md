# Local Symbolator adaptations

Pristine zebreus/symbolator 1.2.2 source was imported separately before these changes.

- Defer Cairo/Pango imports until their original raster/metric code paths are used.
  The application supplies in-memory Pillow metrics from the same bundled fonts
  used by SVG outlining. Symbolator's real NuCanvas SVG shape writer remains in use.
- Allow NuCanvas SVG output to use an already-owned text stream through
  `contextlib.nullcontext`, so no staging pathname or filesystem write is required.
- Fix the actual Verilog lexer to recognize a port direction after a module header
  on the same line. The original line anchor silently lost the first ANSI port.
  `docs/examples/diagrams/module.symbolator.v` is the regression fixture.
- Fix the actual VHDL entity lexer to accept `end entity name;`, `end name;`,
  `end entity;` and `end;`. The original expression rejected the common named form.
  `docs/examples/diagrams/entity.symbolator.vhdl` is the regression fixture.
- Accept VHDL component declarations with the optional `is` omitted, as VHDL allows.
  `docs/examples/diagrams/component.symbolator.vhdl` is the regression fixture.

The adapter calls source-string extraction only, never the CLI, directory scan,
array-type cache loader, HDL simulator, preprocessor or file-based parser entrypoints.
Canonical section colors, clock triangles, active-low bubbles, bus whiskers,
bidirectional arrows and generic/parameter blocks remain in the upstream renderer.
Interface extraction is not HDL compilation or elaboration; the preview reports
that limitation. The shared Python runtime owns import/resource/process policy.
