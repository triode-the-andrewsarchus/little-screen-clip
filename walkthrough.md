# Walkthrough: Rolling System Audio Capture & Interactive Workbench Waveform

We have implemented, refined, tested, and verified **Continuous Rolling System Audio Capture** and its interactive waveform strip directly inside Little Screen Clip's Crafting Workbench.

---

## 1. Feature Highlights

1. **True System Audio Capture (Speaker Output Monitor)**:
   - Configured `pw-record` with `--raw --target @DEFAULT_SINK@ -P stream.capture.sink=true`.
   - Verified via `wpctl status`: PipeWire links stream inputs directly to `Game Audio:monitor_FL` and `Game Audio:monitor_FR` (the speaker output stream), with zero microphone leakage.
   - Sized for $N$ seconds of audio (default: `30s`, configurable via `audio.rolling_capture_duration_sec` in `config.toml`).
   - On IPC request `GetRollingAudio`, it takes a snapshot of the ring buffer and writes a standard 48 kHz stereo 16-bit PCM WAV file in RAM (`$XDG_RUNTIME_DIR/little-screen-clip/rolling_snapshots/current_snapshot.wav`).

2. **Waveform Peak Alignment & 100% Byte-Accurate Playback**:
   - **Fixed 2x/4x Pyramidal Time Scaling**: Resolved mismatch in `RollingAudioStripState::query_peaks` where block divisors were hardcoded to `512` and `4096` instead of `1024` and `16384` (matching `build_peak_pyramid`), which previously compressed time by 2x/4x and caused rendered peaks to appear at completely wrong timestamps.
   - **Pre-Sliced Sample-Accurate Playback**: Slices `s..e` using `slice_wav` directly into `$XDG_RUNTIME_DIR/little-screen-clip/rolling_slices/preview.wav` and streams from position 0.0s, eliminating asynchronous seeking deadlocks and packet-granule seek offsets in Rodio.
   - **48,000 Hz Native Parity**: Configured `pw-record` and `RollingAudioBuffer` to run natively at 48 kHz matching the system's PipeWire graph (`clock.force-rate = 48000`).
   - **Frame-Aligned Circular Writes**: Enforced frame boundary alignment in `push_pcm_s16le` and `push_samples` to prevent any possibility of channel swap on circular buffer wrap.

3. **Dedicated Focus Isolation on the Waveform Strip**:
   - When a region on the rolling audio strip is selected or being dragged:
     - Keyboard focus is explicitly assigned to the waveform canvas.
     - The search `TextEdit` input is set to non-interactive (`interactive(false)`) and releases focus so no keystrokes reach it.
     - The canvas border is highlighted in vivid emerald green (`rgb(52, 211, 153)`).
   - Hitting `Space` immediately plays/pauses the selected audio without typing into or clearing the search text box.
   - Hitting `Ctrl+C` slices the audio and copies it to `Left Hand` (and system clipboard).
   - Hitting `Escape` clears the selection, seamlessly returning focus to the search bar.
   - Clicking into the search bar dismisses the selection and focuses the search bar.

4. **Interactive Colored Waveform Strip (`clip-workbench`)**:
   - Multi-band **Serato DJ 3-band colored waveform** (warm red/orange for bass, bright green/yellow for mids, electric cyan for highs) generated using `build_peak_pyramid`.
   - Time tick markers: `-30s`, `-20s`, `-10s`, `0s (NOW)`.
   - Region selection via click-and-drag directly on the waveform canvas.
   - Translucent highlighted selection box with emerald boundaries.
   - Live moving playhead with indicator marker during playback.

5. **Right-Click for Full Audio Editor**:
   - Right-clicking anywhere on the waveform strip transitions directly into the full-featured audio editor overlay (`AudioPreviewState`) with the snapshot WAV and region pre-selected.

---

## 2. Verification & Validation Results

### 2.1 Audio Stream Monitor Link
Verified with `wpctl status` while `little-clipd` is running:
```
 └─ Streams:
       224. pw-record                                                   
            190. monitor_FL     
            294. input_FR        < Game Audio:monitor_FR	[active]
            308. input_FL        < Game Audio:monitor_FL	[active]
            321. monitor_FR     
```
`pw-record` inputs are strictly connected to `Game Audio:monitor_FL` and `Game Audio:monitor_FR` (system audio output) at 48000 Hz.

### 2.2 Automated Workspace Tests
All 102 tests in the workspace pass:
- `little_clip_core`: 31 tests passed
- `clip_workbench`: 31 tests passed
- `little_clip_ops`: 33 tests passed
- `little_clip_render`: 7 tests passed

### 2.3 Binaries Built & Daemon Live
- Release binaries built (`./launch.sh build`).
- Daemon running in background (`./launch.sh restart`, PID 1490358).
- Verified via `./target/release/clipctl rolling-audio` (48000Hz, 2ch).
