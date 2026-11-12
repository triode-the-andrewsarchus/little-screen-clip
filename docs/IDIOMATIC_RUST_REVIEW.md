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
- [ ] Consolidate diff computation ownership using the existing cancellable task type.
- [ ] Remove worthwhile last-use clones and avoid unnecessary intermediate collections.

## Intentional patterns retained

- `Arc` clones retain immutable snapshots cheaply. Replacing every `Arc<Vec<T>>` with `Arc<[T]>` can itself copy large buffers and break existing ownership transfers.
- Daemon lock-state/configuration locks and search publication locks serialize updates with invalidation; shortening them mechanically would change ordering guarantees.
- The scene raster cache lock prevents simultaneous full-image decoding, bounding peak memory.
- Lazy gallery embedder initialization distinguishes not attempted, unavailable and available. A nested option here records a real three-state lifecycle.
- Generic string errors at UI boundaries are appropriate; internal typed errors remain useful where callers branch on failure kinds.
- Most `map(...).unwrap_or(...)` matches are stylistic alternatives. No blanket rewrite is justified.
- Large rendering functions are existing hotspots. Splitting them solely to lower measured complexity would not simplify their behavior.

Architecture baseline: `idiomatic-rust-review` (341,207 NLOC, 11,222 functions, 4.25% duplication, 16 dependency cycles). Compare changes against this baseline rather than treating existing hotspots as new regressions.
