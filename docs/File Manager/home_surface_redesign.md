# Home surface redesign

Status: active. This supersedes the previous drawing-editor presentation of
Home. Phase 8's old closeout records that delivery, not completion of this revised
experience. Baseline: `before-home-surface-redesign` (`ff108d5a`).

Home is a personal composition of imported artwork, live content and deliberate
click regions. The application supplies placement, useful live views and actions;
it must not force the user to design their space from a small shape toolbox.

## Delivery checklist

- [x] Show the actual project destination before Create/Register, including in
  the Add menu. Keep the existing source-preserving project creation contract.
- [ ] Replace Home's drawing-editor toolbar/property panel with a composition
  surface. Import SVG and supported handwritten/vector documents, retain alpha,
  draw vectors directly and permit placement/resizing without decorative borders.
  Keep existing Home data recoverable during migration.
- [ ] Use a theme-aware dark/light grid. For transparent monochrome near-neutral
  artwork, adapt lightness for contrast while retaining chroma and alpha. Preserve
  multicolor artwork and opaque backgrounds; theme switching is non-destructive.
- [ ] Support immediate pressure-sensitive tablet ink with a few brushes/colors.
  Verify that native tablet pressure reaches the application; mouse/touch input
  must not be mistaken for pen pressure. Mouse drawing controls are unnecessary.
- [x] Pan only with middle-button dragging. Left input selects/places/activates;
  wheel zooms around the pointer without also scrolling an underlying folder.
- [x] Right-button tracing creates a transparent arbitrary closed click region.
  Offer a discoverable region manager with add/edit/delete, enabled state,
  meaningful labels, preview and explicit stacking order. Resolve overlaps
  deterministically, offering a chooser when multiple enabled regions match.
  Right clicks on live controls retain their ordinary menus; a drag is distinct
  from a stationary context click.
- [ ] Design one inline action picker with an explicit target summary. Reuse
  guarded folder/file navigation and the existing workflow mechanism; actions run
  only on deliberate activation, never during import, layout or preview. Do not
  encode arbitrary shell strings as a substitute for workflows.
- [ ] Let each project and its components, especially its live source-backed TODO,
  be placed/resized independently. The registry discovers projects; it does not
  dictate a single enclosing Projects box or copy tasks into Home state.
- [ ] Add live folder panes using the existing file-preview row mechanisms and
  bounded worker/cache ownership. Provide list and basic icon-grid presentation.
  Do not allocate a second FileManager/application or studio per pane.
- [ ] Give each pane its own persisted filter (type/name/relative age at minimum),
  without editing folder-owned .hide/.show or ordinary browser preferences. The
  example “Downloads modified less than one hour ago” refreshes as time passes.
- [ ] Verify vector/theme/placement, arbitrary regions and overlap choices,
  native input, per-project TODO edits, pane previews/filters, migration and restart
  in both appearances; run affected and workspace gates. Commit logical deliveries.

## Reuse and boundaries

Reuse the existing native vector callback/context, camera, notebook importers,
guarded sources, project registry/task writer, worker admission and local-state
atomic transport. Existing desk source and references must survive until migrated;
opening the new UI cannot erase old artwork, cards or pins. Separate immutable
artwork assets, source references, geometry, action targets and live view settings.

Imports and source inspection stay on bounded workers with retained source
identity. Embedded/external asset policy and resource bounds remain those of the
shared vector/document loaders. Home drops copy/import references or captured
artwork; they never authorize deleting the dragged source. Accepted mutations
retain completion ownership and close protection. Live views do not run workflows
or write project files merely because Home is opened.

Use the central palette and shared controls. Keep property/action editors docked
or inline. Selection handles and region outlines appear for editing/inspection,
not as permanent borders around transparent artwork.

## Initial findings

The existing camera already supports middle-button pan, but the embedded
drawing-editor Browse mode adds primary-button pan and white paper. Home needs
an explicit host-composition surface rather than inheriting those editor defaults.
The current Projects card already shares source-backed task reads/writes, so
individual project views can reuse that owner. The existing .note/Supernote
importer must be inspected for its real vector capabilities; its file extension
alone does not establish vector content. Native tablet input also requires
verification beyond reading egui touch-force fields.

## Individual project panel delivery

New Add → Place a project entries offer a project-folder shortcut and a separate
TODO panel for each registered project, showing its path. Panels have independent
IDs, positions, sizes and labels, while retaining the stable project UUID and the
shared bounded task reader/writer. Unregistering leaves a recoverable unavailable
panel rather than deleting either its source or layout. Existing aggregate areas
remain readable for migration; no new aggregate area is offered. Final legacy
area migration and the rest of the composition redesign remain open.

Home/model tests pass (38), including independently persisted geometry and invalid
project-ID rejection; file-manager strict Clippy and the debug build passed. Native
source-backed task workflows are being rerun in both appearances. The Add menu
has a readable minimum width so long destination paths do not squeeze into a
column of letters. The native driver scopes Home to its toolbar and keeps the
pointer over submenus when inspecting them.

## Composition surface delivery

Home now uses a distinct composition presentation of the shared canvas: no drawing
toolbar or shape properties, a neutral theme-aware dot grid, and middle-button-only
pan. Primary drag stays available for placing live panels. The full drawing editor
keeps its existing document behavior. Existing Home artwork is rendered without
paper through the same bounded helper protocol, retaining its source, geometry,
alpha, bindings and camera. There is no second renderer or GL context.

The shared native SVG scene classifies eligible near-neutral single-color ink on
its preparation worker. Home can invert its HSL lightness per frame, retaining hue,
saturation and alpha, without rewriting source colors or scheduling a theme-change
decode. Images, gradients, multiple colors, masks/effects and recognized opaque
backdrops are conservative exclusions. The initial SVG/Supernote placement delivery is recorded below; additional
handwritten/vector formats and legacy-area migration remain outstanding.

- Native review passes nine observations in each appearance:
  `/tmp/home-redesign-surface-dark-02/report.json` and
  `/tmp/home-redesign-surface-light-02/report.json`. These verify real source-backed
  checkboxes, external refresh, primary drag leaving the camera unchanged, real
  middle drag changing it, clicking actual vector ink, visibility and restart.
  Source artwork/color JSON remains byte-for-byte unchanged. Both rendered desks
  were inspected. The native pointer helper now supports middle drag explicitly.
- The full task workflow passes 21 observations with an individual project panel:
  `/tmp/home-redesign-project-dark-03/report.json` (dark initial panel delivery) and
  `/tmp/home-redesign-tasks-light-02/report.json` (light composition surface).
  It covers failed writes, missing/external sources, layout undo independence,
  ordinary editor handoff and unregistering without losing source files.
- Seven shared SVG tests, the transparent-artwork regression, nineteen shared
  drawing tests and five Home UI regressions pass. Strict all-target Clippy for
  file manager, studios, shared UI and render passes. The affected-library suite also passed; results are recorded below.
- Architecture comparison retains average/max complexity and reduces duplication
  by 0.01 points. After separating the composition and document input paths and
  separating SVG traversal/paint/backdrop classification, no new excessive-CCN
  function remains. The diff-aware tool still flags short existing/test clones;
  lifecycle, renderer, camera and protocol owners remain shared.

Remaining work is the unchecked revised scope above, including additional asset
formats, legacy-area migration, tablet input/ink, arbitrary click regions/action manager
and live folder panes/filters. None is implied complete by these foundation checks.

Affected-library gate: 2076 passed, zero failed, 19 ignored across file manager, studios, UI and render (`/tmp/home-redesign-all-affected-tests.log`). Final shared strict Clippy, build, formatting and diff checks passed. The redesigned scope remains active; this is a committed foundation, not its completion.

## Supernote ink import foundation

The existing bounded notebook helper now offers an explicit Supernote ink mode.
It excludes the background/template layer and makes the decoder's white/erased
pixels transparent. Ordinary notebook rendering retains its original paper and
template behavior. Legacy flattened pages cannot separate nonwhite template
pixels from ink, and bitmap resolution is retained rather than traced into fake
vector detail. The Home importer converts grayscale pigment coverage to a single alpha-mask
texture on its worker and tints it with the current theme. Theme switching neither
parses the notebook again nor uploads another bitmap. Colored ink keeps its colors.

All eight Supernote decoder tests pass, including unchanged canonical reference
exports and the new alpha/paper regression (`/tmp/home-redesign-note-tests.log`).
The real subprocess notebook acceptance also passes, including new transparent
ink output, dimensions, source guards, cancellation and existing Xournal and
reMarkable behavior (`/tmp/home-redesign-note-worker-tests.log`).
Strict core all-target Clippy passed (`/tmp/home-redesign-note-clippy.log`).


## Initial artwork placement delivery and review checkpoint

Home accepts SVG and Supernote `.note` references through a Copy-only native drop,
or Add → Place selected artwork after selecting files in the browser. Sources stay
linked at their original locations; importing never rewrites/removes them or
copies them into the folder previously being browsed. Repeated imports can place
separate instances of the same source. Browser-confirmed renames/moves retarget
that shared reference. Explicit Reload artwork refreshes changed source content.

SVG text is outlined with the shared fonts on the existing bounded document helper,
and its retained paths paint directly through the existing native vector renderer.
There is no permanent artwork border/background. Arrange shows selection handles;
primary drag moves artwork and corner dragging resizes uniformly. Handles retain
usable screen dimensions across zoom. The shared camera supplies drop coordinates
and middle-button pan. Placement/history is saved independently of source content.

Supernote imports use the first page's transparent ink at native bitmap density,
with grayscale coverage tinted for either appearance. These are bitmap recordings,
not vectors; magnification cannot invent stroke detail. Layered templates are
excluded, while legacy flattened nonwhite template pixels cannot be separated.
SVG normalization retains the shared embedded-image/resource policy. Unsupported
native vector effects show an explicit unavailable result.

Preparation owns two bounded workers, at most two pending reads, 128 visible
references and a conservative 128 MiB retained-artwork budget. Reads validate the
pinned source before publication and retire when hidden. Accepted Home metadata
writes retain close protection and native-drop completion ownership. No second
renderer, camera, file manager, mutation writer or GL context was introduced.

Native SVG acceptance passes in dark and light appearances:
`/tmp/home-redesign-import-dark-06/report.json` and
`/tmp/home-redesign-import-light-06/report.json`. Real client-to-client drops,
source preservation, transparent theme-adapted ink with retained warm hue, actual
move/resize input and restart persistence are exercised, not mocked events.

This is a review checkpoint. Tablet drawing, arbitrary click regions/actions and
their manager, live folder panes/filters and further asset formats remain pending.
Existing aggregate project areas remain readable until their migration is finished.

Supernote native acceptance also passes in both appearances:
`/tmp/home-redesign-import-note-dark-01/report.json` and
`/tmp/home-redesign-import-note-light-01/report.json`, using the real bundled
notebook fixture. Its ink remains transparent and readable after placement,
uniform resizing and restart, while source bytes remain unchanged.

The architecture comparison against the redesign baseline retains average/max
CCN, lowers duplication from 7.24% to 7.23%, and leaves dependency-cycle count
unchanged. It still classifies the change as a regression for two additional short
clone groups. Inspection found shared worker/admission boilerplate and test setup,
not duplicated renderer or persistence owners. The renamed Supernote render body
is reported as a new CCN-16 function; it carries forward the existing decoder's
fallible parsing and adds the explicit paper/ink choice. The native pointer test
helper has three additional branches to support real middle-button input. These
remaining diagnostics are recorded rather than claimed clean.

The review gate passes 2,081 affected library tests (19 ignored) across file
manager, studios, shared UI and render, with one test thread so bounded shared
worker queues are not overloaded by unrelated test cases:
`/tmp/home-redesign-import-affected-serial-tests.log`. The initial default-parallel
run saturated those queues and failed studio tests; the serial rerun is clean.
The current debug file-manager binary is built. Formatting and staged diff checks
pass. Strict all-target file-manager/render Clippy passes on the final source
(`/tmp/home-redesign-import-clippy-final.log`). The real diagram-helper integration gate passes all
four cases, including deployment/preprocessor guards.

The final eight Supernote decoder tests also pass after separating ink-pixel
conversion (`/tmp/home-redesign-import-note-tests.log`).


## Review feedback: direct artwork placement

Imported artwork is selectable, draggable and resizable without entering Arrange.
Clicking exposes its resize handle; hover explains movement and the handle uses a
resize cursor. Arrange remains available for arranging live panels and labels.
Ordinary shortcuts retain click-to-open behavior. This removes the hidden mode
prerequisite discovered in the first check-in review.

Validation: 42 Home tests and strict file-manager Clippy pass. The real native
artwork workflow now moves/resizes without clicking Arrange and passes in dark
mode (`/tmp/home-feedback-direct-dark-01/report.json`).


## Review feedback: appearance in Files

View → Appearance offers Light mode and Dark mode in folders, studios and Home.
Home no longer disables the View menu. The choice updates the current palette and
future studio configuration immediately, and saves through the existing shared
appearance-setting writer, preserving unrelated configuration. A failed save is
reported; successful changes do not add a status message.

The private native acceptance passes nine observations including both theme
transitions, saved preference after restart and an unrelated-setting sentinel:
`/tmp/files-appearance-menu-dark-02/report.json`. Strict file-manager all-target
Clippy and formatting pass. Architecture measurements retain duplication, average
and maximum complexity; the menu's additional branch is the necessary exception
that makes View available on Home. Reported dependency-cycle changes only reorder
unchanged existing members, and clone reports touch existing host/studio snippets.


## Transparent click region delivery

Holding the right button and tracing on the desk creates an arbitrary closed
outline. Right-clicks without a meaningful drag retain existing context menus;
project/task/shortcut controls keep their own interaction. The inline picker shows
an explicit file/folder target, offers current folder, pinned references and
registered projects, and permits an absolute path. Saving/editing never activates
the action. Targets share stable pins and the existing guarded browser/studio
handoff; source files are neither copied nor rewritten.

Click regions are transparent. Their actual even-odd polygon, including concave
corners, is the hit area; the enclosing rectangle never becomes clickable.
Hover reveals the outline and action. The Click regions button opens an inline
manager with add, enable/disable, edit target/label, redraw outline, delete and
raise/lower controls. Outlines are shown while managing/arranging and otherwise
stay out of the composition. Overlaps offer an inline chooser ordered front to
back; that click cannot also activate artwork underneath. Escape cancels drawing,
action drafts or the chooser before leaving Home.

Accepted edits use the existing guarded Home transaction and revision conflict
check. Shapes participate in layout undo/restart, source renames/merges retarget
their stable pin, and unpinning disables an area while preserving its outline for
reassignment. Limits are 128 regions and 512 finite sampled points per outline;
excessively detailed traces report a failure instead of silently truncating.
The private pointer acceptance helper now supports a bounded multi-point
right-button trace, used for actual native input rather than synthetic egui events.

The complete file-manager library gate passes 1,032 tests (6 ignored), including
concave hit geometry, invalid/collapsed geometry, region persistence, stable target
identity, stale revision rejection and layout history. Strict all-target Clippy,
formatting and the debug build pass. Native light-mode acceptance covers 21
observations, including a concave trace, its empty corner, both target pickers,
overlaps, correct navigation, disable, delete/undo, restart and unchanged sources:
`/tmp/home-regions-light-01/report.json`. The final dark-mode workflow also passes 21 observations:
`/tmp/home-regions-dark-06/report.json`.

Architecture review separated region validation, transaction installation, shared
reference interning, outline painting and activation. This removes the initial
high-complexity region surface and duplicated target insertion. Average complexity
rises by 0.01 for the new capability; maximum stays unchanged and duplication
percentage decreases 0.01 points. Remaining diagnostics concern host controls,
short test/worker snippets and the native pointer test helper's bounded trace
argument/motion branches; no renderer, navigation owner or state writer was added.

The action picker currently opens files/folders/projects. There is no registered
file-manager workflow runner to select; workflow actions remain explicitly pending,
rather than accepting arbitrary shell strings. Tablet input, live folder previews,
per-pane filters, remaining format imports and legacy-area migration remain active.
