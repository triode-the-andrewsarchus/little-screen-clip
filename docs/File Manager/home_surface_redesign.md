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
- [ ] Pan only with middle-button dragging. Left input selects/places/activates;
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
