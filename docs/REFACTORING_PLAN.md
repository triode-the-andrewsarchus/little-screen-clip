# Workspace & Code Quality Refactoring Plan

This document tracks the step-by-step refactoring plan for the `little-screen-clip` workspace following the repository-wide code review in [`docs/CODE_REVIEW_FINDINGS.md`](file:///home/vcarlson/little-screen-clip/docs/CODE_REVIEW_FINDINGS.md). Each task is executed and verified as an independent commit on `refactor/workspace-and-code-quality`.

---

## Architectural Goals

1. **Remove Lava-Flow Dead Code & Personal Config Leaks**: Delete orphaned modules, unreferenced scripts, unused crate dependencies, and machine-specific path defaults.
2. **Consolidate Duplicated Logic & State**: Unify URL tracking-parameter stripping, PATH binary probing, sysfs numeric suffix parsers, `BinaryDiffViewState` navigation fields, and Office-to-PDF background loading.
3. **Centralize Helper Script Resolution & Guarded Content Readers**: Resolve bundled Python scripts relative to the binary/repo root in `little-clip-core`, and route remaining unguarded `std::fs::read*` / `File::open` calls through `kernel_interface::open_content` and bounded readers.
4. **Eliminate Cross-Crate `#[path]` Compilation**: Move the 6 shared presentation modules (`midi`, `model3d`, `network_preview`, `spreadsheet`, `timestamps`, `units`) to their canonical home in `little-clip-core::presentation` and re-export them in `little-clip-ops`, removing `#[path = "../../../little-clip-ops/src/..."]` and `extern crate self as little_clip_core`.
5. **Untangle `clip-workbench` & `clip-file-manager`**: Stop compiling the entire `clip-workbench` source tree twice (`lib.rs` + `main.rs`) and extract `clip-file-manager` into its own workspace crate `crates/clip-file-manager`.
6. **Decompose Monolithic Modules**: Split oversized multi-thousand-line modules (`constellation/ui.rs`, `search/service.rs`, `image_preview/tools/redact.rs`) into focused submodules and separated test modules.

---

## Task Checklist

- [x] **Task 1: Prune dead code, unused dependencies, personal config defaults, and compiler warnings**
  - Remove unused `little-clip-core` and `tracing-subscriber` dependencies from `crates/little-clip-driver/Cargo.toml`.
  - Delete unreferenced `crates/little-clip-ops/src/stl.rs`, `scripts/segment_anything.py`, and orphaned `scripts/__pycache__/download_raw_samples.cpython-314.pyc`.
  - Remove unused imports in `scripts/ocr.py` and dead `_sink` fields in `crates/clip-workbench/src/audio_preview/mod.rs` and `crates/clip-workbench/src/midi_view/mod.rs`.
  - Clear personal machine paths from `[file_manager.search]` `roots` in `config.toml`.
  - Fix `float_literal_f32_fallback` warnings in `crates/clip-workbench`.

- [x] **Task 2: Deduplicate URL cleaning, command availability checks, and sysfs number parsers**
  - Delegate `op_clean_url` in `crates/little-clip-ops/src/lib.rs` to `auto_actions::clean_url_tracking` so expanded tracking rules and bare-domain handling are shared.
  - Replace the duplicate `which_exists` helper in `crates/little-clip-core/src/document.rs` with `crate::process::command_available`.
  - Consolidate duplicated `numbered` / `digits` / `number` suffix-parsing helpers across `file_manager/system/{btrfs,char_devices,Collections,dma,fabric,firmware,graphics,iommu,leds,media,ptp,scsi,watchdog}.rs` into `file_manager/system/mod.rs`.

- [x] **Task 3: Consolidate `BinaryDiffViewState` navigation state and Office-to-PDF loading**
  - Make `BinaryDiffViewState.nav: BinaryDiffNavigation` the single source of truth in `crates/clip-workbench/src/binary_diff_view/{mod.rs,navigation.rs}`, removing the shadowed `scroll_row`, `cursor_offset`, `selected_region`, `sync_scroll`, and `bytes_per_row` fields.
  - Extract a shared Office-document-to-PDF/Markdown loader helper used by both `workbench/loaders.rs` and `file_manager/viewers.rs`, eliminating the redundant second `render_to_pdf` call when `ensure_rendered_pdf` fails.

- [x] **Task 4: Centralize bundled helper script resolution in `little-clip-core`**
  - Add `little_clip_core::process::resolve_bundled_script(script_name: &str) -> PathBuf` that resolves scripts relative to the executable directory, ancestor repo root, or `CARGO_MANIFEST_DIR` before falling back to relative `scripts/<name>`.
  - Update `image_preview/tools/{bg_remove,depth,inpaint}.rs` and `little-clip-core` config resolvers to use `resolve_bundled_script`, and remove dead `ImageConfig::resolve_*_cmd` methods.

- [x] **Task 5: Route remaining unguarded content readers through `kernel_interface::open_content`**
  - Route `audio/waveform.rs::compute_plain_waveform`, `little-clip-ops/src/{lib.rs,diff.rs,binary_diff.rs,midi.rs,spreadsheet.rs}`, `little-clip-render/src/cards/mod.rs`, `clip-workbench/src/main.rs::ImageLoadSource::ItemFile`, and `file_manager/scene_strip/backend.rs` through `kernel_interface::open_content` / bounded readers so special files (FIFOs, device nodes, pseudo-filesystems) cannot block or enter generic decoders.

- [ ] **Task 6: Eliminate cross-crate `#[path]` compilation between `little-clip-core` and `little-clip-ops`**
  - Move `midi.rs`, `model3d.rs`, `network_preview.rs`, `spreadsheet.rs`, `timestamps.rs`, and `units.rs` into `crates/little-clip-core/src/presentation/`.
  - Remove `extern crate self as little_clip_core;` and `#[path = "../../../little-clip-ops/src/..."]` from `crates/little-clip-core/src/lib.rs`, and keep `little-clip-ops` re-exports intact.

- [ ] **Task 7: Decouple `clip-workbench` binary/library targets and extract `crates/clip-file-manager`**
  - Make `crates/clip-workbench/src/main.rs` a thin binary entrypoint that calls into `clip_workbench` instead of re-declaring `mod` for all studio modules (eliminating double compilation and false-positive dead-code warnings).
  - Extract `file_manager` and `src/bin/clip-file-manager.rs` into a dedicated `crates/clip-file-manager` workspace crate depending on `clip-workbench` and `little-clip-*`.

- [ ] **Task 8: Decompose oversized monolithic modules**
  - Split `file_manager/constellation/ui.rs` (~3,950 lines), `file_manager/search/service.rs` (~4,530 lines), and `image_preview/tools/redact.rs` (~2,270 lines) into focused implementation and test submodules.
