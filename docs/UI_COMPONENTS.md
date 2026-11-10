# Shared desktop controls

`clip-ui` owns reusable desktop controls. Both applications and `clip-studios`
consume that implementation; application navigation, document edits and operation
completion stay with their existing owners.

| Control | Canonical implementation | Caller responsibility |
| --- | --- | --- |
| Ordinary action | `clip_ui::buttons::ButtonRole` | Choose Primary, Secondary or Destructive; retain enabled state, tooltip and action handler |
| Compact / inline action | `StyledButton::compact()` / `small()` | Use the shared 24-point toolbar height or native inline sizing; preserve fixed footer allocations |
| Parameter choice | `ui_ext::mode_tab_button` and its icon variant | Supply selected state and preserve parameter/toggle handling; this legacy name does not imply navigation |
| Navigation tabs | `clip_ui::ui_tabs::TabBar` | Apply a changed selection through existing entry/exit hooks; active re-selection is a no-op |
| Labeled numeric adjustment | `clip_ui::sliders::render_context_slider` | Supply range, units, default and width; preserve the domain's value mapping |
| Compact continuous track | `clip_ui::sliders::render_track_slider` | Supply value, range and width; retain the owning preview's focus/playback policy |
| Slider reference/default marks | `clip_ui::sliders::render_slider_track` | Pair with the relevant interactive control |
| Fonts, geometry, native input styling | `ui_ext::configure_workbench_style`, `ui_appearance::apply_visuals` | Apply at host setup/appearance changes, not separately for each tool |
| Icons, feedback, zoom controls | `icons`, `ui_ext::StatusToast`, `ui_ext::zoom_control_bar` | Keep operation lifetimes separate from feedback visibility |

Buttons choose their surface and contrast centrally. Primary identifies the main
affirmative action in a group; Secondary covers ordinary actions and cancellation;
Destructive identifies deletion or replacement. Destructive styling does not
authorize a write, add a confirmation or change existing safety checks. Avoid
choosing a fill independently for each ordinary action.

The lower-level colored button helpers remain useful where color conveys actual
domain data, such as a color swatch. Native menu entries, selectable list rows,
checkboxes, radio choices, text fields, integer/logarithmic sliders and numeric
drags can use egui directly with the shared style. Do not turn menus into rows of
raised action buttons or turn parameter choices into navigation tabs. A direct
egui constructor is therefore an audit candidate, not automatically a violation.

Specialized canvases—waveforms, mappings, timelines and harmony grids—retain their
domain interactions. Reuse geometry, colors and controls where they fit; shared
ownership does not require flattening these views into a generic widget.

Studio slider paths currently reexport `clip-ui` for caller compatibility. New
generic control consumers should use `clip-ui` directly. The component gallery
demonstrates the same semantic buttons and sliders used by the applications.

## Using the shared controls

Choose an action role at the call site, then attach the existing enabled state,
help text and handler. Use `compact()` for the shared 24-point toolbar density
and `small()` for inline/footer controls. Where an existing adaptive row must
match native button geometry exactly, `min_size(egui::Vec2::ZERO)` removes the
standard height without changing the button's text style. Do not grow a footer
or a conditional toolbar row merely to use the standard button height.

`render_context_slider` takes an explicit default and returns whether the value
changed, including a double-click reset. Keep preview invalidation, selection
updates and undo handling at the caller. Share a domain's default with its state
initializer, as Markup does. Percent values are normalized: `0.5` displays as
`50%`, and typing `25` or `25%` sets `0.25`. Other suffixes retain source units;
`3px` and `3 px` both work with a `" px"` suffix. Non-finite entries are rejected.

Prefer existing controls over a new wrapper. Both desktop hosts already share
palette/style setup, SVG assets, feedback and zoom controls. A host helper may
delegate to a shared semantic role, but must not independently choose its skin.

## Caller audit and deliberate exceptions

The consistency pass covers ordinary actions in the workbench, shared studios
and file manager: editor headers and footers, save/export/rename/close dialogs,
search and settings, transfer conflicts, Trash, Shelf, inspectors, database and
subtitle paging, media controls and canvas commands. Repeated selected-button
skins for effects, masks, redaction modes, resize presets, OCR, conversion,
search scope and FFT sizes use the shared parameter-choice painter.

Navigation review retains `TabBar` for workspace/tool/view/sheet navigation and
its existing selection hooks. Preset lists, sort order, chord qualities, axes,
articulation choices and search filters are parameters or data selections, not
navigation tabs. Do not migrate them to `TabBar` just because they are selectable.

Direct egui controls remain in these reviewed cases:

| Family | Reason |
| --- | --- |
| File/Edit/View, item, mount, index, filter and transfer context menus | Native menu geometry, keyboard handling and close-menu behavior |
| Spreadsheet headers/cells, PDF thumbnails and sample/track/glyph lists | Selection and data presentation, rather than ordinary command chrome |
| Rating stars, map-grid overlays and tiny grid-header icons | Domain or spatial composites with fixed hit geometry |
| Constellation category filters, MIDI tracks and decoded symbols | Color encodes category/track/symbol data |
| Integer sliders for velocity, quality, seeds, thresholds, font raster height and G-code position | Preserve integer steps and exact domain ranges |
| Audio gain, pitch, sensitivity and compressor controls | Preserve logarithmic/mapped units, modifier steps and paired numeric input |
| NTSC snow, decorrelation and stepped mask controls | Preserve precision, integer steps or paired readouts; reuse shared track/reset primitives where applicable |
| Dense zoom, font-size and thumbnail controls | Preserve existing single-row geometry and native numeric editing under the shared style |

The old studio slider module is a compatibility reexport, not a second
implementation. The native menu and data constructors above are not migration
backlogs. Specialized canvases continue to own their interactions.

## Verification

The component gallery demonstrates the production semantic roles, disabled
state, compact and inline densities, tabs and both slider forms. Interaction
tests exercise keyboard activation, disabled actions, native padding/compact
height, unit text entry, default reset and track geometry. Existing studio and
browser tests cover selection/history, write completion and fixed footer/canvas
geometry.

Verification passed: 2694 workspace tests, zero failures and 31 ignored. The
workbench's 166 tests (three ignored) were rerun after the final gallery-only
correction; no other source changed after the workspace run. All-target checking,
both desktop builds, formatting, shared API rustdoc/doctests and diagnostic
Clippy pass. Commands are serialized with Cargo builds and cache cleanup:

```sh
cargo fmt --all --check
cargo check -j4 --workspace --all-targets
TESSDATA_PREFIX=/tmp/little-clip-tessdata cargo test -j4 --workspace -- --test-threads=1
cargo build -j4 -p clip-workbench -p clip-file-manager
cargo doc -j4 -p clip-ui --no-deps
cargo clippy -j4 --workspace --all-targets -- -A clippy::approx_constant
```

The Clippy command is diagnostic: it allows the repository's pre-existing
approximate-constant errors and does not claim a warning-free `-D warnings` run.
Serial tests avoid contention between tests using shared bounded worker budgets.

UI acceptance uses only private Weston/Niri sockets and owned fixtures. It
includes dark/light gallery and file-manager chrome, scale-2 gallery, search
settings, and photo edit/history/rating/rename/export/return flows. The photo
runner retains exact label and source/output-file assertions. Additional bounded,
contrast-normalized OCR crops accommodate filled-button text omitted by sparse
full-window OCR; the processing uses one region list and one loop. The chrome
runner checks the actual indexing File-menu entry and exact 9.8 KiB fixture size.

Final private reports and inspected screenshots:

- `/tmp/ui-consistency-corrected-gallery-{dark,light,2x}/report.json`
- `/tmp/ui-consistency-final-files-{dark,light}/report.json` (eight observations each)
- `/tmp/ui-consistency-final-photos-{dark,light}/report.json` (15 observations each)
- `/tmp/ui-consistency-final-search-settings/report.json` (11 observations)
- `/tmp/ui-consistency-final-markup/report.json` (shared pixel-unit sliders)

The workspace log is `/tmp/ui-consistency-final-tests.log`; the gallery rerun is
`/tmp/ui-consistency-final-gallery-tests.log`. Final compile/build/lint logs use
`/tmp/ui-consistency-gallery-refresh-{check,build,clippy}.log`. API documentation
was built and inspected under `target/doc/clip_ui`. These are local verification
artifacts, not a claim of exhaustive visual coverage of every specialized view.

## Architecture review

Baseline: `ui-consistency-start` at `d23c49e`. The final comparison records:

| Measurement | Before | After |
| --- | ---: | ---: |
| Clone groups | 1505 | 1495 |
| Duplication | 4.38% | 4.35% |
| Average / maximum CCN | 4.05 / 195 | 4.04 / 195 |
| Total CCN | 43540 | 43534 |
| Dependency cycles | 16 | 16 |
| Functions | 10763 | 10766 |
| NLOC | 328738 | 329006 |

The small net growth includes shared APIs and interaction tests. Four redundant
audio action wrappers and repeated parameter-button skins are removed. No Cargo
dependencies are added. The local machine-readable summary is
`/tmp/ui-consistency-completed-architecture.json`.

The tool's raw result remains REGRESSION. Its modified-file clone inventory also
reports existing loading/selection/fixture code and the relocated slider tests;
shared role/slider declarations naturally repeat short call shapes. The changed
attribution score for the unchanged acceptance `word` helper is not a source
regression or a claimed source fix. The final report has no newly
reported dependency cycle.

Two test-only cognitive-complexity flags remain for `include_strip_ocr` and its
nested callback. After removing repeated processing, Lizard measures the callback
at CCN 7 / 25 NLOC and its installer at CCN 1 / 4 NLOC. Valknut attributes nested
decisions to both and reports cognitive complexity 35. The remaining conditions
select bounded OCR regions and normalization needed for reliable assertions;
there is no new production controller or separate processing pipeline per crop.
Keep this limitation explicit rather than adding abstractions to hide a score.
