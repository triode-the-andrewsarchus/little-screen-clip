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
- [ ] Verify comments and safety contracts, run strict Clippy/tests and architecture review.

- [x] Include the typed OS dependency in the media helper's independently rebuildable source workspace; both packaging tests pass and check inherited workspace dependencies.
