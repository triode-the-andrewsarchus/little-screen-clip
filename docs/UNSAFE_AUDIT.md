# Unsafe Rust audit

Scope: tracked Rust application/library crates, local vendor patches, and the
standalone video I/O helper. Test fixtures are inventoried separately. System
calls, native codecs, Wayland lifetime bridging and SIMD need an explicit local
safety argument; a comment alone does not repair an invalid invariant.

- [x] Replace the film blur's overlapping mutable image slices with disjoint parallel row bands; compare against a scalar reference across image/band boundaries and verify cancellation.
- [x] Replace manual GPU uniform `Pod`/`Zeroable` assertions with checked derives and raw-pointer parallel slice splitting with safe splits.
- [x] Correct the scalar filter fallback's sample-width calculation and remove its unchecked access; forced scalar tests cover empty/short/long rows and delayed filtering.
- [x] Correct pixel-row stride validation before conversion; reject source/destination rectangles wider than the declared row, including buffers whose total storage is otherwise large enough.
- [x] Replace avoidable OS calls with existing safe descriptor-based APIs. `rustix` was already locked transitively; direct workspace dependencies now cover flags, polling, metadata/statx/openat2, CPU masks, netlink, resource limits and signals. Inherited raw integers are duplicated in the kernel before constructing owned handles, and their source/output policy is still checked separately. Native SQLite locking uses `File::try_lock`/`unlock`.
- [x] Audit native codec/SQLite/Wayland ownership, buffers and callbacks. WebP dimensions/output slice sizes are checked explicitly; native callbacks and guards document borrowing and unique frees. SQLite keeps the extra allocation margin advised for untrusted snapshots; the current safe stream loader does not provide that margin. Wayland display/proxy ownership stays with eframe.
- [x] Review remaining vendor SIMD, thread guarantees and standalone I/O lifetimes. GPU caches no longer assert Send/Sync for browser handles; native caches remain process-wide. SIMD row lengths and index bounds are documented at each remaining unchecked access. Opus decoder access stays private and exclusive for native mutations.
- [x] Retain uncertain io_uring buffers after fatal drain failure instead of treating ring closure as completion; the three standalone direct/prefetch/error tests pass.
- [x] Verify comments and safety contracts, run strict Clippy/tests and architecture review.
- [x] Reuse the shared before-fork parent identity guard for metadata helpers, preserving bounded worker admission and cleanup; nine focused helper tests pass.

- [x] Include the typed OS dependency in the media helper's independently rebuildable source workspace; both packaging tests pass and check inherited workspace dependencies.

## Remaining boundaries

| Area | Reason to retain unsafe / required invariants |
| --- | --- |
| Process launch and raw inherited descriptors | After-fork callbacks run only async-signal-safe calls. Raw integer inputs are validated by duplication before ownership; typed callers use safe duplication. Parent-death and close-on-exec changes retain child ownership. |
| Atomic rename | The existing raw-descriptor API uses a single Linux no-overwrite syscall with live directory handles and checked C strings. It supplies no source/destination admission of its own. |
| CPU identification | Only the Linux CPUID-enabled flag query remains raw; masks and affinity operations use safe APIs, with pin verification and restoration preserved. |
| Extraction sandbox | The disposable entry point closes ambient descriptors before owning documents or starting threads, then installs a copied, bounded seccomp program. Never call it inside a GUI/coordinator. |
| SQLite | Storage-error inspection borrows the live connection. Snapshot deserialization needs SQLite-owned allocation and an initialized 20-byte margin for untrusted bytes, with a unique ownership transfer. Bulk initialization/header edits now use a bounded Rust slice. See [SQLite's deserialize contract](https://www.sqlite.org/c3ref/deserialize.html). |
| Clipboard and Wayland | Tokio registration owns a stable nonblocking file until unregistration. Foreign display/proxy handles remain borrowed from eframe and are never destroyed/disconnected by our drag integration. |
| Native media | FFmpeg contexts, layouts and frame metadata stay borrowed from their owners; graph thread settings are changed before graph initialization. WebP callbacks use live stationary context and atomics, with synchronous encoding and unique cleanup guards. Opus mutations require exclusive access to private decoder state. |
| Vendor conversion/SIMD | MaybeUninit views preserve layout and typed initialization on return/unwind. Source rows require initialized selected channels; retained unchecked SIMD accesses have explicit nonempty/equal-length/index proofs next to them. |
| Standalone video I/O | Aligned, separately allocated blocks stay stationary while kernel reads are pending. C entry points specify exclusivity, alignment, counts and descriptor lifetime. Uncertain pending blocks are retained rather than freed after fatal draining failure. |

The browser GPU path now uses worker-local caches without overriding thread
traits. A native compile/run harness exercises the actual worker-cache macro with
non-Send handles on eight independent threads, verifying reuse within each thread
and separation between threads. The WebAssembly target itself is not installed;
no full WebAssembly build is claimed.

Lexical inventory compared with audit-start commit `a4a3b56`: 100 explicit unsafe
blocks/functions/implementations removed (236 to 136 across tracked Rust,
including unchanged test sites). Comments and unsafe linkage attributes are not
included in those counts. Test fixtures were left separate from production review.

## Verification

- Full workspace suite: 2,843 passed, zero failed, 35 ignored across 43 suites.
- Follow-up focused tests after the final bounded-buffer/launch refinements:
  database six, media probes five, metadata helpers nine, and WebP four, all pass.
- Standalone video I/O: three direct-read/prefetch/error tests pass. Its separate
  Clippy run passes with warnings, undocumented unsafe blocks and missing safety
  docs denied.
- Production workspace Clippy passes with `-D warnings`,
  `-D clippy::undocumented_unsafe_blocks` and `-D clippy::missing_safety_doc`.
  Standard `--workspace --all-targets -- -D warnings` also passes; test fixtures
  are intentionally outside the additional unsafe-documentation policy.
- Core rustdoc builds with warnings denied; the rendered inherited-handle API's
  ownership contract and Errors section were checked. Formatting, whitespace,
  helper packaging tests and Python compilation pass.
- The optimized workspace build and independently rebuildable media-helper
  source package complete with zero compiler warnings. Builds/checks and cache
  pruning were serialized; the pre-existing local Cargo configuration was untouched.

Architecture review against `unsafe-audit`: total measured complexity decreases
by six, average/max remain 4.04/195, clone groups decrease from 1,522 to 1,521,
duplication remains 4.25%, and dependency cycles remain 16. No new high-complexity
production function is reported. The tool still labels the comparison a
regression for existing clone matches touching changed files, an intentionally
combinatorial reference test, and order-only variations in unchanged Python cycle
records; no new dependency direction or copied production implementation was added.
