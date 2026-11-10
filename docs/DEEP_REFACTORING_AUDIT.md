# Deep refactoring acceptance audit

**In progress. This is a verification worklist, not a completion declaration.**

The scope is [DEEP_REFACTORING_PLAN.md](DEEP_REFACTORING_PLAN.md), including its
future-roadmap preparation and preservation rules. Optional native-mirror
adoption remains outside the authorized implementation. The implementation
ledger records historical checkpoints; an old “outstanding” sentence is not the
current status, and an old passing log alone does not prove a changed path.

## Requirement and evidence map

| Requirement | Current evidence and acceptance limit |
| --- | --- |
| 0: Baselines, dependency/caller maps, explicit slices | Architecture snapshots and per-slice measurements are recorded in the plan ledger. Current Cargo metadata and final source/diff checks must accompany the final acceptance. Measurements do not imply a clean-build or frame-rate guarantee. |
| 1: Mount discovery and USB isolation | [Mounts](../crates/clip-file-manager/src/file_manager/mounts.rs) and [display owner](../crates/little-clipd/src/display_worker.rs) have bounded/owned asynchronous contracts and fake stalled-source tests. The phase-1 integration checkpoint records the complete confirmed worklist; no physical USB acceptance is required or claimed. |
| 1: Retained UI mutations and clipboard routing | [Desktop IPC](../crates/clip-studios/src/desktop_ipc.rs), [workbench mutations](../crates/clip-workbench/src/workbench/daemon_actions.rs), [route owner](../crates/little-clipd/src/clipboard_route.rs), and their controlled reply/race tests preserve completion and compare-and-cancel identity. Browser operations continue while a studio is visible. |
| 2: One useful presentation/classification pilot | [Presentation inventory](presentation_boundaries.md), core peak-range reuse, guarded ambiguous multisample tests and prepared-raster readiness tests establish real consumers without another registry/cache. The measured peak-query cost increase remains disclosed; no speedup is claimed. |
| 3: Neutral UI and studio ownership | `clip-ui` and `clip-studios` own shared code; neither application owns the other's implementation. Current Cargo edges, actual executable worker-dispatch tests and the ledger's private host runs are the acceptance sources. Broad dependencies still inherited through core remain a measured limitation. |
| 3: Session history, accepted jobs, host independence | [Browser studio host](../crates/clip-file-manager/src/file_manager/viewers.rs), [photo collection](../crates/clip-file-manager/src/file_manager/viewers/gallery.rs), [workbench image host](../crates/clip-workbench/src/workbench/image_sessions.rs) and shared studio tests retain their distinct lifetimes. Image history restoration now has regression coverage for branching, resize controls and crop-drag retirement. |
| 3: Worker discovery, packaging and licenses | Both binaries dispatch enabled internal workers before ordinary arguments. Alternate-directory executable tests and the previous helper-packaging checkpoints remain relevant while those sources are unchanged. Final acceptance must verify that the separate GPL video helper and LGPL native multisample helper remain outside application linkage. |
| 3a: Native mirror prototype | The opt-in prototype, exact raster reference, bounded display data, appearance/source rejection, CPU paint measurements and private captures are recorded in the plan. Raster remains default. The [private input fixture](../scripts/native_mirror_input_acceptance.py) now compares synthetic clicks with observed selection pixels: 20 transitions per mode at 1×/2×, with raw samples, capture costs and executable hashes. These are conservative virtual-compositor response bounds; physical-input/GPU presentation latency remains unknown and no speedup or native adoption is inferred. The plan ledger records results and limits. |
| 4: Headless guarded search | `clip-search` owns backend orchestration/readers with a core dependency and no egui/application dependency. Ranking/schema/source-lifetime and actual helper/model acceptance are recorded at the search checkpoints. Final review must check current source changes against those verified revisions rather than substitute a UI smoke test for backend safety/quality. |
| 4: Selected domain state transitions | Audio viewport/selection calculations, image history restoration, MIDI transport/ranges and their existing UI consumers are the concrete seams. No universal controller or generic diff algorithm is required. MIDI playback/export feedback now has separate owners, retry/stop and idle-expiry tests, live-job progress and a private actual-copy check; the plan ledger records the evidence. |
| 4: Pipeline ownership and completion | [Per-run artifacts](../crates/little-clip-ops/src/pipeline/artifacts.rs), [guarded saves](../crates/little-clip-ops/src/pipeline/save.rs) and [daemon execution](../crates/little-clipd/src/pipeline.rs) retain outputs, partial saves and accepted replies. Media/diff/checksum/hydration inputs use shared guarded admission and staging. [Recursive ZIP](../crates/little-clip-ops/src/pipeline/archive.rs) now enumerates held directories, opens guarded files and validates revisions before no-overwrite publication. Independent ZIP-reader and controlled source-change tests cover names, metadata, links, limits, output conflicts and completed saves. Final source review must retain these contracts. Scripts remain explicit execution, not a newly promised sandbox. |
| 4: Diff and DSP scope | Text, binary and image algorithms remain separate. DSP callers/dependency costs were inspected, an unused dependency removed and remaining tools retained. A separate radio crate/optional product configuration is not justified by current consumers. |
| 5: Optional product changes | Native default adoption and additional presentation redesign were not authorized. Retaining the raster default and recording prototype findings satisfies this conditional branch; no approval is inferred from approval of the structural plan. |
| 6: Feedback, clocks and artifacts | Shared transient-expiry and persistent-error behavior, monotonic feedback clocks, accepted-write ownership and pipeline scratch cleanup have focused tests. MIDI and rolling notice ownership have caller-level verification. Rolling playback, copy and editor intake retain the displayed revision; cancelled editor results and deferred readers own a guarded temporary-file lifetime. The plan ledger records source-swap reproduction, cleanup/range tests and private pointer handoff evidence. No generic `/tmp` or age-only sweep is authorized or enabled by this plan. |
| Integration and stop rules | Final acceptance needs formatting, relevant tests, serial workspace tests, all-target checks, host builds, actual worker/helper checks, appropriate private UI cases and architecture comparison. Existing global Clippy diagnostics must remain distinguished from new ones; the recorded approximate-constant allowance is not a successful `-D warnings` result. |

## Input, document and job owners

These are current ownership facts to preserve, not a new event bus or proposed
multi-document product. Shared state methods can be tested without launching an
application; the hosts still own navigation and close decisions.

| Concern | Owner and transition contract |
| --- | --- |
| Workbench document | One active `ViewerSession`; startup requests retain FIFO order. Accepted image jobs prevent retirement/replacement until their result is handled. A retained image session is distinct from a fresh source load. |
| Browser document | `Viewers` owns one loading/session request. It polls source identity, disables conflicting editor/navigation actions, asks about dirty sessions and defers close for accepted writes. Browser selection/history/scroll live outside the studio. |
| Photo history | A collection retains one `ImagePreviewState` per photo and its independent undo/redo revisions. Host rename/order/ratings and export-all ownership remain outside the shared image editing state. Pixel revisions remain shared `Arc` values. |
| Audio interaction | `AudioViewport` and `AudioSelectionModel` supply calculations; pointer precedence and per-tool scroll edits remain in their existing consumers. Playback clocks remain separate from frame time and monotonic job lifetimes. Accepted save/copy jobs survive playback stop. |
| MIDI interaction | `MidiViewerState` owns the score, solo/range state, synthesized WAV cache and player. The browser has one chosen strip playback and a separate accepted export state. `stop` must not cancel/drop an accepted export or erase its failure result. Retry must retire only the previous playback notice. |
| Multisample input | [Inline focus](../crates/clip-file-manager/src/file_manager/multisample/strip.rs) chooses one instrument source/revision. A click outside its strip disconnects controller input; source changes/retirement clear its viewer. [Performance](../crates/clip-file-manager/src/file_manager/multisample/midi.rs) owns bounded event delivery, voices, sustain and sample-cache state; overflow/hotplug loss releases voices. |
| Competing browser players | [List coordination](../crates/clip-file-manager/src/file_manager/ui/list.rs) stops competing Music/MIDI/multisample playback when a different preview starts. Navigation clears read/playback work through the browser's existing stop/retirement paths. No queue or automatic advancement is introduced. |
| Focus and text editing | The list registers audio/MIDI focus targets each frame. [Application coordination](../crates/clip-file-manager/src/file_manager/app.rs) disables file-reference clipboard-key interception while a studio is active and retains accepted copy polling outside the visible list. Text fields keep normal text clipboard behavior. |
| Mutation and read cancellation | Preview generations/cancellation retire obsolete reads. Validated source/output handles and retained completion own accepted mutations independently of view visibility. Queue rejection precedes execution; a reply deadline must not discard effects already written. |

## Later roadmap phases

The [file-manager roadmap](<File Manager/plan.md>) remains product scope. The
compatibility table in the deep plan covers phases 7–13; these preparations must
not be reinterpreted as implementation of those features.

- Current-folder/view/selection assumptions remain in browser composition, not
  shared studio/UI or headless search APIs. Future columns, virtual Home and
  inline expansion will need browser location design; they must retain existing
  source-root/navigation guards and lossless paths.
- Facts, preview readiness and coverage remain distinct from future annotations,
  pins, persistent marks and rule policy. Rendering must not create sidecars.
- Naming, transfer, editor exports and pipeline saves have different data needs,
  but share ordinary-source admission, held destinations, no-overwrite/atomic
  publication and explicit completion contracts. A future plan/dry run cannot
  authorize execution without revalidation.
- Future remote/resource scheduling may change admission. It must preserve the
  existing worker semantics and accepted results, not infer an SSH destination
  from a path or promise cross-filesystem transactional rollback.

## Work that still blocks final completion

1. Recheck every row and referenced gate against the final source revision,
   complete missing evidence, and only then mark the goal complete. Optional
   feature decisions remain optional; required work is not waived by this list.
