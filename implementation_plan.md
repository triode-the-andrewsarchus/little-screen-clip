# Implementation Plan: Phase 4 — The Big Node Machine & Visual Pipeline Editor

Transition **Little Screen Clip** from an ambient clipboard manager into an interactive **visual data workbench**. Phase 4 introduces a visual node canvas inside `clip-workbench`, a node pipeline execution engine driven by external tools (Python, Bash, ImageMagick, FFmpeg) to prevent native Rust scope creep, YAML recipe storage in `~/.config/little-screen-clip/recipes/*.yaml`, and recipe integration into the Workbench fuzzy palette and `clipctl` CLI.

---

## User Review Required

> [!IMPORTANT]
> **External Tool Execution Strategy (No Native Rust Scope Creep):**  
> In strict adherence to Section 7.2 of `idea.md`, all transform nodes (Image framing/captioning/resizing, Audio loudness normalization, Text processing) are executed through external CLI utilities (`python3`, `bash`, `magick`/`convert`, `ffmpeg`, `jq`, `sox`) via child processes with temporary files/pipes in `~/.cache/little-screen-clip/pipeline_runs/`.  
> We verified that `python3`, `bash`, `ffmpeg`, `magick`, `convert`, `jq`, and `sox` are already installed on your system.

> [!TIP]
> **Custom Immediate-Mode Node Canvas in `egui 0.27`:**  
> Rather than pulling in outdated or heavy third-party node-graph crates with incompatible dependencies, we implement a lightweight, smooth immediate-mode node canvas directly in `egui`:
> - Smooth canvas pan (right/middle-drag) and zoom (scroll wheel).
> - Glowing cubic Bezier curve connection cables.
> - Port type validation (Text, Image, Audio, File, Any).
> - Draggable node cards with inline property widgets (colors, sliders, scripts).
> - Node catalog search palette with category tags (`#image`, `#text`, `#audio`, `#script`, `#sink`, `#source`).

---

## Open Questions

> [!NOTE]
> 1. **Default Recipe Preloading:**  
>    We will automatically create `~/.config/little-screen-clip/recipes/` on first launch with the canonical recipe from Section 7.1 of `idea.md` (`pad_and_caption.yaml`: LH Image + RH Text Caption $\rightarrow$ Frame $\rightarrow$ Set in LH), along with `normalize_audio.yaml` and `clean_and_slugify.yaml`.
> 2. **Workbench View Switching:**  
>    We propose providing a dual-mode header in the Workbench modal:
>    - `[ 📋 Quick Palette ]` (Default fast fuzzy search action view)
>    - `[ ★ Big Node Machine ]` (Interactive Visual Node Canvas)
>    Switchable with `F2`, `Tab` (when search is empty), or direct mouse click.

---

## Proposed Changes

```
crates/
├── little-clip-core/
│   └── src/
│       └── ipc.rs             # Add ExecuteRecipe, ExecuteGraph, ListRecipes requests & responses
├── little-clip-ops/
│   ├── Cargo.toml             # Add serde_yaml for recipe serialization
│   └── src/
│       ├── lib.rs             # Expose pipeline and recipe modules
│       ├── pipeline/
│       │   ├── mod.rs         # Module exports
│       │   ├── types.rs       # PortType, PortValue, NodeKind, NodeInstance, Connection, NodeGraph
│       │   ├── catalog.rs     # Node registry: templates, inputs/outputs, categories, search tags
│       │   ├── executor.rs    # Topological sort, execution engine (Python, Bash, ImageMagick, FFmpeg)
│       │   └── recipes.rs     # YAML recipe load/save/discovery in ~/.config/little-screen-clip/recipes/
├── little-clipd/
│   └── src/
│       └── main.rs            # Handle ExecuteRecipe & ExecuteGraph IPC requests, sink output to slots
├── clip-workbench/
│   └── src/
│       ├── main.rs            # Tab switching (Palette vs Node Canvas), recipe execution from search
│       └── canvas.rs          # Visual Node Canvas (pan, zoom, node cards, bezier cables, catalog modal)
└── clipctl/
    └── src/
        └── main.rs            # Add `clipctl recipe list` and `clipctl recipe run <name>` subcommands
```

---

### Component 1: `little-clip-ops` (Pipeline Engine & Recipe Store)

#### [MODIFY] [Cargo.toml](file:///home/vega/Documents/git/local/little-screen-clip/Cargo.toml)
- Add `serde_yaml = "0.9"` to `[workspace.dependencies]`.

#### [MODIFY] [crates/little-clip-ops/Cargo.toml](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/Cargo.toml)
- Add `serde_yaml = { workspace = true }`.

#### [NEW] [crates/little-clip-ops/src/pipeline/types.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/types.rs)
- Data structures:
  - `enum PortType { Any, Text, Image, Audio, FilePath, FileBatch }`
  - `enum PortValue { Text(String), ImagePath(PathBuf), AudioPath(PathBuf), FilePath(PathBuf), FileBatch(Vec<PathBuf>) }`
  - `enum NodeKind`:
    - Sources: `LeftHand`, `RightHand`, `Slot(SlotId)`, `ConstantText`, `ConstantNumber`.
    - Operators: `TextTransform`, `ImageTransform`, `AudioTransform`, `FileTransform`.
    - Escape Hatch: `PythonScript`, `BashCommand`.
    - Sinks: `SetLeftHand`, `SetRightHand`, `SaveToDisk`, `PasteToApp`.
  - `struct NodeInstance`: UUID, kind, title, pos `(f32, f32)`, props `HashMap<String, String>`, input/output pins.
  - `struct Connection`: from node/pin to node/pin.
  - `struct NodeGraph`: nodes, connections, pan `(f32, f32)`, zoom `f32`.
  - `struct Recipe`: name, description, tags, graph.

#### [NEW] [crates/little-clip-ops/src/pipeline/catalog.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/catalog.rs)
- Catalog of all built-in node definitions with category and search tags:
  - **Sources:** `Left Hand`, `Right Hand`, `Slot N`, `Literal Text`, `Literal Number`.
  - **Text:** `Trim`, `Slugify`, `Change Case`, `Regex Replace`, `Split Lines`, `Join Lines`, `JSON Query (jq)`, `Base64`.
  - **Images:** `Pad with Frame (Border)`, `Add Title / Caption to Frame`, `Crop`, `Resize`, `Convert to WebP`, `Rotate / Flip`, `Stitch Side-by-Side`, `Strip EXIF`.
  - **Audio:** `Normalize Loudness (-14 LUFS)`, `Trim Silence`, `Adjust Gain (dB)`, `Concatenate`.
  - **Files:** `SHA-256 Hash`, `Compress to Zip`, `Extract Filename`.
  - **Scripting:** `Python Script`, `Bash Command`.
  - **Sinks:** `Set Left Hand`, `Set Right Hand`, `Save to Disk`, `Paste to Active App`.

#### [NEW] [crates/little-clip-ops/src/pipeline/executor.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/executor.rs)
- Graph validation and topological sort (detect cycles).
- Staging directory management under `~/.cache/little-screen-clip/pipeline_runs/<run_id>/`.
- Command execution runner:
  - Invokes `python3` for custom scripts and regex operations.
  - Invokes `bash -c` for shell pipelines and jq queries.
  - Invokes `magick` for image padding, caption annotation, and transforms.
  - Invokes `ffmpeg` for audio loudness normalization and filtering.
- Maps output back to `ClipItem` for sinks.

#### [NEW] [crates/little-clip-ops/src/pipeline/recipes.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-ops/src/pipeline/recipes.rs)
- Manages `~/.config/little-screen-clip/recipes/*.yaml`.
- Default template writer for:
  - `pad_and_caption.yaml`
  - `normalize_audio.yaml`
  - `clean_and_slugify.yaml`
- Recipe loader: `load_all_recipes() -> Vec<Recipe>`, `save_recipe(recipe: &Recipe)`.

---

### Component 2: `little-clip-core` (IPC Messages)

#### [MODIFY] [crates/little-clip-core/src/ipc.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clip-core/src/ipc.rs)
- Add `IpcRequest::ExecuteRecipe { recipe_name: String }`.
- Add `IpcRequest::ExecuteGraph { graph_yaml: String }`.
- Add `IpcRequest::ListRecipes`.
- Add `IpcResponse::RecipeList(Vec<String>)`.

---

### Component 3: `little-clipd` (Daemon Handling)

#### [MODIFY] [crates/little-clipd/src/main.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/little-clipd/src/main.rs)
- Implement handlers for `ExecuteRecipe` and `ExecuteGraph`:
  1. Executes the graph against the current `SlotManager`.
  2. Commits sink results to `mgr.apply_op_result()` (updating `[LH]` or `[RH]` and displacing prior item to `[Undo]`).
  3. Triggers `schedule_async_previews` and `ctx.refresh_display()` so the Trofeo hardware display reflects the new clip immediately.
  4. Copies output to OS clipboard via `copy_to_os_clipboard()`.

---

### Component 4: `clip-workbench` (The Visual Node Canvas)

#### [NEW] [crates/clip-workbench/src/canvas.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/clip-workbench/src/canvas.rs)
- Node canvas state: `graph: NodeGraph`, `selected_node: Option<Uuid>`, `dragging_pin: Option<(Uuid, String, bool)>`, `mouse_pos`, `search_catalog_open: bool`.
- Immediate-mode UI rendering:
  - Canvas transformation (pan offset + zoom scaling).
  - Background grid dots with subtle grid lines.
  - Interactive Bezier cables connecting output pins to input pins with glowing hover indicators.
  - Active drag-wire when connecting pins.
  - Node cards with category headers, close button, title, inline property editors, and socket circles.
  - Sockets with tooltip previews of current/last value.
  - Catalog search palette modal with tag filtering (`#image`, `#text`, `#audio`, etc.).
  - Interactive `[▶ Run Pipeline]` button with live timer and status feedback.
  - Recipe loader dropdown and `[💾 Save Recipe]` dialog.

#### [MODIFY] [crates/clip-workbench/src/main.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/clip-workbench/src/main.rs)
- Add view state: `enum WorkbenchView { Palette, NodeMachine }`.
- Header view tabs: `[ 📋 Quick Palette ]` vs `[ ★ Big Node Machine ]`.
- Shortcut switching (`F2` or `Tab`).
- Include discovered recipes in the Quick Palette search results (e.g. typing `/pad` or `/caption` executes the recipe directly).

---

### Component 5: `clipctl` (CLI Subcommands)

#### [MODIFY] [crates/clipctl/src/main.rs](file:///home/vega/Documents/git/local/little-screen-clip/crates/clipctl/src/main.rs)
- Add `clipctl recipe list` — lists available recipes.
- Add `clipctl recipe run <name>` — executes a recipe via daemon IPC.

---

## Verification Plan

### Automated Tests
- Build workspace:
  ```bash
  cargo check
  cargo test
  ```
- Add unit tests in `little-clip-ops/src/pipeline/executor.rs`:
  - `test_topological_sort_and_cycle_detection`: verify DAG ordering and loop rejection.
  - `test_recipe_yaml_serialization`: verify roundtrip serialization of complex graphs.
  - `test_text_python_node_execution`: verify python script node runs and produces expected text.
  - `test_image_frame_command_generation`: verify ImageMagick command synthesis and execution against test image.

### Manual Verification
1. **Daemon & Hardware Ambient Display:**
   - Restart daemon: `./launch.sh restart`
   - Push an image to Left Hand: `./target/debug/clipctl push --hand left /home/vega/Documents/type_testing/nixie.svg` (or png)
   - Push caption text to Right Hand: `./target/debug/clipctl push --hand right "Nixie Tube Clock v2"`
2. **Workbench Visual Canvas:**
   - Launch Workbench: `./launch.sh ui`
   - Press `F2` or click `[★ Big Node Machine]`.
   - Verify node canvas renders with pan/zoom and nodes.
   - Load the `pad_and_caption` recipe.
   - Click `[▶ Run Pipeline]`.
   - Verify success status notification.
   - Verify Left Hand on both the main monitor mirror and Trofeo screen updates to the framed & captioned image!
3. **Fuzzy Palette Recipe Execution:**
   - Switch back to `[ 📋 Quick Palette ]`.
   - Type `/pad` in the action input.
   - Verify `pad_and_caption` appears with `[Recipe]` tag.
   - Press `Enter` and verify it runs directly.
4. **CLI Recipe Execution:**
   - Run `./target/debug/clipctl recipe list`
   - Run `./target/debug/clipctl recipe run pad_and_caption`
   - Verify output updates Left Hand.
