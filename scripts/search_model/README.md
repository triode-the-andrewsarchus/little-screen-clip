# Local embedding runtime

This helper embeds prepared text, PNG images and PCM16 audio windows with EmbeddingGemma 2. It has no crawler, search database or source-path access protocol. The Rust search coordinator owns enrollment, guarded source reads, extraction, segmentation, index publication and lifecycle/timeouts.

`setup.py` explicitly downloads the two pinned Q8_0 GGUF files and verifies SHA-256 before installation. It builds llama.cpp b11461 at commit `4d756bc72bf00a4aacf410ae15a2d315f3db400d`, using the embedding-logits allocation patch. CPU is always available; CUDA and Vulkan are built when their development tools are available. The C shim is adapted from Recall commit `139960b8728a6f012e957473d418ee20a2476402`; its MIT notice is in `LICENSE.recall`. Upstream llama.cpp is MIT licensed. The build retains shim source, its license and the patch beside binary artifacts.

```sh
python scripts/search_model/setup.py \
  --runtime "$HOME/.local/share/little-screen-clip/search/runtime" \
  --models "$HOME/.local/share/little-screen-clip/search/models"
```

An explicit `--llama-prefix` can reuse a developer's reviewed, already-built patched b11461 prefix. Normal setup builds from pinned source. This option is never discovered or selected implicitly.

`worker.py --runtime PATH --models PATH` loads local files only, emits one readiness JSON object, then accepts JSON lines. `--device` selects `auto`, `cpu`, `cuda`, or `vulkan`; readiness reports the actual device. `--text-only` omits media encoder loading. Only one request runs at a time. A worker process owns one model; its parent must kill and reap it on timeout, cancellation or exit, and release it after the configured idle interval.

The Rust coordinator tries CUDA, then Vulkan, then CPU, with a fresh process for each failed load. An explicit GPU request must report that backend; silent CPU substitution is rejected. Direct worker `auto` selects the first available device in that order; it does not retry a failed model load itself. Index status shows the actual selected device. Backend/toolchain changes rebuild the runtime cache; backend selection alone does not change embedding pipeline identity. Setup takes an exclusive installation lock and workers hold shared locks for their lifetime. Replacement libraries are staged before installation, with rollback on failure, so setup cannot overwrite mapped libraries in a running worker.

- `passage`: `text` contains up to 8 KiB of exact extracted UTF-8 text. Returns checked byte offsets `end` and `next`, plus token counts, for one passage of at most 768 framed model tokens and up to 128 overlap tokens. Paragraph/sentence boundaries are preferred; oversized paragraphs are split. The caller continues from `next`, except at the end of the source. This performs tokenization only, without an inference pass.
- `text`: `inputs` is one to four already framed strings. `token_limit` is 512 (queries) or 2048 (documents). Overlong input is rejected, never truncated.
- `image`: `data` is base64 prepared PNG bytes. `budget` is 140 or 280 vision tokens. Images are limited to 4,194,304 pixels and 4096 pixels per side.
- `audio`: `data` is base64 prepared nonempty mono 16 kHz PCM16 WAV bytes, at most 30 seconds. The coordinator must submit longer recordings as explicitly tracked windows.
- `quit`: acknowledges and releases the model.

The Rust reader prepares video as sparse PNG frames (140 tokens) plus the selected audio track under the ordinary window policy. Visual sampling uses five-second interval midpoints, capped at 1,024 uniformly distributed samples for long clips. Decoded timestamps and sampled/partial coverage stay in SQLite, outside model inputs. Frame preparation uses guarded FFmpeg containers with network/playlist demuxing excluded; the model worker never receives a video source path. Image/PDF inputs retain 280 tokens, and vector reuse keys distinguish the two vision budgets. Completed segments survive a later decode/inference failure only after the coordinator revalidates the original source observation; the status records the completed count and failure instead of claiming exhaustive coverage.

Requests carry an opaque `id`; replies echo it. Successful embeddings contain `dimension: 768`, row `count`, and base64 little-endian float32 `vectors`. Outputs are checked for finiteness/nonzero norm and normalized. This wire precision is independent of persistent float16 vector storage. Individual requests are capped at 16 MiB and media payloads at 8 MiB. Text, source content and vectors are not logged.

Failed replies distinguish `kind: "backend"` from ordinary request errors. The
shared `inference_errors.py` adapter recognizes fixed failure statuses/templates
from the pinned native shims; it never classifies user/model text as an OOM.
Keep that module beside the worker and answer helpers when bundling them.
The Rust owner kills/reaps a failed helper before replaying the same prepared
request on the next backend (CUDA, Vulkan, CPU), at most once per backend and
within the original request deadline. Unexpected helper exit also permits this
bounded recovery; validation/protocol errors, cancellation and timeouts do not.
Recovery rejects a changed model pipeline and retains a successful backend for
later requests. The Index menu shows the selected backend and fallback history.

Run a real model smoke check without enrolling or indexing any folder:

```sh
python scripts/search_model/smoke.py --runtime PATH --models PATH
```

It validates text/image/audio outputs, normalization, a simple relevant-versus-unrelated text comparison and oversized-query rejection. This is a runtime compatibility check, not a retrieval-quality benchmark.

## Generic answer-worker development

The same runtime also builds `libclip-answer.so` for bounded generative text and still-image answers. Explicit setup `--answers qwen3.5-9b` adds the default pinned Qwen3.5-9B Q8_0 weights (about 9.8 GB), plus its pinned f16 vision projection (918 MB). `--answers qwen3.5` and `--answers nuextract3` retain the smaller base/fine-tuned 4B development comparisons. Exact revisions, sizes and SHA-256 checksums live in `answer_models.json`. Normal embedding setup does not download any answering model. The model roles are separate, and normal use must choose one answering model rather than keeping multiple answering models resident.

`worker.py --runtime PATH --models PATH --role answer --answer-provider qwen3.5-9b` uses the same backend selection and lifetime locks. It accepts `answer` requests with `question`, `type` (`text`, `number`, `date`, `boolean`), `complete` (boolean extraction coverage), and `sources` (one to 64 `{id, text}` objects). Prepared question/content is capped at 32 KiB and the model context at 8,192 tokens including generation. Over-budget generation returns a typed unknown result (or a limited evidence selection) without accepting partial output or retiring a healthy model; malformed requests and decoder failures still fail explicitly. The worker returns structured applicability, answer state, string value/unit, cited source quotations, and a reason. Numbers remain exact strings; dates and booleans are validated independently. Non-applicability requires evidence; incomplete coverage cannot establish absence. Text supplied by the user or document is tokenized without recognizing special chat-role tokens. NuExtract3 uses its reviewed structured-task template.

The Rust coordinator connects this text worker to temporary `q` columns, rechecks current source identity after answering, validates typed results/evidence, and shares one owned model process across embedding/answering roles. Switching roles replaces the loaded weights. Column results stay in window memory and do not enter SQLite or source-file metadata. For larger text documents the coordinator collects up to 512 KiB across 512 extracted sources, selects exact quotations in overlapping 8 KiB windows, and reconciles them with the same answer model. Limits are 64 windows, 64 retained quotations, 24 KiB retained evidence and 180 seconds of inference. Saturated or incomplete coverage abstains; selected evidence cannot prove whole-file absence, exhaustive totals or non-applicability. Still-image answers use a lazy-loaded projection and the same 9B model, with bounded prepared PNG input; short/scanned PDF pages and mixed text/image PDFs use bounded evidence selection and shared reconciliation. Run synthetic real-model checks with `answer_smoke.py --runtime PATH --models PATH --provider qwen3.5-9b --device cuda --report /tmp/answer-report.json`. Its fixtures deliberately include boolean false versus non-applicability, missing/ambiguous fields, decimal/date values, generic interpretation and embedded instructions. These checks do not establish general accuracy or visual-document quality.

The expanded 27-case synthetic CUDA check selects 9B over the initial 4B default: it correctly distinguishes exact text absence, omitted real-world facts, negation and extraction coverage. The prepared user payload puts source data first and application-owned coverage last; only `sources[*].text` is file content. Prompt version `generic-qa-v5` and its source hash invalidate ephemeral answer caches. Use `answer_smoke.py --case NAME` repeatedly to reproduce selected failures. Passing these fixtures does not establish general correctness or vision/multipage quality. The 9B model uses the existing answer slot; embedding provisioning and default empty enrollment are unchanged.

## Answer runtime speed probe

Keep the default Qwen3.5-9B Q8_0 weights. To measure actual CUDA prompt processing, token generation, complete request latency and sampled GPU memory:

```sh
python3 scripts/search_model/answer_runtime_probe.py --runtime PATH --models PATH \
  --output-dir NEW_DIRECTORY --samples 5
```

This reuses the production prompts, grammar, context and synthetic answer checks. It requires matching installed/checkout answer shims and builds a private shim with native performance counters and sampler CPU timing; it does not replace installed libraries or weights. The output retains the exact instrumented source, compiler log, per-request metrics, correctness results, resource samples and hashes. Five repeats report medians, not p95. Use repeated `--case NAME` to narrow the correctness cases during diagnosis. Model-load timing uses existing warm model pages and must not be described as disk-cold. Native token rates exclude host sampling, protocol and validation costs; inspect complete request latency too.

The shared text/vision generator follows the pinned upstream common sampler's grammar-rejection approach: validate the preferred greedy token first, then apply the full grammar-before-greedy fallback if invalid. Every emitted token still passes the grammar, with no top-k approximation. The baseline full-vocabulary grammar filter caused substantial host overhead on the 3090; frozen comparisons retained identical complete responses across 37 text requests, and visual checks also passed. This changes sampling work, not the Q8 weights or GPU inference precision.

## Native search latency diagnostics

Set `LITTLE_CLIP_SEARCH_TIMINGS=1` when launching Files to log request IDs, completion/success flags, elapsed milliseconds and accepted entry counts to stderr. This opt-in enables only the dedicated timing target, independently of `RUST_LOG`; it does not log queries, paths or answers. Elapsed time runs from submission through coordinator work and UI result acceptance/sorting, excluding GPU frame presentation. Intermediate cold-load lexical results and final results are recorded separately.

Run `python3 scripts/file_manager_quality_acceptance.py --output-dir NEW_DIRECTORY --appearance dark --timing-samples 21` with the private Weston/Niri dependencies and provisioned models. It indexes the owned 60-file frozen mixed-media fixture and retains `search-latency.json` with individual warm samples, median and nearest-rank p95. Fewer than 20 samples omit p95. This small reproducible workload measures the actual model, coordinator, source verification and UI acceptance; it does not establish large-index latency or representative retrieval quality.

## Retrieval component benchmark

From the repository root, build the library tests in release mode, then run:

```sh
cargo test --release -p clip-workbench --lib --no-run
python3 scripts/search_retrieval_benchmark.py --files 100000 --samples 21 \
  --report /tmp/clip-retrieval-report.json
```

This creates owned temporary files and a private SQLite index using synthetic text-modality vectors, exercises production retrieval, and removes its fixture. It needs no model and enrolls no user folder. The JSON records actual warm timings, planner output and source/binary fingerprints. First-query timing is not disk-cold. Target results are reported independently from test correctness; a successful process exit does not mean the latency targets passed. Model inference, live source verification, explanations and UI publication require separate end-to-end measurements.

For controlled pager experiments, `--sqlite-cache-mib 128` changes only the owned benchmark index; omit it to use the production setting. The report records the actual SQLite cache setting in KiB. Run comparisons sequentially with the same executable and workload. A larger pager can compete with the separate bounded vector cache and needs measured benefit before changing application defaults.

Use `--files 125000` for 187,500 vectors, exceeding the normalized-vector cache capacity. That workload checks partial cache population, the 512 MiB reservation bound, full progress counts and an explicitly uncached late winner. The runner permits up to 200,000 files, but evaluates the 100/500 ms acceptance targets only for the original 100,000-file / 150,000-vector workload. A final high-scoring file is deliberately included so cache truncation cannot hide behind unchanged result counts.

For a separate end-to-end scale probe, build both release artifacts and run:

```sh
cargo build --release -p clip-workbench --bin clip-file-manager
cargo test --release -p clip-workbench --lib --no-run
python3 scripts/file_manager_scale_acceptance.py --output-dir NEW_DIRECTORY \
  --appearance dark --files 100000 --samples 21
```

This reuses the Rust fixture, canonical schema and actual source metadata, exporting a temporary index for the private Files runner. Only its owned folder below `~/Documents/type_testing` is enrolled in the private config. The application performs its ordinary startup metadata reconciliation before measurements; the driver never bypasses membership verification. The runner removes that source/index workspace after the private app exits, including failures. It loads the actual installed embedding model, checks current sources and accepts/sorts results through the ordinary UI. All query cases use hybrid retrieval with a broad text floor, so these are not isolated lexical latency measurements. Stored vectors remain synthetic; the report cannot establish relevance or embedding quality. First requests and individual warm samples are retained separately; model loading during startup reconciliation is outside those samples. Start with `--files 100 --samples 1` when validating the runner; that does not prove the large-index target.

## Runtime resource probes

```sh
python3 scripts/search_model/runtime_probe.py --runtime PATH --models Q8_MODEL_FOLDER \
  --device cuda --samples 21 --cold-model-pages \
  --output-dir /path/on/local-disk/probe-cuda-media-new
```

Use `--text-only` for text-only loading, or `--device vulkan` / `--device cpu` for explicit fallback probes. These commands never enroll folders, install models or change runtime settings. Responses must be finite normalized 768-dimensional vectors. Inputs include short and near-limit queries, a four-item near-context batch, and (in media mode) a 1024-pixel image plus a 30-second audio window. Warm timings include JSON/helper IPC and validation under concurrent telemetry. Fewer than 20 warm samples report median/range without p95.

Reports retain PID-scoped NVIDIA graphics/compute memory, kernel RSS high-water, steady 2 GiB embedding-GPU checks, sampled GPU peaks, first vectors, raw observations and source/runtime/model hashes. Unknown or unlisted GPU memory stays unknown. Raw observations are bounded; peak aggregates survive eviction and truncation is disclosed. Helpers are killed/reaped and private model copies are removed before success is reported. The optional spawn callback belongs only to the evaluation wrapper; the application worker protocol is unchanged.

`--cold-model-pages` copies verified Q8 files privately under the output folder, fsyncs, advises away only those file pages and verifies residency with mincore before launch. Put output on a real local filesystem: tmpfs pages cannot establish this gate. A true flag proves model-file page-cache cold, not cold hardware/controller/system caches. Installed weights and global page caches are untouched. The six 3090 probes documented in the implementation ledger all passed that gate on Btrfs; observed media GPU peaks were 1,526 MiB CUDA and 1,389 MiB Vulkan, below the steady 2 GiB target. These figures concern the embedding helper, not the separate 9B answer model. Numerical backend agreement alone is not a retrieval-quality gate.

## Synthetic semantic-quality component

`quality.py` creates an owned fixture directory and uses the actual disposable Files reader plus the embedding worker. It never enrolls the folder or opens a user index:

```sh
python3 scripts/search_model/quality.py --runtime PATH --models PATH \
  --binary target/debug/clip-file-manager --device cuda \
  --output-dir /tmp/clip-semantic-quality-new
```

Use a new output directory for each run. It retains sources, the frozen manifest, vectors, model diagnostics and a report for review. The fixed corpus has 12 positive queries each for text, PDFs, images, spoken audio and video; 20 negative queries; a short-query/negative stress set; a brief relevant passage in a longer text, a relevant sixteenth PDF page, and a late event in a 65-second clip. Filenames are opaque. Simple drawings and synthetic speech are deliberately reproducible; they are not a representative real-world accuracy benchmark.

This component ranks maximum segment scores per file. It does **not** run production SQLite/lexical/RRF fusion, source-result publication or UI behavior. Float16 packing/normalization follows the storage conversion, but the evaluator uses common float64 dot accumulation for both storage comparisons, rather than reproducing Rust's float32 accumulation bit for bit. It does not compare Q8 model weights against BF16. Exact filename fixtures are listed but this component does not establish their ranking tier. All those acceptance claims remain explicitly false in its reports.

Candidate per-modality cutoffs are midpoint estimates from the frozen calibration subset, evaluated separately against the original hold-out and the additional stress set. The original hold-out was inspected during exploratory development; it is not represented as an untouched final test set. Reports retain failures and never modify application preferences or model files. Inspect no-match behavior and short-query misses before adopting defaults; positive Recall@10 alone is insufficient.

For the production storage/ranking stage, build the library tests and use the frozen output directory above:

```sh
cargo test -p clip-workbench --lib --no-run
python3 scripts/search_quality_ranking.py --folder /tmp/clip-semantic-quality-new \
  --report /tmp/clip-quality-ranking.json --require-targets
```

This uses a verified test harness, an owned temporary SQLite index, the real f16 encoder/scorer, hard filters, filename/FTS matching and rank fusion. It also checks the ten exact filename queries. It does not modify an application index or enroll a persistent user root. Reports separately retain old 0.25 floors, current implicit defaults, and candidate overrides. `--require-targets` checks the current defaults and candidate profile, including negative queries with *any* result so unrelated keyword hits cannot be hidden by a semantic-only metric.

The production runner also evaluates a matched float32 storage reference. A `cfg(test)` cache loader seeds original model outputs after float32 encode/decode normalization; SQLite eligibility, the existing Rust float32 scorer, cutoffs and fusion stay identical. The canonical database remains f16. Reference IDs cover every segment, so the comparison cannot silently mix reference and half-decoded vectors. Reports include both query sets, exact-name checks, changed top-ten file orders, expected-file ranks, matched returned-segment score differences and input/driver/binary/source hashes. `--require-targets` additionally requires at most one percentage point of Recall@10 loss from f16 storage and the reference exact-name tier. Rebuild current library tests: an older harness without the reference fails that gate explicitly. This isolates storage precision for the supplied frozen outputs; it does not compare Q8 weights against BF16 or establish general real-world quality.

The frozen production check exposed broad OR fallback overwhelming good cross-modal matches. Embedding descriptions now use AND keyword matching; OR fallback remains for short keyword-style payloads of at most three tokens. Provisional default floors are text 0.72, image 0.68 and audio 0.64, shared by retrieval, preferences and persistence. Explicit user values are preserved. These are empirical cutoffs, not probabilities, and do not require vector regeneration.

`file_manager_quality_acceptance.py` adds private native UI coverage: it starts with no enrolled index, explicitly indexes all 60 generated files through the actual coordinator, and exercises image, spoken-audio, late-video, short-keyword, negative and exact-filename queries. It retains screenshots/reports. Neither that subset of UI queries nor the synthetic corpus establishes BF16 equivalence, broad real-world accuracy or complete Phase 6 acceptance.

`../search_model_precision.py` runs a matched Q8/BF16 experiment through those same readers, workers and production ranking tests:

```sh
python3 scripts/search_model_precision.py --runtime PATH --models Q8_MODEL_FOLDER \
  --bf16-models REFERENCE_CACHE --binary target/debug/clip-file-manager \
  --test-binary target/release/deps/CURRENT_LIBRARY_TEST_EXECUTABLE \
  --device cuda --output-dir /tmp/clip-model-precision-new --download-reference
```

Build the current release library tests first. The optional `--download-reference` explicitly downloads both checksum-pinned BF16 files from the same official model revision into a separate cache; omit it for a completely local run. Each variant generates its own owned synthetic sources. Every successful model request, including prepared media, text passages and queries, receives an ordered canonical hash. A mismatch in requests or device prevents a model-precision claim. The shared worker's fixed basenames are mapped to verified real files only inside a temporary private alias directory, cleaned after that variant; reports retain actual filenames, sizes and hashes. Application Q8 files, settings and roots are untouched.

The primary comparison uses matched float32 working representations to isolate model-weight precision; canonical f16 comparisons and both storage gates are also reported. Both runs use unchanged application-default floors; exploratory candidate calibration does not alter these comparisons. The initial gate allows at most five percentage points of Q8 Recall@10 loss, with Q8 product relevance, exact-name and storage checks. The frozen CUDA/3090 experiment passes with 242 identical model requests: both have 72/72 positive Recall@10; Q8 returns results for 2/32 negative queries versus BF16's 1/32. Three top-ten lists differ, with no expected-file rank changes. This supports the Q8 choice for these fixtures, not broad quality equivalence or a speed/VRAM claim. Larger representative data and operational measurements remain separate work.

`answer_evidence_smoke.py` checks real-model evidence selection/reconciliation, and `file_manager_large_answer_acceptance.py` exercises a native session column with an answer beyond the former 24 KiB limit. These synthetic checks do not establish general long-document accuracy.

`visual_answer` requests carry a question, requested type and base64 prepared PNG bytes, never a source pathname. The helper caps encoded input at 8 MiB, dimensions at 2048 per side, vision output at 1024 image tokens and total context/generation at the same text limits. The Rust reader currently prepares a maximum 1024-pixel image. The projection loads only for the first image request and is released with the existing answer slot. Python uses explicit bundled-helper paths under isolated mode (`-I`), without adding import search paths or runtime packages. The native decoder consumes the PNG buffer directly.

Visual evidence uses `{source:0, region:[left,top,right,bottom], observation:string}` with coordinates from 0 to 1000. Application code validates regions, states and answer types; the cell tooltip labels the result as a **model observation**. Region/schema checks do not independently verify the depicted fact or establish OCR accuracy. This differs from text quotations, which must occur verbatim in their source. Use `vision_answer_smoke.py` for synthetic model checks and `file_manager_visual_answer_acceptance.py` for private native columns. These paths do not enable audio/video answering or automatic OCR/indexing. Existing installations can continue text answering without the projection; explicit Install answer model adds it. An older runtime reports the need to update rather than claiming image support.

PDF Q/A reuses the source reader's page classification: pages with fewer than 200 trimmed extracted characters include a prepared image; denser pages use extracted text. A single visual page uses direct image answering. For multiple pages, `visual_evidence` selects region observations, ordinary text pages use quotation selection, and one shared reducer reconciles the combined candidates. Already selected visual observations are not selected a second time. Source-page locations and observational provenance survive reconciliation, including ambiguous answers.

Mixed-document collection admits at most 512 pages/sources, 512 KiB text, 16 prepared images and 16 MiB of encoded image data. A 90-second admission deadline is checked between source-reader calls; each active helper retains its own bounded lifetime. Selection/reconciliation share a 180-second inference deadline checked between calls (model startup has its existing separate bound). Exhausted coverage/evidence limits abstain. Dense text pages are **not** automatically rendered for questions about unextracted diagrams or layout; this remains a limitation, not full visual coverage of every PDF. Large-document absence, exhaustive aggregation and non-applicability are not established from selected evidence. `file_manager_pdf_answer_acceptance.py` covers short, scanned, conflicting-page and mixed PDFs in the private compositor without enrollment or an index.

`../search_backend_quality.py` checks whether a Q8 index can be reused after switching inference backends:

```sh
python3 scripts/search_backend_quality.py --runtime PATH --models Q8_MODEL_FOLDER \
  --binary target/debug/clip-file-manager \
  --test-binary target/debug/deps/CURRENT_LIBRARY_TEST_EXECUTABLE \
  --output-dir /tmp/clip-backend-quality-new
```

It explicitly runs CUDA, Vulkan and CPU in sequence, verifies the selected backend and pinned model bytes, and evaluates all nine index/query backend combinations through the existing production ranking harness. A missing backend fails visibly. All comparisons require identical ordered request hashes, model/runtime pipeline, frozen cases and segment identities. Every pair retains a separate ordinary fixture directory and temporary SQLite index; application roots, settings and installed models are untouched. There are no downloads. Comparisons use unchanged product floors, require the product relevance/negative-query targets and exact-name tiers, and report any recall/ranking changes against CUDA/CUDA with both canonical f16 and matched f32 representations. The backend gate permits no positive Recall@10 loss on this fixed corpus; it does not claim general equivalence, end-to-end performance or complete Phase 6 acceptance. Raw per-pair inputs, output ranks and source/binary hashes remain in the output directory.

For a copied/installed desktop executable, keep this complete helper directory as `search_model/` beside the executable, or under `../share/little-screen-clip/search_model/`. Include the Python helpers, model manifest, native C sources, patch, `MODEL_NOTICES.md`, and all `LICENSE.*` files. Discovery is relative to the executable, with the checkout path retained only as a development fallback; launching from another working directory is supported. This describes the search-helper layout, not an installer for the application's other optional helpers. Models and compiled inference libraries remain explicitly provisioned in the per-user search data directory; distributing these scripts does not enroll roots or download weights. A private native acceptance run using an executable-adjacent copied helper tree verifies the selected worker pathname and mixed-media retrieval without using the checkout's model worker.


### Restricted metadata extraction

Explicit Q/A columns first try a session-only `frontmatter_field` plan for complete YAML/TOML headers. The same Q8 answer model maps the question to a field path using only schema labels/formats. The mapping call receives no metadata values. New mappings are then checked against actual header content: an unanchored source answer is typed/evidence-validated and compared in Rust, followed by scoped semantic review. Only an approved literal rule can bypass future model reviews. Value-dependent judgments stay per-file; mismatches and missing context escalate to the existing guarded document answerer. A native byte-only interpreter reparses each source under a default-deny seccomp policy, with no file opens, execution, network, ambient descriptors or writable-file mappings. The ordinary guarded source reader and final identity verification remain authoritative. Unsupported layouts, invalid values, refusals and isolation failures retain generic document answering; they do not authorize shell commands or unisolated tool execution. This is the first extraction operation, not a general regex/table tool suite.

Model-only planner checks:

```sh
python3 scripts/search_model/plan_smoke.py --runtime PATH --models PATH --output-dir /tmp/clip-plan-new
```

A read-only component probe for a deliberately chosen folder (no index/enrollment):

```sh
python3 scripts/search_model/frontmatter_probe.py --runtime PATH --models PATH \
  --binary target/debug/clip-file-manager --source /path/to/posts \
  --question 'What is the publication date?' --type date --expected-field date \
  --limit 8 --output-dir /tmp/clip-fields-new
```

The operator supplies the expected field path; the probe checks exact field values or abstention when that field is absent, plan reuse and current source verification. It is not an independent semantic accuracy benchmark. The report contains selected metadata values; choose its output location accordingly. `file_manager_frontmatter_acceptance.py` covers actual private native columns, sorting, folder return, fallback and no source writes/implicit indexing. `LITTLE_CLIP_SEARCH_TIMINGS=1` enables content-free plan-hit and worker-completion timings; these exclude UI presentation.


### Candidate review and escalation

`extraction_review` is a bounded semantic decision, not another executable tool. The caller first obtains a source-grounded answer without revealing its candidate; mismatched values, units, precision or unsupported/global answers escalate. A subsequent review returns scope (`literal`, `per_file`, `document`), decision and a brief reason. The output grammar and Rust validation reject reuse for a per-file/document scope, and reject upgrading an already per-file plan. The review grammar bounds whitespace as well as its short reason, avoiding stalled generations after a forced string limit. `generic-qa-v6` records the updated content-judgment and prior-review-feedback rules; the model/quant remain unchanged.

```sh
python3 scripts/search_model/review_smoke.py --runtime PATH --models PATH --output-dir /tmp/clip-review-new
```

This composed check includes wrong labels/negation, conflicting tags/content, actual versus stored properties, per-file plausibility, literal metadata, budget handling and embedded instructions. It does not make the same model an independent factual oracle. `../file_manager_answer_review_acceptance.py` tests real native columns: differing judgments for identical layouts, approved literal reuse, a reasonable first age followed by a corrupt age, escalation to corrected body evidence, and no source writes or implicit index. `frontmatter_probe.py` remains a narrower schema-mapping/execution probe; it does **not** exercise the production semantic review or full-document fallback.


### Record selectors and direct-answer recovery

`generic-qa-v9` distinguishes an explicit row/record/item identifier from a
presupposed document type. Finding that entry is the answering task; a missing
entry does not establish non-applicability. Question-only search plans locate
explicit identifiers literally, without assuming where a value/unit appears.
Every match still needs an exact typed answer and per-file sufficiency review.

If a direct full-text response fails worker or Rust validation, the application
tries its existing bounded search/window evidence path under the same 180-second
deadline. It never accepts fuzzy quotations, and cancellation prevents recovery
from starting another call. `file_manager_text_tools_acceptance.py` includes a
240-record regression alongside late-window, table and conflicting-value checks.
The runtime probe groups every applicability/answer generation by request rather
than assuming a single native call; rejected responses remain failed cases in
its report and give a nonzero exit status.
