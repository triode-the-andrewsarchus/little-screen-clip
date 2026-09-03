# Little Screen Clip — System Specification & Architectural Blueprint (v2)

> **Document Version:** `idea.md` (v2 — Session Compaction & Phase 4 Preparation)  
> **Supersedes:** `rough_idea_ai_1.md`, `rough_idea_ai_0.md`, `rough_idea_vega_0.md`  
> **Current Date:** September 2026  
> **Workspace Tech Stack:** 100% Rust (Edition 2024), Tokio Async IPC, Tiny-Skia, Egui (eframe), Trofeo Vision USB Driver (`rusb`).

---

## 1. Executive Summary & Ergonomic Philosophy

**Little Screen Clip** is an ambient clipboard and physical data-juggling environment designed around **skeuomorphic handling**: humans think in terms of picking up, holding, inspecting, combining, and crafting items — like a video-game hotbar (Minecraft, inventory belts, dual-wielding).

### The Dual-Surface Paradigm

```
┌───────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                     PRIMARY DESKTOP MONITOR                                       │
│                                                                                                   │
│   [Your Code Editor / DAW / Blender / Browser / Terminal]                                         │
│                                                                                                   │
│   ┌───────────────────────────────────────────────────────────────────────────────────────────┐   │
│   │                 THE CRAFTING WORKBENCH (`clip-workbench` Desktop Modal)                   │   │
│   │                                                                                           │   │
│   │   [Undo: Z]   [L.Prior 2] [L.Prior 1] [LEFT HAND]  │  [RIGHT HAND] [R.Prior 1] [R.Prior 2]│   │
│   │  ───────────────────────────────────────────────────────────────────────────────────────  │   │
│   │   > /mix_audio __________________________________________  [Dual-Wield: LH + RH]          │   │
│   │   [ Parameters: Gain Balance 50/50 | Normalize: -14 LUFS | Align: Start ]                 │   │
│   │   [ Live Preview: Stereo WAV • 03:24 • Crossfade 50ms ]                                   │   │
│   │                                                                                           │   │
│   │   [1..7: Swap Slots]   [Tab: Focus Hand]   [Enter: Craft to Hand]   [Ctrl+Enter: Paste]   │   │
│   └───────────────────────────────────────────────────────────────────────────────────────────┘   │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘
                                                ▲
                                                │ Unix Domain Socket IPC (`little-clipd`)
                                                ▼
┌───────────────────────────────────────────────────────────────────────────────────────────────────┐
│                          PERIPHERAL USB DISPLAY (1920×480 Trofeo Vision)                          │
│                                           THE AMBIENT HUD                                         │
│                                                                                                   │
│  [Z: UNDO] │ [LP2: Code] │ [LP1: Notes] │ [LEFT HAND]  │  [RIGHT HAND] │ [RP1: SVG]  │ [RP2: WAV] │
│  (Last H)  │ 42 lines Py │ 12 lines MD  │ [HERO CARD]  │  [HERO CARD]  │ nixie.svg   │ 44.1k/16b  │
│            │             │              │  ACTIVE CLIP │  SECONDARY    │ Vector 210× │ |||||||||| │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘
```

1. **Surface 1: The Ambient HUD (Little Screen — 1920×480 Trofeo Vision USB)**
   - **Role:** Pure glanceability. **Never captures mouse focus, never displays dropdowns or prompts.**
   - **Question answered:** *"What am I holding right now, and what is in my inventory?"*
   - **Hardware Reality:** The Trofeo screen (`0416:5408` Winbond USB) does *not* appear as an OS monitor. It is driven via direct USB bulk transfer packets (`rusb`) with a software framebuffer rasterizer (`tiny-skia`).
2. **Surface 2: The Crafting Workbench (`clip-workbench` on Main Monitor)**
   - **Role:** Active desktop overlay modal (built in `eframe`/`egui`).
   - **Question answered:** *"What do I want to inspect, transform, craft, or pipeline?"*
   - **Features:** Live mirror of the slots header, keyboard navigation (`1..7`, `H`, `Z`, `Tab`), fuzzy command palette (`/` or typing), live preview diffs, and the upcoming **Visual Node Canvas**.

---

## 2. Inventory & Slot Architecture: "Two Hands & Two Stacks"

Unlike traditional flat clipboard histories, Little Screen Clip implements a **dual-wielding inventory**:

```
 [OS Clipboard] ──(Ctrl+C)──> [LEFT HAND] (Primary)
                                   │
               Push on new copy    ▼
                               [L. PRIOR 1]
                                   │
                                   ▼
                               [L. PRIOR 2]

 [Secondary / Crafting Target] ──> [RIGHT HAND] (Secondary)
                                   │
               Push on RHS copy    ▼
                               [R. PRIOR 1]
                                   │
                                   ▼
                               [R. PRIOR 2]

   Any operation replacing a Hand pushes the old item to: [UNDO: Z]
```

### The 7 Physical Slots

| Slot Key | Display Position | Role | Typical Use |
| :--- | :--- | :--- | :--- |
| **`Undo`** (`Z`) | Far Left (`x ≈ 28`) | Safety Net | Stores whatever was displaced from `[LH]` or `[RH]` by an operation. Swappable via `Z`. |
| **`L.Prior 2`** | Left Column 2 | Left History Depth 2 | Prior clip #2 on Left stack. |
| **`L.Prior 1`** | Left Column 3 | Left History Depth 1 | Prior clip #1 on Left stack. |
| **`Left Hand`** (`LH` / `1`) | Center-Left Hero (`w = 356`) | **Primary Active** | Default target for OS `Ctrl+C`. Whatever is in `LH` pastes on `Ctrl+V`. |
| **`Right Hand`** (`RH` / `2`)| Center-Right Hero (`w = 356`)| **Secondary Active**| Target for secondary pushes (`clipctl push --hand right`), dual-wield crafting arg 2. |
| **`R.Prior 1`** | Right Column 5 | Right History Depth 1 | Prior clip #1 on Right stack. |
| **`R.Prior 2`** | Far Right (`x ≈ 1684`) | Right History Depth 2 | Prior clip #2 on Right stack. |

---

## 3. Current Implementation Status (Phases 1–3 Complete)

### Implemented Workspace Crates (`Cargo.toml` Workspace, Rust 2024)

```
crates/
├── little-clip-core/    # Core data structures (ClipItem, ClipPayload, SlotManager, ImageThumbnail, Config)
├── little-clip-driver/  # Trofeo Vision USB driver (rusb, Winbond 0416:5408, LY1 protocol, PM=49)
├── little-clip-render/  # 1920x462 software rasterizer (tiny-skia, fontdue, markdown AST, waveforms, grids)
├── little-clip-ops/     # Unary and binary crafting operations engine (Text transforms, case, clean URL, diff)
├── little-clipd/        # Background daemon, Unix socket IPC server, clipboard listener, async thumbnailer
├── clipctl/             # Headless CLI utility (push, pop, swap, inspect, list, ops)
└── clip-workbench/      # Main monitor egui modal overlay (slot mirror, fuzzy palette, keyboard nav)
```

---

## 4. Supported MIME & Preview Engines

| Format Category | Handled Types | Visual Representation on Hardware Screen |
| :--- | :--- | :--- |
| **Markdown** | `.md`, `.markdown` | Custom tokenizer & span renderer: headings (`#`, `##`, `###`), bullets (`• `), numbered lists, blockquotes (`>`), horizontal rules, code blocks (`#0f172a`), inline bold, italics, code, links, and overflow badge (`▼ +N more lines`). |
| **Text & Configs** | `.txt`, `.ini`, `.json`, `.yaml`, `.toml` | Bounded line layout with card clipping, line numbers, char/line badges, and JSON object/array summary tags. |
| **Raster Images** | `.png`, `.jpg`, `.jpeg`, `.webp`, `.bmp` | Fast integer downsampling (`dyn_img.thumbnail`), aspect ratio scaling, dimension badges (`1920×1080`), file size. |
| **Vector Graphics** | `.svg` (files & clipboard XML) | Native `rsvg-convert` rasterization in $< 10\text{ ms}$, `viewBox` dimension parser, `Vector (W×H) • Size • SVG` subtitle, `SVG (VECTOR)` tag. |
| **Multi-Picture** | Multiple image files | Dynamic adaptive grid: **2** side-by-side; **3** triangular; **4** in 2×2; **$> 4$** picks first 2 and last 2 with center `+N more` pill badge and `#1`, `#2` index tags. Async generation. |
| **Animated & Video**| `.gif`, animated `.webp`, `.mp4`, `.webm` | Fast MJPEG/GIF frame extraction, live thumbnail playback paced at $\le 10\text{ fps}$ to protect Trofeo USB watchdog from reset. |
| **Audio** | `.wav`, `.mp3`, `.flac`, `.ogg` | Symphonia metadata extraction, duration (`03:42`), sample rate (`44.1 kHz`), peak RMS waveform multi-band bar chart. |
| **Colors** | Hex strings (`#FF8800`, `#F80`) | Solid color swatch preview, RGB values, hex code title, color space pill. |
| **Files / Batches** | Single or multiple non-media files | File icon, extension badge, elided filename (`filename▸.ext`), total size. |

---

## 5. Critical Lessons & Engineering Gotchas (MUST READ FOR FUTURE AGENTS)

1. **Trofeo USB Watchdog Reset (Display Flickering/Restarting):**
   - The Trofeo device requires a keepalive heartbeat every ~1.0–2.0s, but **it also crashes if frames are pushed too fast**.
   - Frame rate pushed to USB must be capped at $\le 10\text{ fps}$ (100ms between frames). Never push frames on a 60fps loop.
2. **IPC Payload Serialization Choke (Workbench UI Lag):**
   - Raw image `bytes: Vec<u8>` in `ClipPayload::Image` must **never** be serialized across JSON IPC.
   - We marked `bytes: Vec<u8>` with `#[serde(skip)]`. Thumbnails are pre-generated on ingest ($200\times 160$ or $320\times 220$), keeping `GetState` JSON payloads at $\approx 400\text{ KB}$ instead of $30\text{ MB}$. IPC latency dropped from $850\text{ ms}$ to $0.5\text{ ms}$.
3. **Async Thumbnail Ingestion:**
   - Any client sending `PushText` or copying large files must receive an IPC response in $< 1\text{ ms}$.
   - Heavy decoding (e.g. 24-megapixel camera JPEGs, animated GIFs, audio waveforms) is delegated to `tokio::task::spawn_blocking` via `schedule_async_previews`, updating the `SlotManager` and triggering `ctx.refresh_display()` when finished.
4. **Dev Build Optimization (`Cargo.toml`):**
   - Always retain `[profile.dev.package."*"] opt-level = 3`. Media crates (`image`, `tiny-skia`, `symphonia`) run $50\times$ faster while workspace crates compile in 1 second.
5. **Single-Character Elision (`truncate_filename_elided`):**
   - Long filenames preserve their extension and use a single colored character (default `▸` in box border color) rather than an ellipsis `...`, maximizing readable characters in narrow cards.

---

## 6. The Roadmap & Where We Are

```
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ Phase 1: The Ambient USB Mirror (Trofeo Display Driver)       [COMPLETE] │
  │ • USB bulk streaming driver for 0416:5408 Winbond Trofeo Vision          │
  │ • 1920x462 software framebuffer generator (tiny-skia)                    │
  │ • Slots: Undo, Left Prior 2/1, Left Hand, Right Hand, Right Prior 1/2    │
  │ • Rich cards: Text, Markdown, Audio Waveform, Images, SVG, Animated GIF  │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 2: The Workbench Modal (Main Screen Overlay)            [COMPLETE] │
  │ • egui/eframe desktop overlay window (`clip-workbench`)                  │
  │ • Real-time mirror header of all ambient slots                           │
  │ • Instant slot swapping (1..7, H, Z, Tab) and inspection                 │
  │ • Fuzzy command palette with unary operations & diff preview             │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 3: Dual-Slot "Crafting" & Headless CLI Control          [COMPLETE] │
  │ • Two-handed crafting model (Left Hand + Right Hand -> Transform)        │
  │ • Headless CLI (`clipctl`) for scripting and window manager keybinds     │
  │ • Multi-picture grid preview (2, 3, 2x2 with center overflow badge)      │
  │ • Native SVG vector rendering pipeline via `rsvg-convert`                │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 4: The Big Node Machine & Visual Pipeline Editor        [COMPLETE] │
  │ • Visual Node Canvas embedded in the Workbench modal (Tab / F2 toggle)   │
  │ • Connect slots as inputs (LH, RH, S_k) into modular transform nodes     │
  │ • Custom reusable recipes (Save/Load pipelines as YAML in ~/.config)     │
  │ • External CLI execution engine (Python, Bash, ImageMagick, FFmpeg, jq)  │
  └──────────────────────────────────────────────────────────────────────────┘

```

---

## 7. Phase 4 Blueprint: "The Big Node Machine"

The next phase of development transitions Little Screen Clip from an advanced clipboard into a **visual data workbench**.

### 7.1 Visual Node Canvas Architecture

Embedded inside `crates/clip-workbench` as a dedicated tab/view (toggled with `Tab` or `F2`):

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────┐
│ THE CRAFTING WORKBENCH — [SLOTS MIRROR]  [PALETTE]  [★ NODE MACHINE]                             │
├──────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                  │
│   ┌──────────────┐         ┌────────────────────┐                                                │
│   │  LEFT HAND   │[Out]───►│ Pad with frame     │                                                │
│   │  (Picture)   │         │                    │                                                │
│   └──────────────┘         └──────────┬─────────┘                                                │
│                                       │                                                          │
│                                       ▼ [Picture Out]                                            │
│   ┌──────────────┐           ┌────────────────────┐                    ┌─────────────┐           │
│   │  RIGHT HAND  │[Out]─────►│ Add title to frame │───[Picture Out]───►│ SET IN HAND │           │
│   │ (text)       │           │ as caption         │                    │ [always LH] │           │
│   └──────────────┘           └────────────────────┘                    └─────────────┘           │
│                                                                                                  │
│                                                                        [▶ Run]   [Save Recipe]  │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 7.2 Node Categories & Specification

1. **Source / Inventory Nodes:**
   - `Left Hand Source`: Emits active `ClipPayload` in `[LH]`.
   - `Right Hand Source`: Emits active `ClipPayload` in `[RH]`.
   - `Slot N Source`: Emits clip from specific prior slot (`LP1`, `LP2`, `RP1`, `RP2`, `Undo`).
   - `Literal / Constant Node`: User-defined string, regex pattern, delimiter, or number.
2. **Transform / Operator Nodes:**

   !> Important! none of these should be implimented in native Rust. To avoid scope creep (at least for now) everything should be done via calling
      through the external script nodes (Python/Bash/etc) using tools like FFMPEG, imagemagic, headless blender, etc.

   - **Text:** `Trim`, `Slugify`, `Change Case`, `Regex Replace`, `Split Lines`, `Join Lines`, `JSON Format/Extract`, `Base64 Encode/Decode`.
   - **Images:** `Crop`, `Resize (Max W/H)`, `Convert to WebP`, `Rotate/Flip`, `Stitch Side-by-Side`, `Strip EXIF`.
   - **Audio:** `Normalize Loudness (-14 LUFS)`, `Trim Silence`, `Adjust Gain (dB)`, `Concatenate`.
   - **Files:** `Batch Rename Pattern`, `Extract Filename`, `Compute SHA-256`, `Compress to Zip`.

   - These should have a tagging system, to make searching for nodes in the editor easier. 

3. **External Script Nodes ("The Escape Hatch"):**
   - `Python Script Node`: Executes a user script snippet (`def transform(input): ...`).
   - `Bash Command Node`: Pipes input through a shell command (e.g. `jq .data`, `pandoc -f gfm -t plain`).
4. **Sink / Action Nodes:**
   - `Set Left Hand`: Writes result to `[LH]` and pushes previous to `[Undo]`.
   - `Paste to Application`: Sends simulated `Ctrl+V` to the foreground window.
   - `Save to Disk`: Writes to staging or chosen directory.

### 7.3 Recipe Storage & Sharing
- Recipes will be stored in `~/.config/little-screen-clip/recipes/*.yaml`.
- Fast execution: Once saved, recipes appear directly in the Workbench fuzzy command palette (e.g. typing `/markdown_link` executes the entire graph in one stroke).

---

## 8. Development Cheat Sheet for the Next Agent

### How to Run & Test
```bash
# 1. Build workspace
cargo build

# 2. Run all unit tests
cargo test

# 3. Restart the background daemon
./launch.sh restart

# 4. View daemon logs in real-time
./launch.sh logs

# 5. Open the Crafting Workbench GUI modal
./launch.sh ui

# 6. Push test items using the CLI
./target/debug/clipctl push --hand left "/home/vega/Documents/type_testing/nixie.svg"
./target/debug/clipctl push --hand right "Some text snippet"
./target/debug/clipctl status
```

### Key Paths & Locations
- **IPC Socket:** `/run/user/1000/little-screen-clip.sock`
- **Daemon Log:** `/home/vega/Documents/git/local/little-screen-clip/little-clipd.log`
- **Config File:** `/home/vega/Documents/git/local/little-screen-clip/config.toml`
- **Test Files Directory:** `/home/vega/Documents/type_testing/` (contains `.svg`, `.mp4`, `.gif`, `.png`, `.jpg`, `.wav`, `.mp3`, `.txt`, `.csv`, `.c`)
- **Goals Tracker:** `/home/vega/Documents/git/local/little-screen-clip/immediate goals:.yaml`

---

*This document serves as the canonical knowledge base and state-of-the-union for Little Screen Clip entering Phase 4.*
