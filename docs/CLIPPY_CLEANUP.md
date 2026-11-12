# Clippy cleanup

The pass checks the default workspace configuration and every target with
`cargo clippy --workspace --all-targets -- -D warnings`.

- [x] Apply and review mechanical suggestions across workspace crates and local vendor patches: conditionals, redundant borrows, precision literals, arithmetic helpers and test-module placement. Replace the halftone radius's approximate square-root constant.
- [x] Review numeric loops, fixed-size decoding and explicit initialization. SIMD lane indexing and generic-associated chunk lengths retain function/loop-local expectations because the suggested iterators/array types do not express those operations correctly on stable Rust.
- [x] Simplify complex state types and review intentional API boundaries. Shared `JobReceiver<T>` distinguishes scheduling failures from domain results without changing cancellation. Named scan signatures replace positional fields; large IPC, viewer and render variants use boxes. Rendering/export/scan boundaries retain function-local argument-count expectations with reasons, and the public named vector API remains compatible alongside standard operators.
- [x] Resolve remaining UI, test and vendor diagnostics. Explicit initialization, test borrow lifetimes, sorting, clamping and private module naming compile cleanly. Strict workspace Clippy passes with `-D warnings`.
- [x] Run strict workspace Clippy, workspace tests, formatting and architecture comparison.

No workspace-wide lint allowances are added. Existing externally shaped APIs and
index-based numerical algorithms may retain narrowly scoped, explained lint
expectations where changing their shape would harm clarity or compatibility.

Final validation:

- Strict workspace Clippy with every target: zero warnings/errors.
- Workspace tests: 2,837 passed, zero failed, 35 ignored across 43 suites; zero compiler warnings.
- Rust formatting and diff whitespace checks pass.
- Core/operations public documentation builds without warnings; rendered receiver and binary-input alias pages checked.
- All 4,155 generated classifier float literals checked; the 276 precision rewrites preserve identical `f32` bits.

There are 60 new local lint expectations with reasons: 53 explicit argument lists
at rendering/export/scan/startup boundaries, two named vector methods retained
alongside their operator traits, four generic-associated chunk-size cases, and
one SIMD lane loop. These are reviewed exceptions, not silently disabled lints;
no crate-wide or workspace-wide suppression was added.

Architecture comparison against `clippy-cleanup`: total measured complexity falls
by 65, average/max complexity remain 4.04/195, and dependency cycles are unchanged.
The tool labels the comparison a regression for a net one additional clone match
(1,521 to 1,522; duplication 4.24% to 4.25%). Review traced the increased matches
to existing test frame setup, existing comparison renderers and existing media
extension checks whose matching boundaries changed after syntax/attribute edits.
No duplicated production implementation was added. The reported film-render
complexity increase of one comes solely from replacing an Option `let … else`
return with `?`; the markup panel similarly retains its original branches after
let-chain formatting. These measurement effects do not justify artificial code
movement or new wrappers.
