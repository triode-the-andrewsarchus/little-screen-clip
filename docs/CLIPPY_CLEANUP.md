# Clippy cleanup

The pass checks the default workspace configuration and every target with
`cargo clippy --workspace --all-targets -- -D warnings`.

- [x] Apply and review mechanical suggestions across workspace crates and local vendor patches: conditionals, redundant borrows, precision literals, arithmetic helpers and test-module placement. Replace the halftone radius's approximate square-root constant.
- [ ] Review numeric loops, fixed-size decoding and explicit initialization.
- [ ] Simplify complex state types and review intentional API boundaries.
- [ ] Resolve remaining UI, test and vendor diagnostics.
- [ ] Run strict workspace Clippy, workspace tests, formatting and architecture comparison.

No workspace-wide lint allowances are added. Existing externally shaped APIs and
index-based numerical algorithms may retain narrowly scoped, explained lint
expectations where changing their shape would harm clarity or compatibility.
