# Unsafe Rust audit

Scope: tracked Rust application/library crates, local vendor patches, and the
standalone video I/O helper. Test fixtures are inventoried separately. System
calls, native codecs, Wayland lifetime bridging and SIMD need an explicit local
safety argument; a comment alone does not repair an invalid invariant.

- [x] Replace the film blur's overlapping mutable image slices with disjoint parallel row bands; compare against a scalar reference across image/band boundaries and verify cancellation.
- [x] Replace manual GPU uniform `Pod`/`Zeroable` assertions with checked derives and raw-pointer parallel slice splitting with safe splits.
- [x] Correct the scalar filter fallback's sample-width calculation and remove its unchecked access; forced scalar tests cover empty/short/long rows and delayed filtering.
- [x] Correct pixel-row stride validation before conversion; reject source/destination rectangles wider than the declared row, including buffers whose total storage is otherwise large enough.
- [ ] Replace avoidable OS calls with existing safe descriptor-based APIs.
- [ ] Audit native codec/SQLite/Wayland ownership, buffers and callbacks.
- [ ] Review remaining vendor SIMD, thread guarantees and standalone I/O lifetimes.
- [ ] Verify comments and safety contracts, run strict Clippy/tests and architecture review.
