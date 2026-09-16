# Non-UI Architecture & Codebase Review Notes

This document records non-UI bugs, anomalies, performance bottlenecks, edge-case panics, threading hazards, and architectural inconsistencies identified across `clip-workbench` and related crates during the UI system architecture review.

---

## 1. `crates/clip-workbench/src/spreadsheet_view.rs`

### 1.1 Frame-Rate Memory Allocation of Entire TSV Buffer (Performance / Memory Churn)
- **Location:** `crates/clip-workbench/src/spreadsheet_view.rs:75-77`
- **Detail:** In `render_ui`, on **every single frame** (~60–144 times per second), the entire active sheet's contents are formatted into a tab-separated string:
  ```rust
  let current_tsv = self.current_sheet().map(|sheet| {
      sheet.rows.iter().map(|r| r.join("\t")).collect::<Vec<_>>().join("\n")
  });
  ```
- **Impact:** For medium-to-large spreadsheets (thousands of rows), this causes continuous large heap allocations and garbage collection pressure on every frame, even when the user is simply moving the mouse or reading cells.
- **Recommendation:** Defer TSV generation until the "📋 Copy Table" button is actually clicked.

---

## 2. `crates/clip-workbench/src/pdf_view.rs`

### 2.1 Synchronous Multi-Page PDF Rendering Freezes Main Thread (Responsiveness)
- **Location:** `crates/clip-workbench/src/pdf_view.rs:49-60`, `76-110`
- **Detail:** In `PdfEditorState::new` and `reload_pages`, all pages `1..=info.page_count` are rasterized synchronously in a blocking loop via `render_pdf_page_preview(&path, p, 360)` (which invokes `pdftoppm` / ImageMagick subprocesses).
- **Impact:** Opening any multi-page PDF (e.g. 20+ pages) freezes the entire workbench UI for multiple seconds.
- **Recommendation:** Load page count and metadata immediately, but render thumbnails asynchronously on background worker threads (or on-demand as pages scroll into view), matching the thumbnail pattern in `ImagePreviewState`.

### 2.2 Retaining Uncompressed PNG Bytes in Memory Post-Texture Upload (Memory Waste)
- **Location:** `crates/clip-workbench/src/pdf_view.rs:14`, `177-193`
- **Detail:** `PdfPageItem` permanently stores `png_bytes: Vec<u8>` even after the texture has been created and uploaded to the GPU via `ctx.load_texture`.
- **Recommendation:** Drop `png_bytes` once the `TextureHandle` is acquired.

---

## 3. `crates/clip-workbench/src/midi_view.rs`

### 3.1 Synchronous Synthesis of Entire Song on UI Thread (Responsiveness)
- **Location:** `crates/clip-workbench/src/midi_view.rs:154-158`
- **Detail:** When Play is triggered:
  ```rust
  if self.wav_cache.is_none() {
      self.wav_cache = Some(self.data.synthesize_to_wav());
  }
  ```
  `synthesize_to_wav` software-synthesizes all tracks and notes for the entire song duration synchronously on the egui main thread.
- **Impact:** Noticeable hitch or multi-second audio latency when clicking Play on complex or long MIDI compositions.
- **Recommendation:** Synthesize asynchronously on a background thread with an animated progress/loading spinner, or stream synthesized chunks.

---

## 4. `crates/clip-workbench/src/stl_view.rs`

### 4.1 Frame-Rate Dependent Auto-Rotation Speed (Bug)
- **Location:** `crates/clip-workbench/src/stl_view.rs:80-83`
- **Detail:**
  ```rust
  if self.auto_rotate {
      self.yaw += 0.015;
      ui.ctx().request_repaint();
  }
  ```
  Rotation angle increment is added per rendered frame rather than scaled by delta time (`ctx.input(|i| i.stable_dt)`).
- **Impact:** On a 144Hz or 240Hz gaming display, the 3D model spins 2.4x to 4x faster than on a standard 60Hz display.
- **Recommendation:** Multiply the angular speed by delta seconds: `self.yaw += 0.9 * dt;`.

---

## 5. `crates/clip-workbench/src/audio_preview.rs`

### 5.1 Potential Panic on Stereo Analysis Fallback (`unwrap()` on `ch2_samples`) (Crash Hazard)
- **Location:** `crates/clip-workbench/src/audio_preview.rs:3494`
- **Detail:**
  ```rust
  let is_stereo = self.channels >= 2;
  ...
  (base, half * 0.45, analysis.ch2_samples.as_ref().unwrap(), accent_base)
  ```
  If an audio file header indicates 2 channels but decoding or fallback analysis produces mono data (or `ch2_samples` is `None`), calling `.unwrap()` causes an unhandled panic and crashes the workbench.
- **Recommendation:** Safely fall back to `ch1_samples` or return early if `analysis.ch2_samples.is_none()`.

### 5.2 Silent Failure When `zenity` Is Missing (UX Degradation)
- **Location:** `crates/clip-workbench/src/audio_preview.rs:1039-1056`
- **Detail:** Unlike `image_preview.rs` (which tests for `zenity` presence and provides an in-app path fallback modal), `audio_preview.rs` blindly invokes `zenity` in a background thread. If `zenity` is not installed, it silently returns `None` with zero user feedback or fallback.
- **Recommendation:** Use a shared file/directory picker helper with an in-app fallback dialog.

---

## 6. `crates/clip-workbench/src/image_preview.rs`

### 6.1 Synchronous Subprocess Execution on UI Thread for Save Dialog Detection (Lag Spike)
- **Location:** `crates/clip-workbench/src/image_preview.rs:2422-2423`, `2470-2471`
- **Detail:** In `open_save_dialog`:
  ```rust
  let zenity_available = Path::new("/usr/bin/zenity").exists()
      || Command::new("zenity").arg("--version").output().is_ok();
  ```
  Executing `Command::new("zenity").arg("--version").output()` synchronously forks a process on the egui render thread.
- **Recommendation:** Probe `zenity` once lazily at startup using `which` / `PATH` check, or use `LazyLock<bool>`.

### 6.2 External App Escape Hatch Terminates Whole Workbench Process (Behavioral Inconsistency)
- **Location:** `crates/clip-workbench/src/image_preview.rs:2723`, `audio_preview.rs:479` vs `text_preview.rs:356-380`
- **Detail:** When launching Krita, Inkscape, or Audacity, both `image_preview.rs` and `audio_preview.rs` execute:
  ```rust
  ctx.send_viewport_cmd(egui::ViewportCommand::Close);
  ```
  This immediately terminates the workbench application. In contrast, `text_preview.rs` leaves the workbench open, and falls back to `xdg-open` if the configured editor fails.
- **Recommendation:** Standardize escape hatch behavior: keep the workbench running (or minimize/hide it if desired, but do not abruptly kill the process), and provide fallback `xdg-open` across all file types.

---

## 7. `crates/clip-workbench/src/binary_diff_view.rs`

### 7.1 Race Condition in Background Diff Recomputation (Thread Hazard)
- **Location:** `crates/clip-workbench/src/binary_diff_view.rs:69-73`, `100-116`
- **Detail:** When `recompute()` is called (e.g. on rapid mode toggles or operand swaps), a new thread is spawned with a cloned sender. There is no cancellation token or generation ID. If an older computation finishes after a newer one, its stale result can overwrite `self.result`.
- **Recommendation:** Attach an incrementing `u64` generation/request token and discard messages from stale generations.

---

## 8. `crates/clip-workbench/src/canvas.rs`

### 8.1 Recipe File Name Collision via Case Insensitivity & Slugification (Data Loss)
- **Location:** `crates/clip-workbench/src/canvas.rs:73-77`
- **Detail:**
  ```rust
  let safe_name: String = sel_name
      .chars()
      .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
      .collect();
  let path = get_recipes_dir().join(format!("{}.yaml", safe_name.to_lowercase()));
  ```
  Recipes with names like "My-Recipe" and "My Recipe" or differing only in capitalization will map to the exact same YAML filename, silently overwriting each other.
- **Recommendation:** Add unique ID hashes or check collision before saving.

---

## 9. `crates/clip-workbench/src/diff_view.rs`

### 9.1 Unbounded Main-Thread Diff Layout Computation (UI Freeze)
- **Location:** `crates/clip-workbench/src/diff_view.rs:116-121`, `181`
- **Detail:** `compute_diff` and `ansi_to_layout_job` run synchronously on the UI thread for text diffs. When diffing large files (several thousand lines), the entire diff algorithm and syntect color layout job are computed synchronously, causing visible frame drops.
- **Recommendation:** Offload text diffing and layout job construction to worker threads, similar to `spawn_image_diff_worker`.

---

## 10. `crates/clip-workbench/src/rolling_audio_strip.rs` & Audio Device Sharing

### 10.1 Competing Audio Sinks Across Preview Widgets (Resource Contention)
- **Location:** `crates/clip-workbench/src/rolling_audio_strip.rs:54-58`, `audio_preview.rs:400`
- **Detail:** Both `RollingAudioStripState` and `AudioPreviewState` unconditionally open default Rodio `MixerDeviceSink`s upon instantiation. On systems with ALSA or constrained PulseAudio/PipeWire sink limits, opening multiple sinks simultaneously can cause device lockouts or audio stutter.
- **Recommendation:** Share an application-level audio playback device sink across all workbench audio widgets.

---

## 11. `crates/clip-workbench/src/main.rs`

### 11.1 Hardcoded Display Mirror Coordinate Hit-Testing Disregards Dynamic Slot Configurations
- **Location:** `crates/clip-workbench/src/main.rs:693-713`
- **Detail:** Slot hit-testing coordinates on the 1920x462 preview mirror are hardcoded (`< 240.0`, `< 600.0`, etc.). If `config.toml` changes slot count or depths, the hit-testing areas do not match the rendered slots on screen.
- **Recommendation:** Derive mirror hit-test rects directly from the layout geometry computed in `little-clip-render`.

### 11.2 Continuous 250ms IPC Polling Loop
- **Location:** `crates/clip-workbench/src/main.rs:215-224`
- **Detail:** A dedicated OS thread sleeps 250ms and sends `IpcRequest::GetState` over Unix domain sockets indefinitely. While lightweight, it wakes the daemon CPU 4 times a second even when idle.
- **Recommendation:** Implement a notification/event push mechanism or backoff when workbench is unfocused.
