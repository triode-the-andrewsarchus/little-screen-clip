# Phase 8 completion audit

The scope is the full organization phase in `plan.md` plus the drawable personal
Home and live project requirements in `phase8_completion.md`. This audit uses
the implementation on `feat/file-manager-phase8`, not the original static-pin
checkpoint. The delivery ledger retains the individual commits and earlier
failures as well as final evidence.

## Requirements and evidence

| Requirement | Current implementation and verification |
| --- | --- |
| Explicit personal Home, separate from the filesystem home and clipboard | `home.rs`, `home_history.rs`; native history reports below verify no-argument Home, explicit directory startup, Back/Forward, Escape and folder context. Unit tests preserve folder selection/scroll and keep Home visible on a failed return. |
| Free-form native vector desk, drawing/text/shapes, clickable hand-drawn shortcuts | Shared `clip_studios::drawing_canvas` embeds the existing editor, vector renderer and camera. Native desk workflows draw actual pencil ink, bind it, restart and open its real target by clicking that ink. The final review desk does the same with its notebook. No browser engine or alternate raster renderer was introduced. |
| Named sections, arrangement, resize, undo/redo and persistent view | `home/board.rs`, `home/session.rs`, `ui/home/cards.rs`; 33-observation native desk runs exercise move/resize/undo/redo, text draft protection, pan/zoom, restart and non-destructive unpin. Shared rendered tests verify same-frame alignment and live overlay exclusion. |
| Projects in ordinary folders, dynamically registered into Home | `.fmproject` source/registry and live Projects card; native project workflow creates/registers/unregisters real folders, preserves existing documents and requires an explicit copied-ID relocation choice. The review desk shows two separate external project folders. |
| Live TODO checkboxes editing the actual project document | `home/projects/tasks.rs` and bounded preview owner. Native task workflows verify a one-byte checkbox edit, independent layout undo, external edits, failed-write recovery, missing/restored source, Open TODO and unregister without deleting content. The review desk verifies real source bytes, refresh and restart. Task content is absent from Home JSON. |
| Red/yellow/green session marks with editable meanings and recovery | `organization/model.rs`, `ui/organization.rs`; both-theme 32-observation session runs verify labeled underlines, definitions, persistence, clear/restore/forget and disabled-state retention. Unit tests cover concurrency, changed identities and non-UTF-8 paths. |
| Configurable sorting sets and repeated subdivision/filtering | `organization/sets.rs`, `ui/sorting_sets.rs`; both-theme 24-observation workflows verify assignment, child sets, ancestor filters, independent marks, draft preservation and restart. Unit tests reject cycles, excessive hierarchy and invalid memberships. |
| Independent visibility rules and selective dotfile reveal | `visibility/rules.rs`, `ui/visibility.rs`, shared container inventory predicate. Both-theme 10-observation runs verify `.hide`/`.show`, solid labeled backgrounds, inheritance and recovery. Unit tests cover negation/anchoring/precedence, non-UTF-8 names, malformed/future/oversized sources, aliases/replaced directories and ordinary shared-memory mount scope. |
| Versioned readable storage, no-overwrite conversion, bounded work | Guarded shared `local_state`, project source and task readers. Workspace tests pass near-limit v1 migration, malformed/newer preservation, atomic/no-op writes, cooperating-window conflicts, aliases/escaping paths, no-overwrite defaults, bounded hidden preview retirement and accepted-write close ownership. |
| Defined restart/rename/move behavior for pins, marks, sets and projects | `reference_updates.md` specifies path mapping, stable IDs, mark identity limits, collision policy, failures, retries and external moves. Final 26-observation native runs verify file rename, copy versus move, project rename, retry after a real writer-lock failure, refused close, updated shortcuts and restart. Unit tests additionally verify pending layout/undo, cleared-mark recovery and retained definition drafts. |
| Usable in both appearances, no floating property windows | Native screenshots in both themes were inspected. Home uses the existing fixed drawing inspector, a shared camera and host-owned live cards. The review desk's labels, task controls and artwork are readable in both appearances. |

## Native reports

These are private compositor runs with isolated HOME/config/state and actual
native pointer/keyboard input, not mocked application calls.

| Workflow | Dark report | Light report | Observations per appearance |
| --- | --- | --- | --- |
| Drawing and arrangement | `/tmp/phase8-desk-drafts-dark-03/report.json` | `/tmp/phase8-desk-drafts-light/report.json` | 33 |
| Project creation/registry | `/tmp/phase8-project-dark-04/report.json` | `/tmp/phase8-project-light-05/report.json` | 19 |
| Source-backed tasks | `/tmp/phase8-project-tasks-final-dark-02/report.json` | `/tmp/phase8-project-tasks-final-light/report.json` | 20 |
| Session marks | `/tmp/phase8-session-complete-dark/report.json` | `/tmp/phase8-session-complete-light/report.json` | 32 |
| Sorting sets | `/tmp/phase8-sets-dark-06/report.json` | `/tmp/phase8-sets-light-06/report.json` | 24 |
| Visibility | `/tmp/phase8-rules-dark-04/report.json` | `/tmp/phase8-rules-light-04/report.json` | 10 |
| History/startup | `/tmp/phase8-home-history-dark-03/report.json` | `/tmp/phase8-home-history-light-03/report.json` | 12 |
| Reference updates | `/tmp/phase8-relocations-dark-05/report.json` | `/tmp/phase8-relocations-light-05/report.json` | 26 |
| Integrated review desk | `/tmp/phase8-review-dark-03/report.json` | `/tmp/phase8-review-light-03/report.json` | 7 |

The last workflow waits for the actual artwork to appear before capturing the
desk. Earlier review-driver failures were missing OCR punctuation on a leading
dot and a bordered toolbar label; the final run independently checks the real
source and destination bytes and clicks painted ink. Earlier failed reports
remain available and are not counted as passing evidence.

## Review fixture

`/home/vega/Documents/type_testing/phase8_organization/open-review.sh` launches a
separate editable desk with notebook/star shortcuts, notes, two registered
projects and live TODOs. All content lives beside isolated `.home`, `.config`
and `.state` directories. The launcher changes to that directory before starting,
so a caller's working-directory config cannot leak into the review. Normal
personal Home and configuration are untouched. `README.md` describes interactions.
The committed generator is `scripts/file_manager_home_review.py`; it refuses an
existing destination rather than overwriting a customized desk.

## Workspace verification

- `cargo test --workspace -j4 -- --test-threads=1`: 3,372 passed, zero failed,
  36 ignored across 47 test/doc-test results. Includes 1,024 file-manager tests
  and the shared drawing-studio tests. Log: `/tmp/phase8-workspace-tests.log`.
- `cargo clippy --workspace --all-targets -j4 -- -D warnings`: passed, including
  the final command-help correction (`/tmp/phase8-final-clippy.log`).
- `cargo build --workspace -j4`: passed (`/tmp/phase8-workspace-build.log`);
  the final repeat is recorded in `/tmp/phase8-final-build.log`.
- `cargo fmt --check`, `git diff --check`, Python compilation of the changed
  acceptance/review scripts and `sh -n` on the review launcher passed.
- Final help was exercised through `cargo run -p clip-file-manager -- --help`
  (`/tmp/phase8-final-help.log`). Personal Cargo/runtime configuration is excluded
  from commits. Build/test commands and cleanup were serialized; available
  project disk space remained approximately 82 GB. No cache sweep was needed.

## Architecture review

Comparison against `before-phase8-completion`:

| Dimension | Baseline | Current | Delta |
| --- | --- | --- | --- |
| Average cyclomatic complexity | 3.44 | 3.44 | 0 |
| Maximum complexity | 274 | 274 | 0 |
| Duplication percentage | 7.30% | 7.24% | -0.06 points |
| Clone groups | 4,293 | 4,304 | +11 |
| Dependency cycles | 143 | 143 | 0 |
| Functions | 36,042 | 36,391 | +349 |
| Measured code lines | 820,516 | 828,797 | +8,281 |

The aggregate tool verdict is REGRESSION because of the additional clone groups;
this is not represented as a clean architecture score. Review found short
independent test fixtures/assertions, schema/dependency declarations, symmetric
undo/redo transitions and existing domain actor admission/publication scaffolding.
The persistence transport, renderer, camera, path relocation and deferred-intent
batching remain shared. No second project/task parser, browser engine, media
cache or state writer was added. The growth covers the eight organization/Home
deliveries and their regressions, rather than wrappers around a small UI change.

Lizard reports no new excessive-complexity function after the model/delivery
splits; existing drawing keyboard complexity falls from 16 to 13. Valknut still
flags the registry's explicit bounded identity/path validation and the native
driver's OCR fallback selection as complex entities. They preserve necessary
validation/observability branches. Its changed cycle ordering concerns untouched
Python/vendor files; cycle count is unchanged and those sources are unchanged.
The shared input driver naturally becomes a higher-fan-in test dependency.
No further abstraction is justified solely to hide these short patterns.

## Explicit boundaries

Dynamic query-based sections remain Phase 10. The Projects area is a bounded
registry-backed live view, not a general query engine. External moves require
explicit re-registration or leave a retained unavailable reference. Metadata
stores commit individually: normal close retains pending work, but forced process
termination can leave separate stores at different checkpoints. Session marks
are personal annotations, not content identity or permission guarantees. These
contracts are described in `projects.md`, `organization.md`, `visibility_rules.md`
and `reference_updates.md`; they do not hide unfinished Phase 8 deliverables.
