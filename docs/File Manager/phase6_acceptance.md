# Phase 6 acceptance — 2026-11-02

Phase 6 is complete for the authorized scope: filename navigation, command and
content search, image hash/reference sorting, semantic file search, and temporary
typed question columns. **Audio sample similarity sorting and its future map are
excluded**, as requested. General clustering, filesystem snapshot/journal
integration, Recall database compatibility and file-owned/xattr embeddings remain
outside this version's agreed design.

This decision covers implementation source through `fe68e01`; the final release
binary SHA-256 is
`1b935b24abdb3b3daf442896c720c7cb6ae13983758ccf65df97f57bc928d8f1`.
The final documentation checkpoint does not alter runtime code. This is technical
acceptance against the scope and recorded gates, not a claim that the user has
performed a separate final usability review.

## Requirement and evidence map

The [67-row completion audit](phase6_audit.md) records individual requirements,
current-source inspections, named tests, native scenarios, limits and evidence.
Its requirements are derived from the user's objective, the complete
[Phase 6 section](plan.md#phase-6--search), and the
[semantic plan](../../semantic_file_search_plan.md). The grouping below is an
index into that audit, not a replacement for the individual obligations.

| Requirement | Audit rows | Authoritative evidence |
|---|---|---|
| Empty default roots; easy red enrollment, yellow unfinished, green ready control | E1 | Default config, `search/ui.rs::index_ui`, enrollment persistence and observed-mount checks; fresh six-case pixel recount on both current packaged themes |
| Protected system/private/generated folders and aliases cannot enroll/read; settings and restores cannot bypass policy | E2–E6 | Pinned routing/policy, legacy/import purge, decoder syscall traces, protected native controls and explicit setup/no-download cases |
| Existing navigation; explicit colon grammar, literal escape, focus and draft handling | C1–C5 | Parser/input tests plus both-theme controls/type-navigation runs |
| Exact size/date/created ranges, negation, classifiers, bounded regex and scopes | C6–C11 | Exact examples and boundary/DST/unknown tests, shared query IR and native filter/scope/error controls |
| Progressive fuzzy/recursive fallback and literal extracted-content search | C12–C14 | Matching/scope/cancellation components; native local/subtree/always-search tests; supported-format extraction and controlled stale-publication race |
| Sort/pagination, evidence and source-safe actions/previews/studios | C15–C16 | Ordering tests and native selection, result actions, captured source/root replacement and syscall-traced studio/preview cases |
| Image dHash and selected-reference Hamming distance | I1–I4 | Unsigned hash parsing/order tests; both-theme reference/header/invalidation runs, without index/models |
| Generic typed answer columns, lifecycle, evidence/applicability, sorting and source invalidation | A1–A13 | Actual Q8 model probes, typed/evidence/scheduler tests, both-theme sorting/column management, bounded PDF/visual/office paths and private source races |
| Read-only model tools with review and escalation | A7–A9 | Restricted data interpreter, pre-parse seccomp denial regressions, reviewed reuse/escalation probes and adversarial real-model cases |
| Versioned local SQLite, f16 vectors/f32 exact scoring, supported segment formats and budgets | S1–S6 | Schema/serialization/capacity tests, real guarded readers and explicit media/page/token coverage probes |
| Identity/revision/pipeline reuse, incremental moves/updates, conservative scans and current/offline publication | S7–S13 | Digest/canonical-input tests, overlap/hard-link/replacement/restart/event-loss cases, startup/fairness and source-age/offline native traces |
| Retention/cleanup, migrations, backup/restore and crash recovery | S14–S16 | Pruning/migration/checkpoint tests; actual WAL backup/import, accepted-write close, failed-install rollback and owned scratch cases |
| Owned runtime and helpers, backend fallback, scheduling, idle/retry/cache/resource bounds | R1–R6 | CUDA/Vulkan/CPU runtime probes, native failure recovery/preemption/idle release, held-child lifecycle, 100k-file startup and above-cache cancellation |
| Ranking, quality, quantization/storage, accepted speed | R7–R10 | Frozen production-ranking replays and matching-location gates; matched Q8/BF16 and f16/f32 comparisons; scoped timing reports and explicit user speed acceptance |
| Native theme/input/deployment matrix and final build/test/review | R11–R12 | [UI matrix](phase6_ui_acceptance.md), final logs and [architecture review](phase6_architecture_review.md) |

All 56 `.fitness/evidence` references in the audit resolve, including the two
expanded restore-theme paths. Final logs, source/binary hashes, six color recounts,
the raw architecture result and audit inventory are retained under
`.fitness/evidence/phase6-final-review-20261102/`. Detailed native evidence lives
in the per-row directories; older failing/before probes are intentionally retained
and are not represented as current successes. These ignored local artifacts are
not shipped in Git. Tracked drivers, fixture expectations, reviews and this audit
preserve the verification method; large models/media/screenshots remain local.

## Final checks

| Check | Result |
|---|---|
| `cargo test --workspace -- --test-threads=1` | 3,047 passed, zero failed, 38 ignored across 26 suites |
| Project `nobg-venv/bin/python -m unittest discover -s scripts -p 'test_*.py'` | 26 passed, including eight ranking-gate tests |
| `python3 -m unittest discover -s scripts/search_model -p 'test_*.py'` | 96 passed |
| `cargo build --release -p clip-workbench --bin clip-file-manager` | Passed; same binary as final UI matrix |
| `cargo fmt --all --check` | Passed |
| `cargo clippy -p clip-workbench --lib --tests --no-deps` | Completes with unchanged existing warnings: 589 library, 650 library-test, 264 binary-test |
| Private Weston/Niri final interaction matrix | 16 successful runs / 240 observations, both themes; two actual-model packaged runs with the checkout hidden |
| Architecture comparison and source review | Tool classification retained as REGRESSION; scoped review accepts necessary growth, with hotspots/clone/cycle findings disclosed |

The first broad Python run used system Python and could not import the optional
PyMuPDF dependency for PDF tests. Its failure log is retained; running the complete
suite with the application's already-installed environment passes all 26 tests.
No system packages or dependency versions were changed. The Rust ignored tests
include explicit real-model/large-fixture gates exercised separately in their
named native and quality reports; the workspace total alone does not prove them.
Clippy is not warning-clean, and no `-D warnings` success is claimed.

## Decisions and practical limits

- Index enrollment is always explicit. No folder is indexed by default, and
  navigation/search/setup alone cannot enroll it. Eligible files under the
  requested `type_testing` location are exercised by owned media fixtures;
  acceptance does not require indexing the user's entire personal tree.
- Yellow means enrolled but unfinished, including paused/partial/failed states;
  the tooltip explains which. Green describes ready observed coverage, not
  exhaustive understanding of every format. Private/generated/system policy is
  mandatory even for explicit roots/hidden indexing. Custom secrets in otherwise
  ordinary locations still need exclusions; this is not universal secret detection.
- Content search covers the declared text/code/office/PDF readers with bounded
  extraction and explicit partial/unsupported reasons. Archives other than
  supported office containers, executables, 3D and MIDI remain filename/path-only.
  The `rg-a` comparison describes useful extracted-content search, not identical
  support for every adapter, recursive archive or external-resource expansion.
- Semantic media uses declared passages/pages/audio windows/sampled video frames.
  Events between frames can be missed; limits and unavailable extractors never
  imply exhaustive coverage. Automatic OCR/transcription is deferred; bounded
  visual Q/A is separate from a searchable OCR text index.
- Expanded frozen Q8 retrieval passes 76/80 positives (95%), all modality minima,
  ten exact-name tiers and all four required long-event locations. Four of 32
  negative queries still return results. The older 12-negative stress slice has
  three matches (25%) and fails its diagnostic 20% bound; it remains disclosed.
  The original aggregate gate passes without threshold/label changes. Finite
  development fixtures do not establish general model accuracy or equivalence.
- Q/A uses Qwen3.5-9B Q8_0, sharing one resident role slot with the embedding model.
  Exact evidence/type checks and same-model review reduce errors but cannot
  guarantee truth. Unsupported, ambiguous, partial and non-applicable are distinct;
  visual regions remain model observations. No model-generated shell is executed.
- Current timing is accepted by the user. The historical 100-ms lexical/500-ms
  hybrid aspirations are not falsely reported as universally met. No additional
  optimization is required for this acceptance.
- Freshness guarantees apply to observed identities/mounts/events, with conservative
  invalidation on uncertainty. Invisible NAS/session changes and uninterruptible
  kernel I/O have no universal detection/termination guarantee. Last-known offline
  data is opt-in and visibly unavailable; absent historical check times stay unknown.
- Live SQLite must be on a supported local filesystem, even for remote source
  folders. There is no xattr/source-file mutation or Recall schema compatibility.
  Logical deletion is not secure erasure and does not guarantee immediate physical
  shrinkage. One identified backup partial may remain after abrupt death; retries
  refuse that reserved name rather than accumulating outputs or deleting user data.
- Search-helper deployment is verified outside the checkout. This does not install
  every optional dependency belonging to unrelated studios/codecs.

The [decision ledger](../../differences_from_phase_6_plan.md) preserves the design
changes, failures, measurements and user-authorized exceptions. No other Phase 6
capability is silently deferred by this completion decision.
