# Phase 6 native UI acceptance

The release source through `f327aef` passes the following matrix on 2026-11-02. Its runtime is unchanged from the validated `577361f` close-routing fix; later changes are test-only. Each run owns private Weston/Niri sockets and input devices. No test sends input to the host compositor.

Durable reports, screenshots, OCR, source/binary hashes, copied helper files and the namespace launcher are under `.fitness/evidence/phase6-final-ui-20261102/`. This table records the exercised interactions; it does not substitute screenshot counts for behavior assertions.

| Area | Native behavior asserted | Dark / light observations | Driver |
|---|---|---:|---|
| Bare navigation | Offscreen reveal, Escape/Enter, substring selection, expiry, text-field focus isolation | 10 / 10 | `file_manager_type_navigation_acceptance.py` |
| Progressive navigation | Automatic subtree/extra locations, typo return to local scope, Escape restoration, explicit scopes never widening, no index creation | 7 / 7 | `file_manager_navigation_acceptance.py` |
| Command controls and safety | Literal colon names, filter/scope edits draft only, invalid range keeps results, displayed UTC bounds, invalid dates, unknown metadata under negation, `/proc`/`sys`/`dev` and owned credential/repository/profile/alias enrollment denial | 35 / 35 | `file_manager_search_controls_acceptance.py` |
| Preferences | Persist pause/residency/always-search, acknowledge reopening, reject protected configuration, no implicit database/models, semantic request without enrollment | 11 / 11 | `file_manager_search_settings_acceptance.py` |
| Image ordering | Selected reference, Hamming distance, header reversal, retained reference after selection changes, source invalidation, unsigned 64-bit dHash, no embedding index/runtime | 7 / 7 | `file_manager_image_order_acceptance.py` |
| Typed answer columns | Numeric/boolean results, both sort directions, preserved selected file URI after header clicks, folder return, session-only columns, no implicit index | 10 / 10 | `file_manager_answer_acceptance.py` |
| Column management | Photos-to-List switch, selectors, duplicate focus, four-column limit, recursive/indexed scope denial, rename/retry/refresh/remove, slot reuse, folder return, source preservation | 26 / 26 | Same driver with `--column-controls` |
| Checkout-independent deployment | Literal/recursive search, explicit safe enrollment, actual semantic retrieval, changed-source rejection, actual generic Q/A, return to embedding role | 14 / 14 | Copied `file_manager_search_acceptance.py` and current executable-adjacent `search_model` tree |

Total: **16 successful runs, 240 observations**. Model runs are serialized. The package tests place the runner, compositors and app in the same Bubblewrap mount namespace, guard that the checkout's Cargo.toml is absent, and use scratch working directories. The source checkout is never renamed/hidden on the host. The current copied Python helpers are hash-recorded; these tests do not assume the older package snapshot is still current.

The first image runs passed their rendered assertions but failed the direct reader check with `KeyError: entries`. The protocol now returns a bounded inventory containing `Directory.directory.entries`. Correcting that test-only lookup makes both complete; the failing runs remain retained as `image-old-protocol-*`.

## Complementary acceptance

The [full audit](phase6_audit.md) links behavior that needs stronger evidence than this matrix alone:

- C5–C10: every grammar example, unit/range boundary, regex limit, birth-time/unknown semantics and timezone/DST cases use parser/filesystem/component assertions. Native command cases exercise their common input and display path; not every numeric endpoint is a separate desktop scenario.
- E1/E3: independent pixel recounts prove red/yellow/green states, with native paused/nested-mount enrollment/removal coverage. The controls above independently repeat protected-root denial and empty-default behavior.
- C14/C16/S13: controlled stale-content races, source-root replacement, passive previews, studios, Reveal, rename/Delete/Trash/drag conflicts and offline opt-in use identity assertions and syscall traces. Current source-routing/verification-age evidence remains separate from ordinary successful clicks.
- A7/A9/A12/A13: typed evidence/applicability, kernel-restricted data tools, answer freshness, bounded text/office/PDF/scanned/mixed evidence and source disambiguation combine model/component checks with the named native cases. Visual observations are not presented as exact text quotation proof.
- A11: rendered out-of-order completion tests cover pending/N/A/unknown states, stable ties and selection while cells arrive; the native URI checks cover the actual coalesced pointer/keyboard focus path.
- R1/S10/S11/S14/S16: held child/write operations, crash/restart checkpoints, fair scheduling, bounded scratch and recovery/backup ownership use their dedicated controlled scenarios. The accepted-write close fix separately passes dark Ctrl+Q and light compositor close.

Search helpers and the exercised shared-studio routes resolve outside the checkout. This does not promise installation or packaging of every optional dependency used by unrelated studios/codecs. Those remain explicit dependencies of their own features. Audio similarity sorting remains outside this Phase 6 objective.
