# Workbench visual refresh

Branch: `feat/workbench-visual-refresh`, based on `feat/workbench-ui-overhaul` at `02cc03c`.

The visual brief is warm neutrals and charcoal, perceptually derived mint/sky/apricot/lilac accents, readable proportional type, monospace technical readouts, and an instrument-panel hierarchy without decorative machinery.

The refresh extends the existing design system: core owns the shared fixed palettes, `ui_appearance` owns egui roles, and shared buttons, `TabBar`, and `StudioShell` carry the treatment into the viewers. Fira Sans is embedded with its OFL license; existing monospace and Unicode fallback faces remain. Buttons have defined surfaces and edges, active tabs connect visibly to their navigation strip, primary actions stand apart from contextual actions, studio icon tiles identify viewer families, and outlined metadata chips keep headers quieter. Node cards use neutral bodies, lilac script accents, and a visible alignment grid. Quick Palette highlights the keyboard execution target.

Transparent viewer areas now paint the palette canvas. Raw/code text in the little-screen mirror selects the corresponding syntax appearance and resolves token contrast against card surfaces. ANSI comparison data retains its shared dark console viewport.

## Review images

| Surface | Capture |
| --- | --- |
| Original shared gallery | [Before, Dark](before-gallery-dark.png) |
| Refreshed shared gallery | [After, Dark](after-gallery-dark.png) |
| Shared controls | [Dark](controls-dark.png), [Light](controls-light.png) |
| Quick Palette | [Light](palette-light.png) |
| Node Machine | [Dark](nodes-dark.png) |
| Image Studio | [Light](image-light.png) |
| Raw text mirror contrast | [Light](text-mirror-light.png) |
| Text comparison and console | [Light](diff-light.png) |

These are screenshots from the private Weston/Niri runner, not mockups. The blue outer focus frame belongs to the private compositor.

## Verification

- `cargo build -p clip-workbench`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.
- `cargo test --workspace -- --test-threads=1`: passed (1,116 passed, eight existing ignored tests).
- `cargo clippy --workspace --all-targets -- -D warnings`: fails on pre-existing core and vendored diagnostics. Detached-parent runs of workspace Clippy, core Clippy, and filmr Clippy cover every reported current diagnostic by message and file. This is not a claim that later crates, skipped when dependencies fail, are lint-clean.
- Architecture MCP baseline, reuse search, and comparison each timed out after 300 seconds. Source reuse was reviewed directly. Local Lizard comparison of all changed Rust files records 299 → 300 functions (the addition is the raster regression test), CCN 924 → 934, and maximum CCN 74 → 76. The increases cover selected-row cues, family accent selection, widget styling, and appearance-aware text contrast; no new production functions or subsystem were introduced. Duplication and dependency-cycle metrics remain unverified by the unavailable service.

The existing contrast matrix passes in both appearances. The added `text_preview_rasters_have_readable_ink_on_light_and_dark_cards` checks actual plain-text and Rust rasters on hero, prior, and recessed surfaces. Existing tests continue to cover tab wrapping and keyboard behavior, font fallback, shared header/footer geometry, Play/Pause width stability, MIDI data colors, viewer routing, and bounded background work.

Private captures cover Quick Palette, Node Machine, Image Resize, Audio Inspect, MIDI, PDF, Markdown, Spreadsheet with sheet tabs, 3D, text comparison, and binary comparison, plus the component gallery in both appearances. The compact Image Studio case uses 1280×720 logical at 150% scale; the gallery uses 1600×1000; other viewer captures use 1280×800. Every listed successful capture report passes its fixed-popup, centering, and singleton checks. Captures are layout evidence, not exhaustive action parity or physical USB acceptance.

Reproduce the fixtures with `python3 scripts/ui_acceptance_fixtures.py --output-dir /tmp/clip-visual-fixtures`. Run `scripts/ui_acceptance_capture.py --help` for private viewer routes. For example:

```bash
python3 scripts/ui_acceptance_capture.py --output-dir /tmp/clip-visual-gallery-review \
  --component-gallery --appearance dark --width 1600 --height 1000 \
  --click 1400 180 --scroll 1100 700 1000 --move 10 10
```

One digital-decode worker test failed transiently while builds and captures were active; its isolated retry passed. The final serial workspace run passed. Personal configuration, the host compositor, and USB hardware were not accessed.
