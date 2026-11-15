# Phase 8 completion — personal Home and organization

Active objective: complete the full Phase 8 scope, including the user's expanded
Home direction. The earlier static-pin delivery is a foundation, not completion.
Work stays on `feat/file-manager-phase8`; personal Cargo/runtime configuration
changes remain outside commits. Baseline: `before-phase8-completion`.

## Experience

Home is a personal, zoomable desk. Users draw, write, arrange and link their own
landmarks; hand-drawn artwork can become a shortcut. Live file/project cards sit
among those drawings rather than forcing a dashboard grid. Browse and Arrange
modes distinguish opening/checking things from drawing/moving things. No floating
property windows, browser engine, or rasterized-at-one-zoom drawing layer.

Reuse the existing native drawing studio, vector renderer and canvas camera.
Keep artwork, local presentation and target references distinct. Support pan,
zoom, fit, drawing/text/shapes, selection, resize, undo/redo and persistent layout.
Named sections frame areas of the desk. Provide recoverable defaults, keyboard
access and a readable appearance in both themes. Existing pins migrate losslessly.

Projects stay in their own ordinary folders. Home contains references and live
views; it never becomes the owner of project files. A Projects area can reflect
registered projects as they are added/removed, with task excerpts backed by each
project's own TODO document. Checking a task edits that source document through
identity-checked atomic replacement; failures remain visible, external edits are
not overwritten, and unchecked/checked state is not duplicated in Home JSON.
This specific registry-backed area does not introduce the Phase 10 query engine.

## Delivery ledger

- [ ] C1. Expose the existing native drawing surface for host-managed live overlays
      and clickable artwork without copying its renderer/editor.
- [ ] C2. Home canvas: migration, free arrangement, artwork shortcuts, named
      sections, live card placement, view state, undo/redo and deliberate editing.
- [ ] C3. Versioned `.fmproject`, safe create/register workflow, no-overwrite
      README/TODO defaults and local global project registry.
- [ ] C4. Bounded live project/TODO views, source-backed checkboxes, refresh/error
      handling and project-area membership from the registry.
- [ ] C5. Session red/yellow/green marks with editable meanings; distinct labeled
      underlines, persistence and clearing/recovery.
- [ ] C6. Configurable A/B/... sets, membership/filtering and repeated subdivision;
      independent from session state and visibility.
- [ ] C7. `.hide`/`.show` syntax, precedence/inheritance, selective dotfile reveal,
      labeled solid backgrounds and an obvious recovery/ignore-rules control.
- [ ] C8. Home browser history/startup behavior; defined/tested rename/move/restart
      handling for pins, marks, sets and project identities.
- [ ] C9. Realistic review fixtures, native interaction/visual verification in both
      themes, affected tests plus workspace gate, strict Clippy, formatting,
      architecture review and logical commits as each delivery becomes usable.

## Storage and mutation contracts

Local Home, organization and registry state use separate versioned models over
the shared guarded `local_state` transport/worker. Preserve unknown/newer input
and report failures; migrate old Home pins without touching source content.
Cooperating windows merge semantic changes, never replace another window's newer
state with a stale snapshot. Layout/drawing writes must retain their completion
owner and defer close. Bound canvas size/elements/points, source documents,
visible project reads, worker admission and retained previews.

Project manifests are portable project-owned metadata with stable UUID identity,
a readable title and relative README/TODO references confined to the project.
The local registry maps that identity to observed folder references; project
content and tasks remain there. Do not infer original identity from an old path,
follow unvalidated relationship targets or overwrite existing files on creation.
Browser-managed moves/renames update local references only after confirmed
success; external moves need explicit re-registration or report unavailable.

Visibility rules are project/folder-owned, not Home state. Specify exact matching,
negation/precedence, ancestor scope, bounds and errors before enabling them.
Rule recovery must always be accessible even if all normal items are hidden.
Session/sets must remain separate models and visual signals from these rules.

## Completion audit

Each ledger item needs committed implementation plus requirement-specific tests
or native evidence. C2/C4 need observed live interactions, not static screenshots
alone. Verify external TODO edits, write failures, stale identities, restart,
rename/move, malformed/newer persistence, no-overwrite creation and missing
mounts/targets. The completed goal is the whole scope above, not only Home polish.
