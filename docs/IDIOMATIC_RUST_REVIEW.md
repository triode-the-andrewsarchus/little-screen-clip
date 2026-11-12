# Idiomatic Rust review

Branch: `refactor/idiomatic-rust-review`, based on `bde2e26`.

## Checklist and approach

Reviewed first-party workspace code against:

- [Cloning to satisfy the borrow checker](https://rust-unofficial.github.io/patterns/anti_patterns/borrow_clone.html): distinguish unnecessary payload copies from deliberate shared ownership.
- [Ownership transitions with `take` / `replace`](https://rust-unofficial.github.io/patterns/idioms/mem-replace.html): move values at their last use instead of cloning them.
- [Type safety](https://rust-lang.github.io/api-guidelines/type-safety.html): encode mutually exclusive states rather than coordinating booleans and optional fields.
- [Flexibility](https://rust-lang.github.io/api-guidelines/flexibility.html): borrow read-only inputs and transfer ownership when required.
- [Dependability](https://rust-lang.github.io/api-guidelines/dependability.html): review fallible cleanup and lock lifetimes.

Searches covered clones, nested options, owned buffer wrappers, borrowed container parameters, drains/collections, lock lifetimes and destructors. A diagnostic Clippy pass additionally enabled `redundant_clone`, `rc_buffer`, `option_option`, `map_unwrap_or`, `flat_map_option`, `significant_drop_tightening` and `needless_collect`.

Diagnostic matches are review leads, not a mandate to rewrite every occurrence. Existing behavior, source safety, cancellation and accepted-write ownership take precedence over stylistic uniformity.

## Changes

- [x] Share immutable diff inputs with background workers. Previously initial text jobs cloned both complete targets and image jobs copied both encoded buffers. Workers now retain the same `Arc<DiffTarget>` snapshots as the viewer, including during recomputation. Image validation and comparison behavior remain unchanged. Validation: diff-view unit tests.
- [x] Consolidate diff computation ownership using core’s existing `Task`. One optional task replaces two optional receivers, a separate ticket and a boolean. Text/image outcomes share terminal handling; replacement and drop request cancellation through the same owner. Tests cover computation, recomputation cancellation, viewer-drop cancellation, image errors and queue rejection. Validation: 53 diff-view tests and strict studio Clippy.
- [x] Avoid a second panic during search-service destruction when dispatch is poisoned. Shutdown recovers the guard only to close admission and signal cancellation; it never resumes potentially inconsistent queued work. A regression test poisons the dispatch and verifies all cancellation signals and stopped admission.
- [x] Move last-use values instead of cloning them across audio analysis, editor handoffs/saves, media controls, mount workers and system topology construction. Palette construction now moves the full harmony set after borrowing its fields instead of cloning the set. No new helper abstractions or dependencies. Production code passes the additional `redundant_clone` lint.

## Intentional patterns retained

- `Arc` clones retain immutable snapshots cheaply. Replacing every `Arc<Vec<T>>` with `Arc<[T]>` can itself copy large buffers and break existing ownership transfers.
- Daemon lock-state/configuration locks and search publication locks serialize updates with invalidation; shortening them mechanically would change ordering guarantees.
- The scene raster cache lock prevents simultaneous full-image decoding, bounding peak memory.
- Lazy gallery embedder initialization distinguishes not attempted, unavailable and available. A nested option here records a real three-state lifecycle.
- Generic string errors at UI boundaries are appropriate; internal typed errors remain useful where callers branch on failure kinds.
- Sound topology retains a bounded vector of parents because it reverses traversal order. The underlying `Ancestors` iterator (and its bounded adapter) is not double-ended; the `needless_collect` suggestion cannot preserve that traversal directly.
- Borrowed `String` parameters found in widget conversion/equality trait implementations match the implemented traits. No general `&Vec`-parameter cleanup is needed.
- Destructors generally cancel work or perform best-effort cleanup. Native-child owners intentionally kill/reap children, and workspace cleanup’s temporary-filename assertion follows construction guarantees.
- Most `map(...).unwrap_or(...)` matches are stylistic alternatives. No blanket rewrite is justified.
- Large rendering functions are existing hotspots. Splitting them solely to lower measured complexity would not simplify their behavior.

Architecture baseline: `idiomatic-rust-review` (341,207 NLOC, 11,222 functions, 4.25% duplication, 16 dependency cycles). Compare changes against this baseline rather than treating existing hotspots as new regressions.

## Architecture review

After consolidating diff work, production and test code together shrink by roughly 80 lines; total measured cyclomatic complexity drops by 8 and clone groups by 2. No crate/dependency changes. The comparison labels existing clone blocks in touched files as regressions (unchanged shortcut handling, image controls and egui test setup); the diff introduces no copies of those blocks. Reported Python cycle additions/removals have the same members in a different enumeration order, in untouched scripts. These are comparison artifacts, not grounds for unrelated rewrites.

Final measured changes (including regression tests): NLOC 341,207 → 341,166; total cyclomatic complexity 45,359 → 45,354; duplication 4.25% → 4.24%; clone groups 1,521 → 1,518. Dependency cycles remain 16. Additional touched-file clone reports cover the same pre-existing topology and mount-job boilerplate; moving values does not add algorithm copies.

## Validation

- `cargo fmt --all --check` and `git diff --check`: passed.
- `cargo test -p clip-studios -j4 diff_view:: --lib`: 53 passed.
- `cargo test -p clip-search -j4 --lib service::tests`: 33 passed.
- `cargo clippy --workspace -j4 --all-targets -- -D warnings`: passed.
- `cargo clippy --workspace -j4 -- -D warnings -D clippy::redundant_clone`: passed. The extra lint is for production screening; test-only fixture copies are not an enforced new workspace policy.
- `cargo test --workspace -j4 -- --test-threads=1`: 2,844 passed, 35 ignored, zero failures; no compiler warnings.
- The initial parallel workspace test run failed two existing file-manager tests: shutdown helper admission was busy, and a text Save As destination was missing. Both pass in the serialized workspace run. Their shared bounded worker budgets make concurrency contention a plausible explanation; this pass does not claim to fix parallel test isolation.

The user’s local `.cargo/config.toml` change is excluded from the commits. Builds/tests explicitly use four Cargo jobs and no additional build trees or cleanup sweeps.
