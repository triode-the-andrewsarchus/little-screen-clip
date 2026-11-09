# Phase 6 architecture review — 2026-11-02

Review source: `fe68e01`, against the retained `phase6-search-start` baseline
(`0084d74`, 2026-10-19). Raw comparison:
`.fitness/evidence/phase6-final-review-20261102/architecture.json`.

The tool reports **REGRESSION**. Keep that classification: absolute clone counts
and its inferred cycle count increase. This baseline spans subsequent main merges
as well as Phase 6 work, so these repository-wide deltas are not a measurement of
search alone. The branch diff against the current main merge base (`49929fa`) is
296 files, 55,137 insertions and 1,810 deletions, including documentation, fixtures,
tests and source-lifetime integration with existing viewers and mutations.

| Metric | Baseline | Current | Delta |
|---|---:|---:|---:|
| Measured source lines | 235,512 | 294,058 | +58,546 |
| Files | 655 | 832 | +177 |
| Functions | 7,393 | 9,381 | +1,988 |
| Average CCN | 4.03 | 4.06 | +0.03 |
| Maximum CCN | 196 | 195 | -1 |
| Duplication | 5.73% | 4.85% | -0.88 percentage points |
| Clone groups | 1,371 | 1,476 | +105 |
| Inferred cycles | 5 | 15 | +10 |

## Review decisions

1. **Complexity:** the average is effectively stable and the maximum decreases;
   this does not excuse individual hotspots. Search findings include
   `Answer::decode` (answer.rs:83, CCN 19), `Retrieval::search`
   (retrieval.rs:318, 21), `lexical` (retrieval.rs:599, 20), `index_file`
   (service.rs:1874, 27), backup `export` (store/maintenance.rs:24, 22), and
   `clean_staging` (store/restore.rs:264, 18). Inspected branches implement distinct
   state/type/evidence checks, scope admission, checkpoint/reuse decisions,
   accepted-write ownership or conservative cleanup. Splitting them merely to
   move CCN would not simplify their contracts. Earlier incremental reviews
   already removed accidental concentration in scheduling, cache ownership and
   the quality runner; their before/after reports remain linked in the ledger.

2. **Duplication:** 580 findings touch changed files; only 19 touch the search
   directory/helper tree. Inspected the retained clone fragments against current
   source. The largest search fragment is 15 lines of test store setup
   (store.rs:1009/1082); other fixtures repeat small input construction and
   assertions. Production fragments include two boundary-specific vector norm
   checks (store.rs:932/964), SQLite error/collection plumbing
   (store.rs:365/407 and store/pruning.rs:33), the mutex/error conversion in two
   lexical admission callbacks (retrieval.rs:238/252), image eligibility counting
   (image_order.rs:294/355), and six lines of accepted pause persistence
   (service.rs:1103/1122). These do not duplicate a search, decoding, mutation or
   persistence mechanism. Vector input encoding and corrupt-storage decoding
   need their separate error/representation contracts. The seven-line score
   signature shared with retrieval/quality.rs also exists in the earlier
   baseline. No clone finding warrants a new general abstraction solely to
   reduce this count. License/text and merged-main changes also contribute to
   the repository-wide result.

3. **Dependencies:** inspected all ten newly reported cycle records. These are
   function-name/call heuristics, not ten new module import cycles. `setup.run`
   calls `subprocess.run`; `quality.read` calls `os.read`; `quality.close` calls
   process/log methods; `runtime_probe.start` calls `thread.start`; the vision
   test's local `generate` calls `visual.generate`; the stem driver's
   `environment` calls `stems.environment`; and package `read` calls a ZIP
   stream's `read`. The worker group conflates string/byte `encode` and token
   counting with request dispatch. `documents.odt_text` and the unrelated
   constellation compactor's `block` intentionally recurse over bounded input
   trees. No reciprocal store/vector-cache dependency remains: the parent Store
   owns the shared epoch/budget. Do not rename symbols to suppress these reports.

4. **Proportionality:** this is a parser/navigation/filter/content feature plus
   a local multimodal index, reconciliation, provisioning/inference, typed
   answering and restricted extraction, with native and adversarial acceptance.
   Much of the integration prevents stale search results from bypassing existing
   preview, studio, drag and mutation checks. The size is substantial and future
   changes should remain localized; it is not evidence that another framework,
   daemon, database or ANN service is needed. None was introduced.

5. **New hotspots:** the functions above remain concrete maintenance risks, as
   do the coordinator's broad state transitions. Their state contracts have
   targeted regressions and bounded native cases; the final review found no
   untested extra mechanism to remove. Keep those contracts together when
   changing behavior rather than layering another scheduler or safety policy.

6. **Existing hotspots:** the tool reports `execute_save` below its former
   complexity threshold, and the repository maximum falls by one. These broad
   results include merged changes and are not credited solely to search.
   Necessary source-guard integration expands some existing viewer entry points;
   they now consume one captured `PreviewContext`/`ReadCheck`, rather than each
   inventing a search-specific source policy.

7. **Reuse:** the reuse search identifies core's `pin_content_directory`
   (kernel_interface.rs:205), actually used by backup/export. Source readers use
   existing `CommandRequest` descriptor/lifecycle handling and ordinary-content
   routing. Studio handoffs use `FileSource`/`ReadCheck`; UI jobs use shared
   bounded executors. Natural filename sorting, content classification, media
   preparation, typed answer validation and the query representation are shared.
   Recall-derived inference is adapted at its narrow runtime boundary, with
   notices/pins, rather than importing its path/store assumptions. The extraction
   interpreter has a deliberately smaller, data-only process boundary and no
   model-generated shell executor. A broad vocabulary reuse query also produced
   irrelevant candidates; those were not used as evidence of actual reuse.

**Disposition:** accept the reviewed necessary growth and retain the quantitative
regression report. The review found no new module cycle, competing store, duplicate
engine or accidental abstraction requiring a final code refactor. This is a
reasoned disposition of specific findings, not a claim that every metric improved
or that technical debt disappeared. Further performance work is deferred by the
user; no metrics-only restructuring is part of acceptance.
