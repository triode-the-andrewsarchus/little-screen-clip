# Little Screen Clip — System Architecture & Concept Plan (v1)

> **Document Version:** `rough_idea_ai_1.md` (also aliased as `ai_1.md`)  
> **Source References:** [rough_idea_vega_0.md](file:///home/vega/Documents/git/local/little-screen-clip/rough_idea_vega_0.md), [rough_idea_ai_0.md](file:///home/vega/Documents/git/local/little-screen-clip/rough_idea_ai_0.md)  
> **Key Refinement in v1:** Strict separation between the **Ambient Peripheral HUD** (Little USB Display) and the **Active Workbench** (Main Monitor Pop-up/Overlay).

---

## 1. Ergonomic Vision: The HUD vs. The Workbench

A fundamental ergonomic truth drives this version: **A small screen mounted off to the side (e.g. inside or beside a PC case) is fantastic for glancing, but terrible for focused interaction.** Having to turn your head, squint at tiny menus, or navigate complex dialogues on a secondary peripheral creates cognitive friction and neck strain.

Therefore, the system operates on a dual-surface paradigm:

```
┌───────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                     PRIMARY DESKTOP MONITOR                                       │
│                                                                                                   │
│   [Your Code Editor / DAW / Browser / Game]                                                       │
│                                                                                                   │
│   ┌───────────────────────────────────────────────────────────────────────────────────────────┐   │
│   │                        THE CRAFTING WORKBENCH (On-Demand Modal)                           │   │
│   │                                                                                           │   │
│   │   [H: In-Hand]   [S1: Snippet]   [S2: Vocal.wav]   [S3: Logo.png]   [Z: Undo]  (Mirror)   │   │
│   │  ───────────────────────────────────────────────────────────────────────────────────────  │   │
│   │   > Type action: trim silence____________________  [Context: Audio]                       │   │
│   │   [ Parameters: Threshold -40dB | Pad 50ms ]                                              │   │
│   │   [ Live Preview: 03:24 -> 02:11 (-1.13s) ]                                               │   │
│   │                                                          [Enter: Apply & Paste]  [Esc]    │   │
│   └───────────────────────────────────────────────────────────────────────────────────────────┘   │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘

                                            ▲ Sync via IPC / State Daemon
                                            ▼
┌───────────────────────────────────────────────────────────────────────────────────────────────────┐
│                        PERIPHERAL USB DISPLAY (1920×480 Trofeo Vision)                            │
│                                     THE AMBIENT HUD                                               │
│                                                                                                   │
│   [H: Vocal.wav (Trimmed)]  │  [S1: Code Snippet]  │  [S2: Vocal.wav]  │  [S3: Logo]  │  [Z: Undo] │
│   |||||||||||||||| 02:11    │  def transform()..   │  ||||||||| 03:24  │  [PNG 1.2M]  │  (Old H)  │
│   * Active: Ready to Paste  │  * Py | 42 lines     │  * 44.1k | WAV    │  * 512x512   │           │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Surface Roles

| Surface | Display Type | Interaction | Purpose |
| :--- | :--- | :--- | :--- |
| **Little Screen (1920×480)** | USB Framebuffer (`0416:5408`) | **Passive / Ambient** (Zero interactive clutter, no menus, no prompts) | **"What am I holding right now?"** Glanceable inventory reference without consuming desktop monitor real estate. |
| **Main Monitor Overlay** | Desktop GUI Modal (Wayland/X11) | **Active / Focused** (Keyboard-first fuzzy palette, diffs, node recipes) | **"What do I want to do with it?"** Complete crafting table, slot manager, and live transform preview in direct field of view. |

---

## 2. Surface 1: The Ambient HUD (Little USB Screen)

### 2.1 Design Philosophy: Zero Clutter, Instant Glanceability
* **Never displays menus, dropdowns, or execution prompts.**
* **Never captures mouse cursor or OS focus.**
* Displays exclusively the **state of inventory slots**:
  * `[H]` **In-Hand**: Highlighted hero card (active clipboard payload).
  * `[S1]`–`[S6]` **Hotbar**: Current items stored in quick-access slots.
  * `[Z]` **Undo**: The item that was replaced in `[H]` by the last operation or swap.
  * **Status Pill**: Minimal status indicator (e.g. `SYNCED`, `LOCKED`, or subtle flash when updated).

### 2.2 Dedicated Visual Previews per MIME Type
Every slot renders a card optimized for the 1920×480 aspect ratio (e.g., 5–7 cards across the ultra-wide layout):

| Content Type | Glanceable Representation on USB Screen |
| :--- | :--- |
| **Plaintext / Code** | Syntax-highlighted 4–6 line preview, language badge, line count, char count. |
| **URLs** | Clean favicon/domain pill (`youtube.com`), path summary, tracking parameter indicator. |
| **Images / Screenshots** | Crisp thumbnail fitted with aspect preservation, dimension badge (`1920×1080`), file size. |
| **Audio** | Peak/RMS waveform envelope, duration timestamp (`01:45`), sample rate badge (`48kHz`). |
| **Video** | Extracted representative keyframe with playhead pill and duration. |
| **MIDI** | Mini piano-roll note cluster preview, track count, bar count, tempo. |
| **File Batches** | Extension badge (`.pdf`), primary filename, item counter badge (`+14 files`), total size. |
| **Directories** | Folder icon, folder name, child item count, total recursive size. |
| **Colors** | Solid color block fill, Hex code (`#7C3AED`), RGB/HSL values. |

---

## 3. Surface 2: The Crafting Workbench (Main Monitor)

When the user wants to organize slots, combine items, or transform data, they invoke the **Crafting Workbench** via global hotkey (e.g., `Super+Space`, `Super+V`, or a dedicated button on an Adafruit Macropad / Stream Deck).

### 3.1 The 3-Tier Workbench Layout

```
┌───────────────────────────────────────────────────────────────────────────────────────────────────┐
│ [1: H] In-Hand         [2: S1] Audio       [3: S2] Code         [4: S3] Image       [5: Z] Undo   │
│  "Clean Vocal.wav"      "Backing.wav"       "parse.py"           "cover.png"         "Raw.wav"    │
│  [ SELECTED (Arg 1) ]  [ SELECTED (Arg 2)]                                                        │
├───────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                   │
│  > Action: mix__________________________________________________________________________________  │
│                                                                                                   │
│  Suggested Recipes for [Audio + Audio]:                                                           │
│  * Mix (Simultaneous Playback)                  [Enter]                                           │
│  * Concatenate (Sequential Append)              [Alt+C]                                           │
│  * Crossfade 2.0s                               [Alt+X]                                           │
│  * Compare Durations & Sample Rates             [Alt+D]                                           │
│                                                                                                   │
├───────────────────────────────────────────────────────────────────────────────────────────────────┤
│  PREVIEW & PARAMETERS:                                                                            │
│  Ratio: [H: 100%] [S1: 85%]  |  Align: [Start]  |  Output: 48kHz Stereo WAV                       │
│  Result Duration: 03:42 (Matches [S1])                                                            │
│                                                                                                   │
│  [Enter] Execute -> Put in [H] & Dismiss      [Ctrl+Enter] Execute & Paste      [Esc] Cancel      │
└───────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Key Capabilities of the Workbench

1. **Integrated HUD Mirror**:
   - The top strip of the modal mirrors the exact state of the Little Screen.
   - You never need to look down/away while working; your cursor and selection states are visible in high-resolution directly in your line of sight.
2. **Context-Aware Action Palette**:
   - Fuzzy search powered by action tags (e.g., typing `trim`, `slug`, `resize`, `norm`, `json`).
   - Dynamically filters available operations based on the types of the selected slots:
     - If `[H]` is text $\rightarrow$ Text operations appear first.
     - If `[H]` is audio $\rightarrow$ Audio filters (gain, normalize, trim silence) appear first.
     - If `[H]` + `[S1]` are selected $\rightarrow$ Dual-input "crafting" operations (concat, diff, stitch, mix) appear.
3. **Live Diff / Preview Panel**:
   - Before committing destructive or unexpected changes, preview the outcome:
     - Text: Side-by-side or inline diff.
     - Audio: Duration change, waveform overlay, or loudness delta.
     - Images: Before/after split or resolution change.
4. **Execution & Paste Shortcuts**:
   - `Enter`: Commits the transform to `[H]`, saves previous `[H]` to `[Z]`, closes modal, updates Little Screen.
   - `Ctrl+Enter` ("Craft & Paste"): Commits transform to `[H]`, closes modal, and immediately sends `Ctrl+V` to the underlying app.
   - `Esc`: Discards and closes with zero changes.

---

## 4. Slot Dynamics & Mental Model

```
   [System Clipboard] ──(Ctrl+C)──> [H: IN-HAND] ──(Ctrl+V)──> [Application]
                                        │   ▲
                     (Transform / Swap) │   │ (Undo [Z])
                                        ▼   │
                                    [Z: UNDO]
                                        
             [H] ◄──────(Swap / Store / Load)──────► [S1] .. [S6] (Hotbar)
```

### 4.1 The Slot Rules
* **Rule 1 (The Hand Rule):** `[H]` is always the primary clipboard. Whatever is in `[H]` is what pastes when you hit `Ctrl+V`. Any standard copy action (`Ctrl+C`) anywhere in the OS replaces `[H]`.
* **Rule 2 (The Safety Net):** Any operation that replaces `[H]` pushes the previous contents of `[H]` into `[Z]` (Undo). Pressing `Undo` swaps `[H]` and `[Z]`.
* **Rule 3 (The Staging Rule):** Files and large datasets are stored as immutable references (Path + Metadata). Transforms do not mutate original files on disk; results are written to a temporary staging cache (`~/.cache/little-screen-clip/staging/`), preventing accidental data corruption.

---

## 5. Operations Catalog ("Crafting Recipes")

### 5.1 Unary Operators (Targeting `[H]`)

| Category | Operation | Input Type | Description |
| :--- | :--- | :--- | :--- |
| **Text** | `Trim Whitespace` | Text | Trims edges, collapses excessive internal blank lines. |
| **Text** | `Change Case` | Text | Cycles `snake_case` $\rightarrow$ `camelCase` $\rightarrow$ `kebab-case` $\rightarrow$ `UPPER`. |
| **Text** | `Format JSON` | Text | Pretty-prints JSON (or minifies if already formatted). |
| **Text** | `Clean URL` | URL | Strips tracking queries (`utm_*`, `fbclid`, `si`), converts shortlinks. |
| **Text** | `Sort & Dedupe` | Lines | Alphabetical/numerical sort and duplicate removal. |
| **Text** | `Slugify` | Text | Converts string into URL/filename friendly slug. |
| **Text** | `Base64 / Hex` | Text/Binary | Encode or decode Base64 / Hex strings. |
| **Images** | `Rotate / Flip` | Image | 90° CW/CCW, 180°, horizontal/vertical mirror. |
| **Images** | `Crop Square` | Image | Center-crops to 1:1 aspect ratio. |
| **Images** | `Compress / WebP` | Image | Downscales or re-encodes to high-efficiency WebP/JPEG under 1MB. |
| **Images** | `Strip EXIF` | Image | Clears all metadata/location tags for privacy. |
| **Images** | `OCR to Text` | Image | Extracts textual content from screenshot into `[H]` as plaintext. |
| **Audio** | `Trim Silence` | Audio | Auto-detects noise floor and crops start/end dead air. |
| **Audio** | `Normalize Loudness` | Audio | EBU R128 (-14 LUFS) or Peak normalization. |
| **Audio** | `Gain ±3dB` | Audio | Quick volume adjustment. |
| **Audio** | `Fade In/Out` | Audio | Applies smooth 50ms anti-click ramps to ends. |
| **Files** | `Batch Rename` | File List | Applies prefix, suffix, sequential numbering, or lowercasing. |
| **Files** | `Compress Archive`| File List | Packages items into a `.zip` or `.tar.zst` in staging. |
| **Files** | `Hash Rename` | File | Renames file to its SHA-256 digest while keeping extension. |
| **MIDI** | `Transpose` | MIDI | Shifts notes ±1 semitone or ±1 octave. |
| **MIDI** | `Quantize` | MIDI | Snaps note onsets to grid (1/16th, 1/8th). |

### 5.2 Multi-Input Operators (Targeting `[H] + [S_k]`)

| Category | Formula | Outcome in `[H]` |
| :--- | :--- | :--- |
| **Text** | `[H] + [S1] Concat` | Appends text (configurable delimiter: newline, space, comma, path separator). |
| **Text** | `[H] vs [S1] Diff` | Unified diff patch showing lines added/removed. |
| **Images** | `[H] + [S1] Stitch` | Joins two images side-by-side or vertically stacked. |
| **Images** | `[H] vs [S1] Diff Mask` | Highlight map showing pixel discrepancies between two iterations. |
| **Audio** | `[H] + [S1] Append` | Sequential crossfade concatenation. |
| **Audio** | `[H] + [S1] Mix` | Simultaneous audio blend with balance control. |
| **Files** | `[H] vs [S1] Compare` | Fast binary check / hash verification report. |
| **MIDI** | `[H] + [S1] Merge` | Combines tracks or sequences into a single multi-track MIDI file. |

---

## 6. System Architecture & Tech Stack

```
                                 ┌─────────────────────────────────────────┐
                                 │            CLIPBOARD MANAGER            │
                                 │     (Wayland wl-clipboard / X11)        │
                                 └────────────────────┬────────────────────┘
                                                      │ (Clip Events)
                                                      ▼
┌───────────────────────────┐    ┌─────────────────────────────────────────┐
│     INPUT LISTENERS       │    │           CORE STATE DAEMON             │
│ • Global Shortcuts (evdev)│───►│ • Slots: [H], [S1..S6], [Z]             │
│ • Macropad / MIDI / Knobs │    │ • Metadata & Thumbnail Extractor        │
│ • CLI (`clipctl`)         │    │ • Reference & Staging Cache             │
└───────────────────────────┘    └──────┬───────────────────────────┬──────┘
                                        │                           │
                   (IPC: State & Events)│                           │ (Dirty Buffer)
                                        ▼                           ▼
        ┌─────────────────────────────────────────┐   ┌─────────────────────────────────────┐
        │       MAIN MONITOR WORKBENCH (GUI)      │   │     USB FRAMEBUFFER RENDERER        │
        │ • PySide6 / Qt Overlay (Wayland Layer)  │   │ • Pillow / Cairo 1920x480 Pipeline  │
        │ • Slot Mirror Strip                     │   │ • Specialized Mime Card Layout      │
        │ • Fuzzy Command & Crafting Palette      │   │ • LY Bulk USB Streaming Engine      │
        │ • Live Diff & Parameter Preview         │   └──────────────────┬──────────────────┘
        └─────────────────────────────────────────┘                      │ (Raw Bulk Packets)
                                                                         ▼
                                                      ┌─────────────────────────────────────┐
                                                      │  Thermalright Trofeo (0416:5408)    │
                                                      └─────────────────────────────────────┘
```

### 6.1 Recommended Tech Stack
* **Language:** Python 3.11+ (or modular Rust backend with Python scripting).
* **Display Streaming:** `pyusb` / `libusb` with the Thermalright LY bulk protocol (chunked bulk transfer + keepalive loop).
* **Framebuffer Generation:** `Pillow` / `PySide6.QtGui` for clean typography, anti-aliased geometry, and fast thumbnail rendering.
* **Workbench GUI:** `PySide6` (Qt6):
  * Supports Wayland layer-shell protocol (renders seamlessly on top of apps without stealing desktop focus improperly).
  * Extremely fast launch time and hardware-accelerated rendering.
  * Shared visual components between the main monitor mirror and the USB display renderer.
* **Media Processing Engines:**
  * Audio: `numpy` + `pydub` / `soundfile` / `ffmpeg` for waveform peaks and transforms.
  * Images: `Pillow` + `imagehash`.
  * Text/Diff: Built-in `difflib` + `pygments` (syntax highlighting).
  * MIDI: `mido` / `pretty_midi`.

---

## 7. Phased Implementation Roadmap

```
  ┌──────────────────────────────────────────────────────────────────────────┐
  │ Phase 1: The Ambient USB Mirror (Trofeo Display Driver)                  │
  │ • Write USB LY-bulk streaming driver for 0416:5408 Winbond device        │
  │ • Build 1920x480 static framebuffer layout generator (Slots H, S1-S6, Z) │
  │ • Hook into OS clipboard (Wayland/X11) to update [H] on copy             │
  │ • Render glanceable cards (Text snippets, Image thumbs, Audio waveforms) │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 2: The Workbench Modal (Main Screen Overlay)                       │
  │ • Build PySide6 pop-up overlay with mirror header of the slots           │
  │ • Add keyboard navigation to inspect and swap slots                      │
  │ • Implement fuzzy command palette with the first 15 core unary transforms│
  │ • Implement 1-step Undo [Z] system                                       │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 3: Multi-Slot "Crafting" & Hardware Macro Control                  │
  │ • Add dual-input operations (Concat, Diff, Mix, Image Stitch)            │
  │ • Add bindings for Adafruit Macropad RP2040 / MIDI controller knobs      │
  │ • Add headless CLI (`clipctl`) for terminal automation and scripts       │
  └─────────────────────────────────────┬────────────────────────────────────┘
                                        │
  ┌─────────────────────────────────────▼────────────────────────────────────┐
  │ Phase 4: Custom Recipe Pipelines & Visual Node Editor                    │
  │ • Allow saving multi-step recipes (e.g. "Clean URL -> Copy Title")       │
  │ • Visual node canvas embedded in the Workbench modal                     │
  │ • Script execution sandbox for custom Python/Bash filters                │
  └──────────────────────────────────────────────────────────────────────────┘
```

---

## 8. Summary of Improvements in v1

1. **Ergonomic Clarity**: The little screen is officially designated as a **read-only glanceable HUD**. No interaction fatigue.
2. **The Workbench Concept**: The main monitor is where all active manipulation, searching, diffing, and crafting happens, complete with a mirrored view of the slots.
3. **Streamlined Workflow**:
   * *Copy* $\rightarrow$ glance at little screen.
   * *Craft* $\rightarrow$ press hotkey $\rightarrow$ search action on main monitor $\rightarrow$ hit Enter $\rightarrow$ modal closes $\rightarrow$ little screen reflects new state.
   * *Paste* $\rightarrow$ standard `Ctrl+V`.

---

*This document represents iteration 1 (`rough_idea_ai_1.md`). As design choices become firm, we can begin scaffolding the Phase 1 USB display driver and framebuffer layout.*
