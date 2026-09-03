# Little Screen Clip — System Architecture & Concept Plan (v0)

> **Document Version:** `rough_idea_ai_0.md`  
> **Source Reference:** [rough_idea_vega_0.md](file:///home/vega/Documents/git/local/little-screen-clip/rough_idea_vega_0.md)  
> **Target Hardware:** 1920×480 USB Framebuffer Display (Thermalright Trofeo Vision 9.16" / Winbond `0416:5408`) + Linux Desktop (X11/Wayland)

---

## 1. Executive Summary & Vision

**Little Screen Clip** is an experimental clipboard and file juggling environment built on a physical, skeuomorphic mental model: **The Inventory & Hotbar**. 

Instead of an ephemeral, invisible clipboard or a clunky vertical history popup, data items (text, files, screenshots, audio, MIDI, URLs) exist as tangible objects in physical "slots" displayed on a dedicated secondary ultra-wide USB display (1920×480). 

Users can manipulate, transform, inspect, and combine items using single-key operations and multi-slot "crafting" formulas, with instant visual previews and non-destructive workflows.

```
+---------------------------------------------------------------------------------------------------+
|  [H] IN-HAND (Active)  |  [S1] Code Snippet  |  [S2] Screenshot  |  [S3] Vocal.wav  |  [Z] UNDO   |
|  "https://youtu.be/.." |  def transform()... |  [1920x1080 PNG]  |  [||||||||||||]  |  "prev_str" |
|  * Selected: CANONICAL |  * Py | 42 lines    |  * 1.2 MB         |  * 44.1k | 3.2s  |  (1-step)   |
+---------------------------------------------------------------------------------------------------+
```

---

## 2. Core Mental Model & Slot Mechanics

### 2.1 The Slot Topology

The workspace is organized into discrete, predictable slots:

| Slot Type | Identifier | Purpose & Behavior |
| :--- | :--- | :--- |
| **In-Hand** | `[H]` | The active clipboard object. Whatever is in `[H]` is what standard OS `Ctrl+V` pastes. Any new OS `Ctrl+C` immediately populates `[H]`. |
| **Hotbar Slots** | `[S1]` – `[S8]` | Fast-access storage banks (like a videogame inventory). Items can be swapped, stored, or selected as secondary inputs for operations. |
| **Undo / Previous** | `[Z]` | Automatically receives the pre-operation state of `[H]` whenever a transformation occurs. Pressing `Undo` swaps `[H]` and `[Z]`. |
| **Cursor / Focus** | `Target` | The currently highlighted slot. Navigation moves the focus across slots for inspection, swapping, or multi-slot operations. |

### 2.2 Non-Destructive Data Handling (Zero-Copy & Staging)

To solve the performance bottleneck of large files (e.g. multi-gigabyte videos, audio archives, large datasets):
1. **Reference-First**: Slots store a lightweight **Clip Item Descriptor** (`URI`, `MIME`, `Path`, `Metadata`, `Cached Preview`), not the full file payload in RAM.
2. **Lazy Materialization**: File operations do not clone large files eagerly. Transforms operate in a sandboxed staging directory (`~/.cache/little-screen-clip/staging/`) using copy-on-write (reflink/hardlinks where supported) or streaming pipes.
3. **In-Memory Payloads for Small Data**: Pure strings, URLs, and clipboard images (< 50MB) are kept in memory for zero-latency operations.

---

## 3. Hardware & Display Architecture (1920×480 USB Framebuffer)

### 3.1 Device Specifications & Communication

* **Device:** Winbond Electronics Corp. `0416:5408` (Thermalright Trofeo Vision 9.16" LCD).
* **Resolution:** `1920 × 480` pixels (4:1 horizontal ultra-wide aspect ratio).
* **Protocol:** Vendor bulk transfer protocol (**"LY Bulk"** via `pyusb` / `libusb` with keepalive packets).
* **Display Server Independence:** The display does **not** register as an X11/Wayland monitor. The daemon renders frames directly to an internal memory buffer and transmits raw byte chunks over USB.

### 3.2 Dynamic Visual Previews by Data Type

The 1920×480 canvas is subdivided into distinct slot viewports (e.g., 1 Active Hand viewport + 4-6 Hotbar viewports + Status/Info bar). Each slot renders a specialized visual card:

| Data Type | Visual Representation on 1920×480 Screen |
| :--- | :--- |
| **Plaintext / Code** | Syntax-highlighted snippet, truncated with ellipsis, line count, character count, encoding. |
| **URLs** | Clean domain badge, protocol icon, path preview, query parameter indicator. |
| **Images / Screenshots** | Aspect-ratio-preserved thumbnail, image resolution badge (`1920×1080`), file format, color palette swatch. |
| **Audio Files** | Rendered waveform profile (peak/RMS), duration (`03:24`), sample rate (`48 kHz`), channel configuration (Stereo/Mono). |
| **Video Files** | Extracted keyframe thumbnail + duration + resolution overlay. |
| **MIDI Files** | Mini piano-roll note density heatmap, bar count, track count, tempo/key signature. |
| **Single / Batch Files** | Primary file icon, filename, file size, batch item count (`+11 files`). |
| **Directories** | Folder icon, directory name, recursive item count, total directory size. |
| **Color Codes (Hex/RGB)** | Solid color fill swatch, color name, Hex/RGB/HSL values. |
| **Structured Data (JSON/YAML)** | Formatted key-value mini-tree or formatted snippet with validation indicator (Valid/Invalid). |

---

## 4. Operation Engine ("The Crafting System")

Operations manipulate what is held in `[H]` or combine `[H]` with one or more selected slots `[S_k]`.

### 4.1 Unary Operators (Single-Input: Acts directly on `[H]`)

```
Input: [H]  ───> [ Operation ] ───> [H] (Transformed)
                               └──> [Z] (Old [H] saved to Undo)
```

#### A. Text & String Transformations
1. **Trim & Whitespace**: Trim leading/trailing whitespace, collapse multiple spaces/newlines.
2. **Case Conversion**: `UPPERCASE`, `lowercase`, `Title Case`, `camelCase`, `snake_case`, `kebab-case`.
3. **Line Sorting & Deduplication**: Sort alphabetically/numerically, remove duplicates, reverse line order.
4. **Delimited Join / Split**: Split lines to CSV / Join CSV to lines / Join with custom delimiter.
5. **Code & Markup Escaping**: Escape/Unescape Shell, HTML entities, Markdown, JSON string.
6. **JSON Formatting**: Pretty-print (indented) $\leftrightarrow$ Minify (compact one-liner).
7. **Base64 & Hex**: Base64 encode $\leftrightarrow$ decode, Hex encode $\leftrightarrow$ decode.
8. **URL Sanitization**: Strip UTM/tracking parameters, decode percent-encoded URLs, canonicalize.

#### B. Image & Screenshot Transformations
9. **Rotation & Flip**: Rotate 90° CW / CCW, 180°, Mirror Horizontal, Mirror Vertical.
10. **Quick Crop & Aspect**: Crop to center square, trim transparent/solid borders.
11. **Scale & Format Convert**: Convert PNG $\leftrightarrow$ JPEG $\leftrightarrow$ WebP, compress to web-friendly size (< 1MB).
12. **Metadata Scrubbing**: Strip all EXIF / GPS / camera metadata.
13. **OCR to Text**: Extract text from clipboard screenshot using local lightweight OCR (Tesseract / rapidocr).
14. **Color Extraction**: Extract dominant hex color from image directly to clipboard.

#### C. Audio & Sound Transformations
15. **Trim Silence**: Detect start/end silence thresholds and crop.
16. **Normalize & Gain**: EBU R128 / Peak normalization, ±3dB / ±6dB quick gain shifts.
17. **Fade In / Out**: 50ms / 200ms anti-click micro-fades.
18. **Reverse Audio**: Playback reversal.
19. **Format / Sample Rate Match**: Quick convert to 44.1kHz / 48kHz 16-bit WAV for DAW/sampler compatibility.

#### D. Batch Files & File System
20. **Batch Renaming**: Prefix, Suffix, Sequential numbering (`file_01.ext`), Sanitize illegal characters.
21. **Extension Swap / Hash Rename**: Rename files to their SHA-256 hash or update extensions.
22. **Archive Compression / Extraction**: Pack selection into `.zip` / `.tar.gz` or extract in-place.
23. **Hash Calculation**: Calculate and copy SHA-256 / MD5 hash string.

#### E. MIDI Transformations
24. **Transposition**: Transpose ±1 octave, ±1 semitone.
25. **Velocity Scale / Humanize**: Fixed velocity (e.g. 100), scale 80%, randomize velocity ±10.
26. **Quantize Timing**: Snap note onsets to nearest 1/16th or 1/8th note.

---

### 4.2 Binary / Multi-Slot Operators ("Crafting Formulas")

```
Inputs: [H] (Hand) + [S_k] (Selected Slot) ───> [ Operation ] ───> [H] (Result)
                                                          └───> [Z] (Old [H] saved to Undo)
```

| Domain | Formula | Behavior |
| :--- | :--- | :--- |
| **Text** | `[H] + [S1] → Concat` | Combines text in Hand with Slot 1 (configurable separator: newline, space, comma). |
| **Text** | `[H] vs [S1] → Diff` | Generates a unified diff summary between the two text items. |
| **Image** | `[H] + [S1] → Stitch` | Stitches two images side-by-side (horizontal) or stacked (vertical). |
| **Image** | `[H] vs [S1] → Visual Diff` | Computes visual difference highlight mask between two versions. |
| **Audio** | `[H] + [S1] → Concat` | Joins two audio clips sequentially with crossfade. |
| **Audio** | `[H] + [S1] → Mix` | Mixes two audio tracks simultaneously (equal gain or auto-duck). |
| **Files** | `[H] vs [S1] → Compare` | Fast binary / hash comparison (Match / Mismatch report). |
| **MIDI** | `[H] + [S1] → Merge` | Merges MIDI tracks or appends measures sequentially. |

---

### 4.3 Slot & Inventory Management Actions

* **Store to Slot (`Shift + 1..8`)**: Copies current `[H]` into slot `[Sn]`.
* **Take from Slot (`Alt + 1..8` or Select + Enter)**: Pulls `[Sn]` into `[H]`, pushing previous `[H]` to `[Z]`.
* **Swap (`X`)**: Swaps `[H]` with currently selected `[Sn]`.
* **Undo / Revert (`U` or `Ctrl+Z`)**: Swaps `[H]` with `[Z]`.
* **Clear Slot (`Del`)**: Clears current slot.
* **History Scroll (`Wheel` / `PgUp-PgDn`)**: Cycles recent clipboard history through `[H]`.

---

## 5. Software Architecture & Implementation Plan

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           LITTLE-SCREEN-CLIP                            │
│                                                                         │
│  ┌───────────────────────┐         ┌─────────────────────────────────┐  │
│  │   OS Clipboard Hooks  │         │       Input / Control Layer     │  │
│  │  (wl-clipboard/xclip) │         │ (Global Hotkeys / Macropad/CLI) │  │
│  └───────────┬───────────┘         └────────────────┬────────────────┘  │
│              │ (Copy Event)                         │ (Actions/Keys)    │
│              ▼                                      ▼                   │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                     Core State & Slot Manager                     │  │
│  │  - Slot Array: [H], [S1]..[S8], [Z]                              │  │
│  │  - Non-destructive Cache & Staging Engine                        │  │
│  │  - Type Analyzer & Metadata Extractor                            │  │
│  └───────────┬──────────────────────────────────────┬────────────────┘  │
│              │                                      │                   │
│              ▼                                      ▼                   │
│  ┌────────────────────────┐        ┌─────────────────────────────────┐  │
│  │   Transform Engine     │        │   1920x480 Framebuffer Renderer │  │
│  │  - Unary/Binary Ops    │        │  - Dynamic Card Layout          │  │
│  │  - Subprocess Runners  │        │  - Waveform & Image Generator   │  │
│  │  - (Future Node Graph) │        │  - Active Focus Highlighting    │  │
│  └────────────────────────┘        └────────────────┬────────────────┘  │
│                                                     │ (Raw RGB Frames)  │
│                                                     ▼                   │
│                                    ┌─────────────────────────────────┐  │
│                                    │   USB Display Driver (LY Bulk)  │  │
│                                    │   Thermalright Trofeo (0416:5408│  │
│                                    └─────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

### 5.1 Component Breakdown

1. **Clip Engine (`clip_engine/`)**:
   - Monitors X11/Wayland clipboard selections (`CLIPBOARD` and `PRIMARY`).
   - Extracts mime-types (`text/plain`, `text/uri-list`, `image/png`, etc.).
   - Dispatches new entries to `[H]` without lag.

2. **Slot Store (`slot_store/`)**:
   - Manages state: slots `[H]`, `[S1..S8]`, `[Z]`, cursor index, multi-selection set.
   - Metadata extractors:
     - `text_meta`: line/word/char count, syntax detection.
     - `image_meta`: dimensions, thumbnail generation, color palette.
     - `audio_meta`: waveform peak arrays, duration, sample rate.
     - `file_meta`: size, permissions, hashes.

3. **Operations & Recipe Pipeline (`ops/`)**:
   - Modular registry of operators (`@register_op(name, inputs=[Type1], output=Type2)`).
   - Built-in pure-Python & standard CLI tools (`ffmpeg`, `imagemagick`, `tesseract`, `pydub`, `mido`).
   - Future extension point: Node graph executor for user-defined macro pipelines.

4. **Renderer & Layout Engine (`ui_renderer/`)**:
   - Fast 2D rendering pipeline using Pillow/Cairo to produce 1920×480 24-bit RGB / 16-bit RGB565 frames.
   - Component-based visual widgets (SlotCard, WaveformWidget, ThumbnailWidget, StatusBar).
   - Micro-animations / Flash highlight on transform execution.

5. **Hardware USB Driver (`usb_display/`)**:
   - Implements Thermalright Trofeo / Winbond `0416:5408` LY Bulk USB packet streaming.
   - Dirty-rectangle / Partial updates or full-frame push at ~10-20 FPS on state change.
   - Background keepalive loop to prevent hardware timeout.

6. **Control & Input Client (`control/`)**:
   - Global hotkey listener (via `evdev` / D-Bus / compositing shortcuts).
   - Companion support for hardware macro pads (Adafruit Macropad RP2040, MIDI controllers, stream decks).
   - Fast CLI interface (`clipctl hand swap 1`, `clipctl op trim`, etc.).

---

## 6. Phased Evolution Roadmap

```
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ Phase 1: Hardware Driver & Core UI (The "Mirror Hotbar")                 │
  │ • Establish stable USB 1920x480 streaming to Trofeo screen               │
  │ • Build base UI renderer (Slots H, S1-S4, Z + status bar)                │
  │ • Integrate Wayland/X11 clipboard listener                               │
  │ • Implement basic slot storage & swapping (H <-> S1-S4)                  │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 2: Core 25 Built-in Operators (The "Swiss Army Tool")              │
  │ • Implement Unary transforms (Text, Image, Audio, Files)                 │
  │ • Implement Dual-input operations (Concat, Diff, Stitch, Mix)            │
  │ • Add 1-step Undo [Z] system                                             │
  │ • Global Hotkey & Macropad bindings for one-touch execution              │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 3: Script & Subprocess Plugins (The "Extensibility Layer")         │
  │ • Support custom user scripts (Python, Bash) as slot filters             │
  │ • Fast CLI tool (`clipctl`) for scriptable external triggers             │
  │ • Auto-detection of custom MIME types                                    │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 4: Visual Node Pipeline Builder (The "Visual Macro Engine")        │
  │ • Desktop GUI node editor to chain transformations into recipes          │
  │ • Save recipe macros to hotbar hotkeys or dedicated macro slots          │
  └──────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Open Questions & Design Decisions for Discussion

1. **Slot Capacity & Layout Density**:
   - On a 1920×480 canvas, how many slots feel optimal?
   - *Option A:* 1 Large Active Hand (e.g. 640×480) + 4 Hotbar Slots (320×480 each).
   - *Option B:* Equal-width grid of 6 slots (320×480 each) with a distinct highlight frame on `[H]`.
   - *Option C:* Dynamic layout (Hand dominates center, hotbar slots flank left/right).

2. **Primary Input Mode**:
   - Should default interaction be pure global hotkeys (e.g., `Super+C`, `Super+V`, `Super+1..4`), a companion hardware macropad / knob, or a lightweight pop-up quick-menu on the main monitor?

3. **Audio / Waveform Generation Performance**:
   - For audio items, should waveform generation be done via fast peak extraction in Python, or a lightweight background worker to ensure zero delay when copying large sound files?

---

*This document serves as the collaborative baseline (`rough_idea_ai_0.md`). Feedback and iterations can increment to `rough_idea_vega_1.md` / `rough_idea_ai_1.md` as we refine the spec.*
