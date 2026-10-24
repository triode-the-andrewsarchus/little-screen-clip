# Constellation redesign review

This pass implements the requested redesign after `aaa99bd`. The sidebar retains
its 360 px default and 280–420 px limits. Shelf behavior is unchanged.

## Changes and commits

| Change | Commit |
| --- | --- |
| Height-aware square pads at the existing rack width | `4d2c0ca` |
| Grouped editor and Shift-right-click mute-group gestures | `6adb646` |
| Drag complete instruments between pad positions | `30f87fa` |
| Audio Studio handoff and guarded sibling-save return | `56e4067` |
| dB gain, centered pan and logarithmic fade sliders | `950b711` |
| Shared Studio viewport/ruler and improved trim canvas | `bb9e044` |
| Inline filters, shorter categories and quieter toolbar | `6f10279` |
| Density-adaptive sizing while rendering every point | `56ffa33` |
| Visual compressor characteristic and live output meters | `ad88edf` |

Follow-up commits fix compact layout, isolate pad gestures, share export range
planning, and protect typed drag payloads. In egui 0.27, asking for the wrong
payload type consumes it; explicit type checks now protect internal pad swaps
from the file-drop handler. A pointer-release regression test covers that failure.

Hovering a grouped pad draws red connections and names its linked keys. Alt shows
details, keeping normal hover unobstructed. Swaps preserve key/MIDI positions,
move the complete instrument, stop affected voices and support Undo.

Audio Studio returns an edited sibling through the existing guarded export
pipeline. Naming collisions retry atomic installation without re-encoding or
overwriting either the source or another sibling. A changed originating layer
rejects replacement while retaining the saved file. Actual published paths drive
the handoff and notification.

## Architecture comparison

The architecture MCP tools were used before implementation, for reuse searches,
and after the changes. Current-main comparison uses fetched `origin/main`
`999cd24c9f28814cf06441a1a7be780ac9e5c20c`, rather than only the latest commit.

| Metric | Current main | Start of this pass | Result |
| --- | ---: | ---: | ---: |
| NLOC | 227,320 | 238,628 | 239,914 |
| Files | 609 | 628 | 631 |
| Functions | 6,934 | 7,210 | 7,254 |
| Total CCN | 27,909 | 29,209 | 29,392 |
| Average CCN | 4.02 | 4.05 | 4.05 |
| Maximum CCN | 196 | 196 | 196 |
| Duplication | 5.98% | 5.74% | 5.69% |
| Clone groups | 1,382 | 1,394 | 1,393 |
| Reported cycles | 5 | 6 | 6 |
| Valknut health | 73.88 | 62.73 | 62.73 |

Both comparisons report **REGRESSION**. This pass reduces clone groups by one
and duplication by 0.05 percentage points; the full branch adds eleven clone
groups and lowers the aggregate health score. The additional reported cycle
relative to main is bounded Python recursion in the model conversion script,
not a Rust module cycle. Different checkout prefixes cause unchanged Python
cycles to appear as both added and resolved; those are not claimed as fixes.

The new waveform renderer separates ruler, peaks, markers and trim interaction.
Studio close confirmation and completion polling have their own responsibilities.
Save dialog names and actual exports now share region planning, eliminating an
existing 21-line duplicate. Both map projection paths share filter-color policy.

Remaining modified hotspots flagged in this pass:

- `audio_preview/export.rs::save_audio_region`: CCN 15 → 16; returns the actual
  installed sibling name while retaining source checks and pinned-directory IO.
- `file_manager/app.rs::main_content`: 15 → 18; independent normal-file and
  pad-studio handoff routing.
- `constellation/ui.rs::render_ui`: 14 → 20; Output-tab lifecycle and editor
  visibility conditions.
- `constellation/pack_shelf_ui.rs::render_pad_grid`: 17 → 18; height-aware pad
  layout and relationship feedback. Gesture handling is separate.

`execute_save` falls from CCN 19 to 1, delegating to the shared guarded plan.
The branch-wide report also retains indexing/feature-extraction hotspots,
including `extract_features` at CCN 56. This is remaining technical debt, not
evidence of a clean architecture verdict. Several “new_or_modified_clone”
entries are pre-existing export/test patterns touching edited files; that label
alone does not establish a newly added clone. Repeated test fixture setup and
the duplicated map dimming policy introduced during this pass were consolidated.

Reuse searches identified existing Studio viewport/ruler helpers and sliders.
Waveform colors and fade envelopes use the actual shared audio mechanisms.
No dependency was added.

## Validation

- `cargo test --workspace -- --test-threads=2`: **2,544 passed**, 0 failed,
  23 ignored across 26 test targets, including doctests.
- Focused constellation tests: **71 passed**. Tests cover actual OUT-marker
  pointer dragging, typed pad drops, swaps/Undo/stale drags, mute linking,
  guarded Studio return, compact/tall dark/light layouts, and camera stability.
- Dense rendering fixture: **68,055 points**, four vertices each, including
  dimmed points under category/search filters.
- FFmpeg sibling-export test verifies original/sibling preservation, actual
  returned filename, and selected output duration.
- Private Weston/Niri acceptance exercises real keyboard and pointer input,
  native file drops, swaps, mute groups, Studio save/return, output controls,
  pack versioning/export, Undo/Redo, close protection and navigation.
  Both final runs passed all 35 observations:
  `/tmp/constellation-redesign-dark-13` and
  `/tmp/constellation-redesign-light-4` (hardware keys). Evidence includes raw PNGs,
  OCR observations, saved bank metadata, original-preservation assertions and WAV
  export assertions. Hardware-remapped bindings use `[";qjk", "aoeu", "',.p", "1234"]`.
- `cargo build -p clip-workbench --bin clip-file-manager`: passed, with the two
  existing float-fallback warnings in mounts/redaction code.
- `cargo fmt --all --check`, `git diff --check`, and acceptance Python compilation:
  passed.
- `cargo doc -p clip-workbench --no-deps --document-private-items`: passed; inspected
  the generated viewport example and save API documentation. Existing Hilbert
  bare-URL rustdoc warning remains.
- Diagnostic `cargo clippy -p clip-workbench --lib -- -A clippy::approx_constant`:
  completed with 583 existing warnings, matching the baseline. This is not a
  warning-free strict Clippy result. The existing `1.4142` approximation in
  `little-clip-ops/src/creative_fx/cmyk_halftone.rs` blocks the unmodified strict
  invocation; no source-level lint suppression was added.

Private audio uses an ALSA null sink. Physical ALSA recovery, a physical MIDI
controller and the subjective sound of compression still need real hardware
listening; passing these tests does not establish that a physical POLLERR has
been reproduced or eliminated.

See [drum rack usage](constellation-drum-rack.md) for gestures and configuration.

