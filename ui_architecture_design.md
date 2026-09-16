# StudioShell & Unified UI System Architecture

> **Specification & Architectural Roadmap for `clip-workbench` and Little Screen Clip**  
> **Status:** Design Approved via Architecture Interview  
> **Target:** Elimination of ad-hoc UI duplication, visual inconsistencies, and fragmented modal chrome.

---

## 1. Executive Summary & Problem Analysis

Across the `clip-workbench` crate, each previewer and studio tool (`image_preview.rs`, `audio_preview.rs`, `spreadsheet_view.rs`, `pdf_view.rs`, `midi_view.rs`, `stl_view.rs`, `text_preview.rs`, `diff_view.rs`, `binary_diff_view.rs`, `canvas.rs`) attempts to solve the exact same structural UI problems independently:

1. **Ad-hoc Top Chrome & Header Inconsistencies:**
   - `image_preview.rs` implements a custom `egui::Frame` with title badge, dimensions/format/size chips, escape hatches (Krita, Inkscape, Nomacs), Save As, Copy to Slot, and Close.
   - `audio_preview.rs` implements its own custom header, but embeds `StatusToast` inside the header's right layout and uses `Rounding::same(6.0)` instead of the design system's `4.0`.
   - `spreadsheet_view.rs`, `text_preview.rs`, `pdf_view.rs`, `midi_view.rs`, and `stl_view.rs` use `ui_ext::render_view_header`, but misuse its slots: `pdf_view.rs`, `midi_view.rs`, and `stl_view.rs` jam interactive buttons, transport controls, and zoom sliders directly into the `badges` closure because no dedicated sub-toolbar slot existed.
   - `diff_view.rs` and `binary_diff_view.rs` completely bypass `render_view_header` and paint their own custom frames.

2. **Asymmetric Multi-Panel Geometry:**
   - In `image_preview.rs`, the Film Simulation viewport uses `left_w = 235.0`, `right_w = 210.0`, and `bottom_h = 96.0`.
   - In the same file, the Creative FX viewport uses `left_w = 230.0` and `right_w = 295.0` with no bottom panel.
   - In `binary_diff_view.rs`, the differences panel uses `diff_panel_w = 175.0` and minimap `16.0`.
   - None of these sidebars support user resizing or collapse toggles, and panel padding/margins diverge between `6.0`, `8.0`, and `14.0` px.

3. **Duplicated Before/After Split Comparison:**
   - The interactive split slider (with texture UV partitioning, divider line, central draggable pill knob `"◀ ▶"`, and hover detection) is copy-pasted verbatim across Film Simulation (`image_preview.rs:8263-8292`), Creative FX (`image_preview.rs:9945-9974`), and Convert mode.

4. **Fragmented Escape Hatch & Process Lifecycles:**
   - Launching Krita, Inkscape, or Audacity in `image_preview.rs` and `audio_preview.rs` executes `ctx.send_viewport_cmd(egui::ViewportCommand::Close)`, which terminates the entire workbench process without fallback.
   - `text_preview.rs` keeps the workbench open and provides an `xdg-open` fallback.
   - `spreadsheet_view.rs`, `pdf_view.rs`, `midi_view.rs`, `stl_view.rs`, `diff_view.rs`, and `binary_diff_view.rs` provide zero escape hatches, leaving users stranded with no way to open their files in external editors (Calc, Okular, MuseScore, Blender, Meld, ImHex).

5. **Disparate Bottom Footers, Help Hints, and Toast Rendering:**
   - `image_preview.rs` and `binary_diff_view.rs` have rich bottom status footers with keyboard shortcuts and inspection readouts.
   - `spreadsheet_view.rs`, `pdf_view.rs`, `midi_view.rs`, and `stl_view.rs` have no footer at all.
   - `StatusToast` is rendered in the footer in `image_preview.rs`, in the top header in `audio_preview.rs`, and floating in the content body in `text_preview.rs`.

6. **Workbench & Display Mirror Coupling in `main.rs`:**
   - `WorkbenchApp` manages 10 separate `Option<State>` fields, rendering each through repetitive `CentralPanel` modal boilerplate.
   - The 1920×462 Trofeo Display Mirror is rendered via hardcoded pixel hit-testing math (`rel_x < 240.0`, `< 600.0`, etc.) in `main.rs` that contradicts dynamic slot configurations in `config.toml`.

---

## 2. The 5-Zone UI Layout Architecture (`StudioShell`)

Every studio viewer, editor, and preview modal in `clip-workbench` adheres to a strict 5-zone layout managed by a centralized builder: `StudioShell`.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ZONE 1: TOP HEADER & ACTIONS BAR                                             │
│ [Icon] [Title] [Badges...] ─────── [Hatch 1] [Hatch 2] [Actions...] [✕ Close]│
├──────────────────────────────────────────────────────────────────────────────┤
│ ZONE 2: MODE & VIEWPORT TOOLBAR                                              │
│ [Mode 1] [Mode 2] [Mode 3] ─────── [Reset] [1:1] [Fit] [-] [ 100% ] [+]     │
├──────────────────────────────────────────────────────────────────────────────┤
│ ZONE 2b: CONTEXTUAL SUB-STRIP (Optional: Tool parameters, search filter, etc) │
├──────────────┬────────────────────────────────────────────────┬──────────────┤
│ ZONE 3A:     │ ZONE 3B: INTERACTIVE CENTER CANVAS / VIEWPORT   │ ZONE 3C:     │
│ LEFT SIDEBAR │ (Image, Audio Waveform, 2D Grid, Piano Roll,  │ RIGHT PANEL  │
│ Presets /    │  3D Orbital Mesh, Code / Diff Viewer)          │ Inspector /  │
│ Thumbnails / │                                                │ Parameters / │
│ Sheets List  │                                                │ Properties   │
│ (240px def.) │                                                │ (280px def.) │
├──────────────┴────────────────────────────────────────────────┴──────────────┤
│ ZONE 3D: BOTTOM DRAWER (Optional: Effects, Timelines, Mini-Strips - 96px def)│
├──────────────────────────────────────────────────────────────────────────────┤
│ ZONE 4: STATUS, FOOTER & SHORTCUT HELP BAR                                   │
│ 🖱 Contextual Shortcuts Help ──── 🔍 Live Hover Readout ──── 📋 Toast Pills  │
└──────────────────────────────────────────────────────────────────────────────┘
  ZONE 5: OVERLAY SYSTEM (Modals, Confirmation Dialogs, Zenity Fallbacks)
```

---

## 3. Core Abstractions & Data Structures

All new abstractions reside in `crates/clip-workbench/src/shell/` and are re-exported into `clip-workbench`.

### 3.1 `StudioShell` Component Builder

```rust
pub struct StudioShell<'a> {
    title: &'a str,
    icon: &'a str,
    palette: &'a AppPalette,
    close_flag: &'a mut bool,
    escape_hatches: Vec<EscapeHatch>,
    header_badges: Vec<HeaderBadge>,
    header_actions: Vec<ShellAction>,
    modes: Vec<ShellMode>,
    active_mode_idx: Option<usize>,
    zoom_controls: Option<ZoomState>,
    footer_hints: Option<&'a str>,
    footer_readout: Option<String>,
    toast: Option<&'a mut StatusToast>,
    panel_config: PanelConfig,
}
```

#### Builder Fluent API
```rust
StudioShell::new("Image Studio", "🖼", palette, close)
    .with_badge("4000×3000", BadgeStyle::Dimension)
    .with_badge("PNG", BadgeStyle::Format)
    .with_badge("2.4 MB", BadgeStyle::Size)
    .with_escape_hatch(EscapeHatch::new("Krita", "krita", "Open in Krita (Raster Editor)"))
    .with_escape_hatch(EscapeHatch::new("Inkscape", "inkscape", "Open in Inkscape (Vector Editor)"))
    .with_primary_action("💾 Save As... [Ctrl+S]", ActionId::SaveAs)
    .with_primary_action("📋 Copy Slot [Ctrl+C]", ActionId::CopySlot)
    .with_modes(&["View [V]", "Crop [C]", "Deskew [K]", "Film Sim [L]"], current_mode_idx)
    .with_zoom_controls(zoom_pct)
    .with_footer_hints("🖱 Scroll: Zoom | Drag: Pan | Enter: Apply | Esc: Close")
    .with_status_toast(&mut self.status_toast)
    .show_three_panel(ui, ctx, &mut self.panel_state, |panels| {
        panels.left(|ui| self.render_presets_panel(ui));
        panels.center(|ui| self.render_canvas_viewport(ui));
        panels.right(|ui| self.render_inspector_panel(ui));
        panels.bottom(|ui| self.render_secondary_effects_drawer(ui));
    });
```

---

### 3.2 Standardized Panel Geometry & Persistent State (`PanelConfig`)

Eliminates the arbitrary divergence between `235px` (FilmSim), `230px` (CreativeFX), and `175px` (BinaryDiff).

```rust
pub const PANEL_LEFT_DEFAULT_WIDTH: f32 = 240.0;
pub const PANEL_RIGHT_DEFAULT_WIDTH: f32 = 280.0;
pub const PANEL_BOTTOM_DEFAULT_HEIGHT: f32 = 96.0;
pub const PANEL_MIN_WIDTH: f32 = 180.0;
pub const PANEL_MAX_WIDTH: f32 = 450.0;
pub const PANEL_MIN_HEIGHT: f32 = 60.0;
pub const PANEL_MAX_HEIGHT: f32 = 250.0;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PanelState {
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
    pub left_collapsed: bool,
    pub right_collapsed: bool,
    pub bottom_collapsed: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            left_width: PANEL_LEFT_DEFAULT_WIDTH,
            right_width: PANEL_RIGHT_DEFAULT_WIDTH,
            bottom_height: PANEL_BOTTOM_DEFAULT_HEIGHT,
            left_collapsed: false,
            right_collapsed: false,
            bottom_collapsed: false,
        }
    }
}
```

#### Panel Chrome Features:
1. **Collapsible Header Pills:** Small `◀` / `▶` toggle buttons on sidebars to reclaim screen space.
2. **Interactive Drag-Resize Dividers:** 3px invisible grab zones on borders with `CursorIcon::ResizeHorizontal` and `CursorIcon::ResizeVertical`.
3. **Contrast-Guaranteed Surface Fill:** Inner frames automatically rendered with `palette.theme.b_low`, 1px `palette.theme.b_med` stroke, and `Rounding::same(4.0)`.

---

### 3.3 Unified Escape Hatch Architecture (`EscapeHatchRegistry`)

External application launching is extracted into a reliable, non-fatal subsystem with automatic `xdg-open` fallback and standard feedback.

```rust
pub struct EscapeHatch {
    pub label: String,
    pub icon: &'static str,
    pub command: String,
    pub tooltip: String,
    pub is_primary: bool,
}

pub struct EscapeHatchLauncher;

impl EscapeHatchLauncher {
    /// Launches an external application with the given file path.
    /// 1. Tries the configured command (e.g. "krita", "audacity", "code", "calc").
    /// 2. If command is not found, attempts `xdg-open <file_path>` fallback.
    /// 3. Emits a contrast-safe StatusToast notification.
    /// 4. Leaves the workbench open (or minimizes if configured), never killing the process.
    pub fn launch(
        app_name: &str,
        command: &str,
        file_path: &Path,
        toast: &mut StatusToast,
        now: f64,
    ) -> bool {
        let mut cmd = std::process::Command::new(command);
        cmd.arg(file_path);
        match cmd.spawn() {
            Ok(_) => {
                toast.set(format!("🚀 Launched {} ({})", app_name, file_path.file_name().unwrap_or_default().to_string_lossy()), now);
                true
            }
            Err(_) => {
                // Fallback to system default desktop handler
                if std::process::Command::new("xdg-open").arg(file_path).spawn().is_ok() {
                    toast.set(format!("🚀 Opened in default application via xdg-open"), now);
                    true
                } else {
                    toast.set(format!("⚠️ Failed to launch '{}' (tool not installed)", command), now);
                    false
                }
            }
        }
    }
}
```

#### Standard Escape Hatch Registry per File Type:
| View | Primary Tool | Secondary Tool | Fallback |
|---|---|---|---|
| **Image Studio (Raster)** | `🎨 Krita` (`krita`) | `👁 Nomacs` (`nomacs`) | `xdg-open` |
| **Image Studio (Vector)** | `✒ Inkscape` (`inkscape`) | `👁 Nomacs` (`nomacs`) | `xdg-open` |
| **Audio Studio** | `🔊 Audacity` (`audacity`) | `Tenacity` (`tenacity`) | `xdg-open` |
| **Spreadsheet Studio** | `📊 LibreOffice Calc` (`localc`) | — | `xdg-open` |
| **PDF Studio** | `📑 Okular` (`okular`) | `Evince` (`evince`) | `xdg-open` |
| **MIDI Studio** | `🎹 MuseScore` (`mscore`) | `Ardour` (`ardour`) | `xdg-open` |
| **STL 3D Studio** | `🧊 Blender` (`blender`) | `FreeCAD` (`freecad`) | `xdg-open` |
| **Code / Text Studio** | `📝 External Editor` (from `config.toml`) | — | `xdg-open` |
| **Diff Viewer** | `⑂ Meld` (`meld`) | `KDiff3` (`kdiff3`) | `xdg-open` |
| **Binary Diff** | `01 ImHex` (`imhex`) | `Okteta` (`okteta`) | `xdg-open` |

---

### 3.4 Shared Split-Slider Comparison Widget (`SplitCompareView`)

Replaces the copy-pasted mesh shaders and slider bars in `FilmSim`, `CreativeFx`, and `Convert`.

```rust
pub struct SplitCompareView<'a> {
    pub original_texture: &'a egui::TextureHandle,
    pub processed_texture: Option<&'a egui::TextureHandle>,
    pub split_position: &'a mut f32, // 0.0 ..= 1.0 (clamped 0.05 ..= 0.95)
    pub is_dragging: &'a mut bool,
}

impl<'a> SplitCompareView<'a> {
    pub fn show(self, ui: &mut egui::Ui, rect: Rect, palette: &AppPalette) -> egui::Response {
        // Centralized implementation of:
        // 1. Left mesh UV rect (0.0..split)
        // 2. Right mesh UV rect (split..1.0)
        // 3. Double-stroked contrast line
        // 4. Circular thumb knob with "◀ ▶" text and guaranteed contrast
        // 5. Drag handling and double-click to center (0.5)
    }
}
```

---

### 3.5 Reusable Hardware Display Mirror (`ScreenMirrorWidget`)

Extracts the 1920×462 Trofeo display preview out of `main.rs`, replacing hardcoded pixel coordinates (`< 240.0`, `< 600.0`, etc.) with geometry dynamically derived from `little-clip-render`.

```rust
pub struct ScreenMirrorWidget<'a> {
    pub slot_manager: &'a SlotManager,
    pub mirror_texture: Option<&'a egui::TextureHandle>,
    pub palette: &'a AppPalette,
    pub left_priors_count: usize,
}

pub enum MirrorInteraction {
    ClickedSlot(SlotId),
    DoubleClickedSlot(SlotId),
    DraggedSlot { from: SlotId, to: SlotId },
}

impl<'a> ScreenMirrorWidget<'a> {
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<MirrorInteraction> {
        // Computes pixel-exact slot rects from SlotLayout geometry
        // Handles drag-and-drop slot swapping visually
        // Displays slot labels, types, and active animations
    }
}
```

---

### 3.6 Centralized Shortcut Dispatcher & Cancellation Stack

```rust
pub enum ShortcutEvent {
    SaveRequested,
    CopyRequested,
    UndoRequested,
    RedoRequested,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomActual,
    CloseRequested,
}

pub struct ShortcutDispatcher;

impl ShortcutDispatcher {
    /// Evaluates input and handles the hierarchical Escape cancellation stack:
    /// 1. If text input focused: surrenders focus.
    /// 2. If background task in progress (e.g. OCR, Stem Separation, Optimization): cancels task.
    /// 3. If active interactive tool has pending state (e.g. crop rect modified, 4 points placed, redactions unburned): resets tool.
    /// 4. If selection active: clears selection.
    /// 5. Otherwise: requests modal close (`close = true`).
    pub fn process(
        ctx: &egui::Context,
        can_cancel_op: bool,
        cancel_op: impl FnOnce(),
        can_clear_selection: bool,
        clear_selection: impl FnOnce(),
        close: &mut bool,
    ) -> Option<ShortcutEvent>;
}
```

---

## 4. File-by-File Migration Plan

### 4.1 `crates/clip-workbench/src/image_preview.rs`
- **Current Lines:** ~11,794 lines.
- **Changes:**
  - Replace `render_header()` (~120 lines) with `StudioShell` header builder.
  - Replace `render_toolbar()` with `StudioShell` mode and zoom bar.
  - Replace `render_footer()` with `StudioShell` footer hint & toast renderer.
  - Delete duplicate split-screen slider blocks in `FilmSim` (lines 8263–8292) and `CreativeFx` (lines 9945–9974); call `SplitCompareView::show()`.
  - Harmonize `FilmSim` and `CreativeFx` side panels to standard `PANEL_LEFT_DEFAULT_WIDTH` (240px) and `PANEL_RIGHT_DEFAULT_WIDTH` (280px).
  - Migrate `launch_external_app()` to `EscapeHatchLauncher`. Remove process-killing `ctx.send_viewport_cmd(Close)`.

### 4.2 `crates/clip-workbench/src/audio_preview.rs`
- **Current Lines:** ~4,485 lines.
- **Changes:**
  - Replace `render_audio_header()` and `render_audio_tool_bar()` with `StudioShell`.
  - Move `StatusToast` out of the top header and into the standard bottom footer.
  - Standardize button corner rounding from `Rounding::same(6.0)` to design system `Rounding::same(4.0)`.
  - Migrate Audacity launching to `EscapeHatchLauncher` with `xdg-open` fallback.
  - Fix crash hazard `.unwrap()` on `ch2_samples` in stereo wave drawing.

### 4.3 `crates/clip-workbench/src/spreadsheet_view.rs`
- **Current Lines:** ~259 lines.
- **Changes:**
  - Wrap `SpreadsheetViewerState` in `StudioShell`.
  - Add standard escape hatch: `LibreOffice Calc` (`localc`) with `xdg-open` fallback.
  - Add standard bottom footer with search match count and cell coordinate readout (`Row 4, Col B`).
  - **Performance Fix:** Eliminate per-frame `current_tsv` string allocation; generate only when the Copy button is triggered.

### 4.4 `crates/clip-workbench/src/pdf_view.rs`
- **Current Lines:** ~327 lines.
- **Changes:**
  - Remove Rotate and Delete buttons from `badges` closure; place in secondary sub-strip or header action slot.
  - Wrap in `StudioShell` with standard bottom footer.
  - Add standard escape hatch: `Okular` (`okular`) / `Evince` (`evince`).
  - Drop `png_bytes` once GPU texture is created to free memory.

### 4.5 `crates/clip-workbench/src/midi_view.rs`
- **Current Lines:** ~789 lines.
- **Changes:**
  - Remove transport buttons and zoom control bar from the header `badges` slot; route to `StudioShell` mode/transport bar.
  - Add standard bottom footer with hovered note pitch, velocity, and time readout.
  - Add standard escape hatch: `MuseScore` (`mscore`) / `Ardour` (`ardour`).

### 4.6 `crates/clip-workbench/src/text_preview.rs`
- **Current Lines:** ~1,332 lines.
- **Changes:**
  - Wrap with `StudioShell`. Move floating toast from the middle of the frame down to the bottom footer.
  - Standardize Find toolbar into Zone 2b (contextual sub-strip).
  - Standardize external editor escape hatch to use `EscapeHatchLauncher`.

### 4.7 `crates/clip-workbench/src/diff_view.rs` & `binary_diff_view.rs`
- **Current Lines:** ~1,186 lines (`diff_view`) and ~1,532 lines (`binary_diff_view`).
- **Changes:**
  - Replace custom painted background frames and headers with `StudioShell`.
  - Standardize diff stats bar into Zone 2b.
  - Add external diff tool escape hatches (`Meld`, `KDiff3`, `ImHex`).
  - Standardize binary diff right panel width from `175px` to `PANEL_RIGHT_DEFAULT_WIDTH` (280px) or `SIDEBAR_NARROW` (220px).

### 4.8 `crates/clip-workbench/src/stl_view.rs`
- **Current Lines:** ~342 lines.
- **Changes:**
  - Remove auto-rotate and wireframe buttons from header `badges`; route to Zone 2b sub-toolbar.
  - Add standard escape hatch: `Blender` (`blender`) / `FreeCAD` (`freecad`).
  - Add standard bottom footer: `"🖱 Drag: Orbit | Shift+Drag: Pan | Scroll: Zoom"`.
  - Fix delta-time bug in auto-rotation.

### 4.9 `crates/clip-workbench/src/main.rs`
- **Current Lines:** ~2,433 lines.
- **Changes:**
  - Extract the 1920×462 Trofeo display preview into `ScreenMirrorWidget`.
  - Derive slot hit-test rectangles dynamically from `little-clip-render` layout logic instead of hardcoded `< 240.0` pixel thresholds.
  - Replace repetitive `if let Some(viewer) = &mut self.xyz { CentralPanel::default().show(...) }` blocks with a unified `render_active_modal` dispatcher.

---

## 5. Architectural Duplication & Complexity Impact

| Metric | Current Working Tree | Target After StudioShell Migration | Delta / Benefit |
|---|---|---|---|
| **Duplicated Lines** | **1,940 lines** | **< 350 lines** | **-82% duplication reduction** |
| **Clone Groups (jscpd)** | **210 groups** | **< 25 groups** | **-88% clone group elimination** |
| **Max Cyclomatic Complexity** | **163** (`render_ui`) | **< 40** | **-75% complexity reduction** |
| **UI Header Implementations** | 10 independent versions | 1 unified `StudioShell` builder | 100% consistency |
| **Escape Hatch Availability** | 3 out of 10 views | 10 out of 10 views | All views have external app hatches |
| **Process-Killing Viewport Close** | Unconditional on 2 tools | 0 (Non-destructive `EscapeHatchLauncher`) | No accidental workbench terminations |
| **Side Panel Widths** | 235px / 230px / 175px / 295px | Standard 240px / 280px tokens | Visual consistency across studios |

---

## 6. Implementation Verification Plan

1. **Visual Contrast Verification:**
   - Execute existing test suite: `cargo test -p clip-workbench -- test_all_45_themes_all_buttons_contrast` to confirm all newly structured header, panel, and footer buttons maintain WCAG AA 4.5:1 contrast across all 45 themes.
2. **Layout & Interaction Check:**
   - Verify every studio view renders the 5 standard zones without visual clipping or overlapping scroll areas.
   - Test collapse toggles and drag-resizing on multi-panel views (`FilmSim`, `CreativeFx`, `BinaryDiff`).
   - Test escape hatch launching on every view type (ensuring workbench remains running with fallback to `xdg-open`).
   - Test Escape key cancellation stack across complex modes (e.g. OCR in Image Studio, multi-selection in Audio Studio).
3. **Fitness Vector Regression Test:**
   - Run `architecture_fitness` to measure `duplication_percentage` delta, verifying line duplication drops below 2%.
