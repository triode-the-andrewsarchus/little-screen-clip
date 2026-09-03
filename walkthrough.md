# Walkthrough: Phase 4 — The Big Node Machine & Visual Pipeline Editor

We have designed, implemented, tested, and verified **Phase 4** of **Little Screen Clip**, transforming the system from an ambient clipboard manager into an interactive **visual data workbench**.

---

## 1. Architectural Highlights

### 1.1 External CLI Execution Engine (Zero Scope Creep)
In strict alignment with Section 7.2 of `idea.md`, all transform nodes avoid native Rust re-implementation and leverage standard CLI tools installed on the host system:
- **Image Processing (`ImageMagick`)**: `magick` / `convert` handles padding borders (`-border`), adding bottom title/caption banners (`-splice`, `-annotate`), cropping, resizing, rotating, and format conversion to WebP.
- **Audio Processing (`FFmpeg`)**: `ffmpeg` handles broadcast loudness normalization (`-af loudnorm=I=-14:LRA=11:TP=-1.5`), silence trimming (`silenceremove`), and gain adjustments.
- **Text & JSON Processing (`Python 3` & `jq`)**: `python3` executes slugify, regex search/replace, casing transforms, and custom script snippets; `jq` executes structured JSON filtering.
- **Archive & Checksum**: `zip` and `sha256sum` handle file batch archives and checksum verification.

### 1.2 Interactive Visual Node Canvas in `clip-workbench`
Implemented directly in `egui 0.27`:
- **Dual-Mode Header Tabs**: Instantly switch between `[ 📋 Quick Palette [F2] ]` (1-shot transforms) and `[ ★ Big Node Machine [F2] ]` (the visual graph canvas), toggled with mouse click, `F2`, or `Tab`.
- **Smooth Canvas Controls**: Infinite pan (right-drag or middle-drag) and zoom (scroll wheel).
- **Interactive Node Cards**: Color-coded category banners (Source, Text, Image, Audio, Files, Script, Sink), inline property editors for borders, colors, text, scripts, and real-time output preview pills (`✔ "HELLO WORLD"`).
- **Glowing Bezier Cables**: Interactive pin dragging from output to input sockets, visual connection lines with glowing drop-shadows, and port type validation (`Any`, `Text`, `Image`, `Audio`, `FilePath`, `FileBatch`).
- **Node Catalog Palette**: Pressing `＋ Add Node` opens a searchable catalog with instant search tags (`#image`, `#text`, `#audio`, `#script`, etc.) and category pills.
- **Top Toolbar**: Live recipe dropdown selector, `[💾 Save Recipe]` dialog, `[🗑 Clear Canvas]`, and a prominent purple `[▶ RUN PIPELINE]` button.

### 1.3 YAML Recipe Store & Discovery
- **Storage Location**: `~/.config/little-screen-clip/recipes/*.yaml`.
- **Preloaded Canonical Recipes**:
  1. `pad_and_caption.yaml`: The Section 7.1 blueprint recipe (Left Hand Image + Right Hand Text Caption $\rightarrow$ Frame $\rightarrow$ Bottom Caption $\rightarrow$ Set in Left Hand).
  2. `normalize_audio__14_lufs_.yaml`: Audio Left Hand $\rightarrow$ -14 LUFS Loudness Normalization $\rightarrow$ Set in Left Hand.
  3. `clean_and_slugify.yaml`: Text Left Hand $\rightarrow$ Regex Clean Separators $\rightarrow$ URL-safe Kebab Slug $\rightarrow$ Set in Left Hand.
- **Instant Palette & CLI Integration**:
  - Recipes appear in the Quick Palette search results with `[Recipe]` badges (typing `/` or keywords matches them directly).
  - CLI commands: `clipctl recipe list` and `clipctl recipe run <name>`.

---

## 2. Key Code Changes

| Crate / File | Description |
| :--- | :--- |
| [`Cargo.toml`](file:///home/vega/Documents/git/local/little-screen-clip/Cargo.toml) | Added `serde_yaml = "0.9"` to workspace dependencies |
| [`crates/little-clip-ops/Cargo.toml`](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/Cargo.toml) | Added `serde_yaml`, `serde`, and `uuid` |
| [`crates/little-clip-ops/src/pipeline/types.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/types.rs) | Defined `PortType`, `PortValue`, `NodeKind`, `Pin`, `NodeInstance`, `Connection`, `NodeGraph`, and `Recipe` |
| [`crates/little-clip-ops/src/pipeline/catalog.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/catalog.rs) | Node registry with 25+ templates, input/output pins, default properties, categories, and tagging system |
| [`crates/little-clip-ops/src/pipeline/executor.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/executor.rs) | Topological sort (Kahn's algorithm), DAG cycle detection, and external tool runners (`python3`, `magick`, `ffmpeg`, `jq`) |
| [`crates/little-clip-ops/src/pipeline/recipes.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/recipes.rs) | YAML recipe loading, saving, and auto-initialization of default recipes |
| [`crates/little-clip-core/src/ipc.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-core/src/ipc.rs) | Added `ExecuteRecipe`, `ExecuteGraphYaml`, and `ListRecipes` to IPC requests/responses |
| [`crates/little-clipd/src/main.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clipd/src/main.rs) | Daemon IPC handler for recipe & graph execution, slot updates, and Trofeo screen refresh |
| [`crates/clip-workbench/src/canvas.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/clip-workbench/src/canvas.rs) | Complete interactive egui Node Canvas with Bezier cables, node drag, catalog modal, and toolbar |
| [`crates/clip-workbench/src/main.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/clip-workbench/src/main.rs) | View mode switching (`Palette` vs `NodeMachine`), recipe search execution, and enlarged window |
| [`crates/clipctl/src/main.rs`](file:///home/vega/Documents/git/local/little-screen-clip/crates/clipctl/src/main.rs) | Added `clipctl recipe list` and `clipctl recipe run <name>` subcommands |

---

## 3. Verification & Validation Results

### 3.1 Automated Tests
All 26 automated unit tests across the workspace pass with zero errors:
```
running 9 tests in little_clip_ops:
test pipeline::executor::tests::test_topological_sort_and_cycle_detection ... ok
test pipeline::executor::tests::test_recipe_yaml_roundtrip ... ok
test pipeline::executor::tests::test_python_script_execution ... ok
test pipeline::executor::tests::test_clean_and_slugify_execution ... ok
test tests::test_case_transforms ... ok
test tests::test_clean_url ... ok
test tests::test_concat ... ok
test tests::test_slugify ... ok
test tests::test_trim ... ok
```

### 3.2 CLI Recipe Execution Verification
- **List Discovered Recipes**:
  ```bash
  $ ./target/debug/clipctl recipe list
  === LITTLE SCREEN CLIP RECIPES ===
    ★ Clean and Slugify
    ★ Normalize Audio (14 LUFS)
    ★ Pad and Caption
  ```
- **Text Recipe Execution (`Clean and Slugify`)**:
  ```bash
  $ ./target/debug/clipctl push --hand left "  Phase 4 Visual Node Machine -- Little Screen Clip!  "
  $ ./target/debug/clipctl recipe run "Clean and Slugify"
  Result: Transformed -> phase-4-visual-node-machine-little-scree
  ```
- **Dual-Slot Image & Text Framing Recipe (`Pad and Caption`)**:
  ```bash
  $ ./target/debug/clipctl push --hand left /home/vega/Documents/type_testing/Space.jpg
  $ ./target/debug/clipctl push --hand right "Deep Space Nebula"
  $ ./target/debug/clipctl recipe run "Pad and Caption"
  Result: Transformed -> titled_71aab7bc-909f-4940-995c-50ff64f3e5f7.png
    Subtitle: PNG • 2.2 MB
  ```
  Verified via `clipctl status`:
  - `[ UNDO ] Space.jpg (1920×1920 • 201.1 KB • JPG)`
  - `[LEFT HAND ] titled_71aab7bc...png (PNG • 2.2 MB)`
  - Displaced input correctly shifted into `Undo`, and the Trofeo hardware screen updated immediately.

---

## 4. How to Use the Big Node Machine

1. **Launch the Workbench UI**:
   ```bash
   ./launch.sh ui
   ```
2. **Switch to Visual Node Canvas**:
   - Click **`★ Big Node Machine [F2]`** in the top header (or press `F2` or `Tab`).
3. **Build or Edit Pipelines**:
   - Click **`＋ Add Node`** to open the catalog and search for nodes.
   - Drag an output pin to an input pin to connect them with glowing cables.
   - Select recipes from the **`Recipe:`** dropdown (`Pad and Caption`, `Normalize Audio`, etc.).
   - Click **`💾 Save Recipe`** to name and save custom pipelines to `~/.config/little-screen-clip/recipes/*.yaml`.
4. **Run Pipelines**:
   - Click **`▶ RUN PIPELINE`** to execute the graph, inspect node preview pills, and commit results to `Left Hand` and the Trofeo hardware display!
