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
- [ ] Right-button tracing creates a transparent arbitrary closed click region.
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
backdrops are conservative exclusions. This is the rendering foundation; importing
SVG/notebook assets as independent movable objects remains outstanding.

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

Remaining work is the unchecked revised scope above, including asset placement,
legacy-area migration, tablet input/ink, arbitrary click regions/action manager
and live folder panes/filters. None is implied complete by these foundation checks.

Affected-library gate: 2076 passed, zero failed, 19 ignored across file manager, studios, UI and render (`/tmp/home-redesign-all-affected-tests.log`). Final shared strict Clippy, build, formatting and diff checks passed. The redesigned scope remains active; this is a committed foundation, not its completion.

## Supernote ink import foundation

The existing bounded notebook helper now offers an explicit Supernote ink mode.
It excludes the background/template layer and makes the decoder's white/erased
pixels transparent. Ordinary notebook rendering retains its original paper and
template behavior. Legacy flattened pages cannot separate nonwhite template
pixels from ink, and bitmap resolution is retained rather than traced into fake
vector detail. The Home importer prepares grayscale light/dark variants on its
worker; changing themes does not parse the notebook again.

All eight Supernote decoder tests pass, including unchanged canonical reference
exports and the new alpha/paper regression (`/tmp/home-redesign-note-tests.log`).
The real subprocess notebook acceptance also passes, including new transparent
ink output, dimensions, source guards, cancellation and existing Xournal and
reMarkable behavior (`/tmp/home-redesign-note-worker-tests.log`).
Strict core all-target Clippy passed (`/tmp/home-redesign-note-clippy.log`).
