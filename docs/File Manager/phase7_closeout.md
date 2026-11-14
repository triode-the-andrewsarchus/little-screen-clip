# Phase 7 closeout

Approved objective: finish the six closeout tasks, record the review checkpoint,
merge `feat/file-manager-phase7` into `main`, then begin Phase 8.
Personal `.cargo/config.toml` and `config.toml` changes stay outside commits.

## Delivery ledger

- [x] 1. Prioritize filenames over optional media summaries; verify narrow/wide headers.
- [x] 2. Make Filename order an idempotent ascending natural-name reset; preserve viewport/selection.
- [ ] 3. Verify keyboard/focus, sticky-header and preview continuity interactions.
- [ ] 4. Recheck folder/archive/search and saved-view workflows against the current UI.
- [ ] 5. Update selected acceptance drivers and reconcile historical/current documentation.
- [ ] 6. Pass workspace tests, strict all-target Clippy, owned-source formatting and canonical build.
- [ ] Record requirement-by-requirement review checkpoint and remaining intentional limits.
- [ ] Merge Phase 7 into main, preserving personal configuration changes.
- [ ] Start Phase 8 with its roadmap and a first usable implementation slice.

## Evidence

Task 3 audit found a sidebar handoff bug: bookmark/mount clicks did not claim
keyboard focus, and a release frame could reactivate default list focus. Both
rows now request focus and mouse gestures retain ownership through release.
`cargo test -p clip-file-manager --lib bookmark_and_mount_rows_keep -j4` passes
in both views using mounted-folder fixtures, without invoking mount operations.
The remaining native keyboard matrix is recorded below when complete.

Task 1: `cargo test -p clip-file-manager --lib media_summaries_yield -j4` passes.
The shared header reserves measured filename width before optional summaries at
95/130/260px; this keeps calendar and other custom row names legible.

Task 2: `cargo test -p clip-file-manager --lib filename_reset_is_idempotent -j4`
passes. Repeated resets retain ascending natural names, selected file and viewport
in both views; mixed folders retain type grouping and clear only the family sort.

Record commands, results, source revisions and native reports as each task finishes.
Tests intentionally ignored by their existing harnesses are reported separately.
