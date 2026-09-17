# Little Screen Clip 🖥️📎

An ambient, hardware-driven clipboard companion, dual-buffer workbench, and visual data pipeline engine designed for Linux Wayland desktops and dedicated secondary USB displays.

!> Note, currently the only tested/supported display is the Thermalright Trofeo Vision 9.16" 1920x480 display

---

## Overview & Architecture

Standard clipboard managers flatten everything into a single chronological list. **Little Screen Clip** introduces an asymmetric **Dual-Handed Workspace Model** paired with a physical 480×480 ambient screen:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          LITTLE SCREEN CLIP                             │
├───────────┬──────────────┬──────────────┬──────────────┬────────────────┤
│   UNDO    │  LEFT HAND   │  LEFT STACK  │ RIGHT STACK  │   RIGHT HAND   │
│  [ UNDO ] │  (Primary)   │ [LP1]  [LP2] │ [RP2]  [RP1] │  (Secondary)   │
│           │   Ctrl + C   │              │              │  Ctrl+Alt + C  │
└───────────┴──────────────┴──────────────┴──────────────┴────────────────┘
```

- **Left Hand (Primary Buffer)**: Continuously tracks your system OS clipboard via Wayland (`Ctrl+C` / `Ctrl+V`).
- **Right Hand (Secondary Buffer)**: An independent secondary clipboard slot captured via `Ctrl+Alt+C` and pasted via `Ctrl+Alt+V`. It does not clobber your primary OS clipboard.
- **History Stacks**: Independent 2-deep history stacks for both hands (`LeftPrior1/2` and `RightPrior1/2`).
- **Undo Slot**: Dedicated preservation slot holding previous data before transformations.
- **Crafting Workbench (`Mod+C`)**: An interactive GUI modal featuring:
  - **Quick Action Palette**: Fuzzy search for instantaneous transforms (case changes, JSON formatting, URL cleaning, hashing, extraction).
  - **The Big Node Machine**: Visual node canvas to build and execute multi-step pipelines (Python, Bash, ImageMagick, FFmpeg).
  - **Diff Viewers**: Interactive text/code diffs and Biodiff-style aligned binary hex comparisons.
  - **Big Text & Code Previewer (`Right-Click Text/Code Slot`)**: Full-screen inspection modal with Syntect syntax highlighting, rich Markdown rendering, toggleable line-wrapping, line numbers gutter, regex/find toolbar, text selection/copying, and configured editor escape hatch (`code`).
  - **Rolling Audio Strip**: Ambient 30s background audio buffer snapshot, waveform visualizer, and slicer.
  - **Picture Studio (`Right-Click Image Slot` / `Palette`)**: Interactive visual editor with lossless cropping (1:1, 16:9, 9:16), rotations/flips, privacy black-bar redaction, magnifying color loupe eyedropper, target pixel & file size optimizer, palette extraction, one-click escape hatches (Krita, Inkscape, Nomacs), and AI background removal (`nobg`).

---

## System Requirements

### Hardware
- **Display**: Trofeo Vision 2.1" 480×480 USB LCD display (Vendor ID: `0x0416`, Product ID: `0x5408` or `0x5020`).
  *(Note: If no USB display is detected, the daemon automatically runs in virtual mode without crashing).*
- **Permissions**: Ensure your user has permissions to access USB devices without root (add to `plugdev`/`uaccess` group, or see udev rules below).

### Software & Desktop Environment
- **Operating System**: Linux with a Wayland Compositor (e.g. **Niri**, Sway, Hyprland, Wayfire).
- **Core CLI Utilities**:
  - `wtype` — Wayland virtual keyboard simulation (for synthetic copy/paste).
  - `wl-clipboard` (`wl-copy`, `wl-paste`) — Wayland clipboard protocol tools.
  - `pipewire` / `pipewire-pulse` — for rolling system audio buffer capture (`pw-record`).
- **Rust Toolchain**: Rust edition 2021 (`cargo`, `rustc`).
- **Optional Tools for Pipeline Recipes & Picture Studio**:
  - `difftastic` (`difft`) — Structural semantic code diffing.
  - `ffmpeg` & `sox` — Advanced audio normalization and transcoding.
  - `imagemagick` (`magick` / `convert`) — Image framing, resizing, and composition.
  - `python3`, `bash`, `jq` — Custom node scripting.
  - `rsvg-convert` (`librsvg`) — Vector SVG thumbnailing and rasterization.
  - `krita`, `inkscape`, `nomacs` — External full-editor escape hatches.
  - `nobg` (`feyninc/FeyNobg`) — AI background removal (CUDA GPU-accelerated).

---

## Installation & Setup

### 1. Build Binaries

Compile the workspace in release mode:

```bash
cd /home/vega/Documents/git/local/little-screen-clip
./launch.sh build
```

This compiles:
- `little-clipd`: Background ambient hardware daemon & clipboard observer.
- `clipctl`: Command-line controller and IPC interface.
- `clip-workbench`: GUI modal for transforms, node pipelines, and diffs.

Symlinks to `~/.local/bin/` can be created automatically:
```bash
mkdir -p ~/.local/bin
ln -sf "$(pwd)/target/release/clipctl" ~/.local/bin/clipctl
ln -sf "$(pwd)/launch.sh" ~/.local/bin/little-screen-clip
```

---

### 2. Install Systemd User Service

Little Screen Clip includes a dedicated systemd user unit that starts the daemon automatically with your graphical session.

Install and enable the service with one command:
```bash
./launch.sh install-service
```

This installs `little-screen-clip.service` to `~/.config/systemd/user/` and enables it for `graphical-session.target`.

#### Service Commands:
```bash
./launch.sh service start    # Start service via systemd
./launch.sh service stop     # Stop systemd service
./launch.sh service status   # View systemd service status
./launch.sh service logs     # Follow systemd journal logs
```

> [!TIP]
> **Development-Aware Lifecycle:**  
> When you run development commands like `./launch.sh start`, `./launch.sh restart`, or `./launch.sh test` manually from a terminal, the script automatically detects the active systemd service and stops it first. This ensures no conflicting daemon instances run during active development.

---

### 3. Optional: Set Up AI Background Removal (`FeyNoBg`)

The Picture Studio integrates `feyninc/FeyNobg` (a 0.3B parameter BiRefNet vision model) for instantaneous cutout generation. On NVIDIA GPUs with CUDA, inference completes in ~280ms.

If running on a fresh system, configure the dedicated environment:

```bash
# Create isolated venv sharing system PyTorch & CUDA packages
uv venv --python /usr/bin/python3 --system-site-packages ~/.local/share/little-screen-clip/nobg-venv

# Install nobg package
uv pip install --python ~/.local/share/little-screen-clip/nobg-venv/bin/python nobg
```

*(Note: If `nobg` is not installed, the workbench UI detects it gracefully and offers a 1-click `[📋 Copy Install Cmd]` button).*

---

### 4. Configure Compositor Keybindings (Niri)

Add the following binds to your `~/.config/niri/cfg/keybinds.kdl`:

```kdl
binds {
    // ─── Little Screen Clip ───
    // Launch Crafting Workbench Modal
    Mod+C hotkey-overlay-title="Little Screen Clip: Workbench" {
        spawn "/home/vega/Documents/git/local/little-screen-clip/launch.sh" "ui";
    }

    // Capture active window selection directly to Right-Hand Buffer
    CTRL+ALT+C hotkey-overlay-title="Little Screen Clip: Copy to Right Hand" {
        spawn "/home/vega/Documents/git/local/little-screen-clip/launch.sh" "copy-right";
    }

    // Paste content from Right-Hand Buffer into active window
    CTRL+ALT+V hotkey-overlay-title="Little Screen Clip: Paste from Right Hand" {
        spawn "/home/vega/Documents/git/local/little-screen-clip/launch.sh" "paste-right";
    }
}
```

Reload Niri config without restarting your session:
```bash
niri msg action load-config-file
```

---

## Dual Clipboard Workflow

| Shortcut | Action | Target Buffer | Notes |
| :--- | :--- | :--- | :--- |
| <kbd>Ctrl</kbd> + <kbd>C</kbd> | Standard Copy | **Left Hand** | Normal OS clipboard. Synchronizes with physical display Left Hand. |
| <kbd>Ctrl</kbd> + <kbd>V</kbd> | Standard Paste | **Left Hand** | Pastes current Left Hand content into active window. |
| <kbd>Ctrl</kbd> + <kbd>Alt</kbd> + <kbd>C</kbd> | Right Copy | **Right Hand** | Captures selection to Right Hand buffer. Preserves Left Hand in OS clipboard. |
| <kbd>Ctrl</kbd> + <kbd>Alt</kbd> + <kbd>V</kbd> | Right Paste | **Right Hand** | Pastes Right Hand buffer into active window without losing Left Hand. |
| <kbd>Mod</kbd> + <kbd>C</kbd> | Open Workbench | **Both Hands** | Opens GUI modal with Left Hand & Right Hand ready for diffs/pipelines. |

---

## CLI Management (`clipctl` & `launch.sh`)

### Using `./launch.sh`:
```bash
./launch.sh start           # Start little-clipd in background
./launch.sh stop            # Stop little-clipd
./launch.sh restart         # Restart daemon
./launch.sh status          # View status and current 7-slot buffer state
./launch.sh ui              # Open Crafting Workbench modal
./launch.sh diff            # Open text / image diff viewer
./launch.sh bdiff           # Open binary hex diff viewer
./launch.sh copy-right      # Capture selection to Right Hand buffer
./launch.sh paste-right     # Paste Right Hand buffer
./launch.sh reload          # Hot-reload config.toml
./launch.sh logs            # Tail daemon log file
./launch.sh test            # Run direct USB hardware test
```

### Using `clipctl`:
```bash
clipctl status                       # View slots table
clipctl push "text" --hand right     # Push text to Right Hand
clipctl swap left right              # Swap Left Hand and Right Hand
clipctl undo                         # Revert last Left Hand operation
clipctl copy --hand right            # Trigger synthetic capture to Right Hand
clipctl paste --hand right           # Paste from Right Hand
clipctl op diff                      # Run operation between LH and RH
clipctl recipe list                  # List available pipeline recipes
clipctl recipe run pad_and_caption   # Run visual pipeline recipe
clipctl reload                       # Hot-reload display configuration
clipctl lock                         # Blank physical screen (sleep/lock mode)
clipctl unlock                       # Restore physical screen
```

---

## Configuration (`config.toml`)

Edit `config.toml` in the project root (or `~/.config/little-screen-clip/config.toml`):

- `[display]`: Screen rotation (180° for bottom-cable mounting), debug badge toggle, animation FPS limiter.
- `[colors]`: Full 24-bit hex color theme for all 7 card slots, borders, highlights, and text previews.
- `[audio]`: Rolling waveform analysis thresholds and snippet lengths.
- `[workbench]`: Preferred external text editor for script nodes.

Reload configuration live without restarting:
```bash
./launch.sh reload
```

---

## Troubleshooting

1. **USB Display Permission Denied:**
   Create `/etc/udev/rules.d/99-trofeo-screen.rules`:
   ```udev
   # Winbond Electronics Corp. USBDISPLAY / Trofeo Vision Display
   SUBSYSTEM=="usb", ATTR{idVendor}=="0416", ATTR{idProduct}=="5408", MODE="0666", TAG+="uaccess"
   SUBSYSTEM=="usb", ATTR{idVendor}=="0416", ATTR{idProduct}=="5020", MODE="0666", TAG+="uaccess"
   ```
   Reload udev rules:
   ```bash
   sudo udevadm control --reload-rules && sudo udevadm trigger
   ```

2. **Daemon says "Socket already in use":**
   Run `./launch.sh stop && ./launch.sh start`. Stale sockets in `$XDG_RUNTIME_DIR/little-screen-clip.sock` are automatically cleaned up.

3. **Development vs Systemd Service:**
   If you ever want to switch permanently back to systemd:
   ```bash
   ./launch.sh service restart
   ```
# Dependency update policy

Git dependencies are pinned to an immutable revision in their crate manifest. Update a pin only
with its corresponding Cargo.lock change and a review of the upstream release or commit.
