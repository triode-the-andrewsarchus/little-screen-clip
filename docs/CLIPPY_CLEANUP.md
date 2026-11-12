# Clippy cleanup

The pass checks the default workspace configuration and every target with
`cargo clippy --workspace --all-targets -- -D warnings`.

- [x] Apply and review mechanical suggestions across workspace crates and local vendor patches: conditionals, redundant borrows, precision literals, arithmetic helpers and test-module placement. Replace the halftone radius's approximate square-root constant.
- [x] Review numeric loops, fixed-size decoding and explicit initialization. SIMD lane indexing and generic-associated chunk lengths retain function/loop-local expectations because the suggested iterators/array types do not express those operations correctly on stable Rust.
- [x] Simplify complex state types and review intentional API boundaries. Shared `JobReceiver<T>` distinguishes scheduling failures from domain results without changing cancellation. Named scan signatures replace positional fields; large IPC, viewer and render variants use boxes. Rendering/export/scan boundaries retain function-local argument-count expectations with reasons, and the public named vector API remains compatible alongside standard operators.
- [x] Resolve remaining UI, test and vendor diagnostics. Explicit initialization, test borrow lifetimes, sorting, clamping and private module naming compile cleanly. Strict workspace Clippy passes with `-D warnings`.
- [ ] Run strict workspace Clippy, workspace tests, formatting and architecture comparison.

No workspace-wide lint allowances are added. Existing externally shaped APIs and
index-based numerical algorithms may retain narrowly scoped, explained lint
expectations where changing their shape would harm clarity or compatibility.
