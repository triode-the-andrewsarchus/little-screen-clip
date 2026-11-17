# Native Dadaspec audio previews

Dadaspec is an optional mirrored stereo mel spectrogram. The upper half shows
left-channel energy with bass at the center; the lower half shows the right
channel with bass at the center. Mono is mirrored. Bass contributes red, mids
green, and highs blue. Each channel and color band is normalized independently:
these are perceptual thumbnails, not absolute-level measurements.

## Choosing the view

- **Files List, Columns, and Music:** View → Audio → Dadaspec. The preference is
  saved as `[file_manager] audio_dadaspec = true`; Waveform and Spectrum remain
  separate choices. The same preference selects Dadaspec in Shelf audio previews and constellation hover cards.
- **Rolling audio:** enable **Dadaspec** in the rolling strip header. This displays
  the frozen stereo snapshot; Refresh/F5 captures a new snapshot as before.
- **Sample rack layer editor:** enable **Dadaspec** above the trim canvas. It
  defaults to the Files audio-view preference and keeps trim/pan/zoom controls.
- **Little screen and its workbench mirror:** set `[audio] dadaspec = true` in
  `config.toml`, then use `clipctl reload` (or restart the daemon).
  This also supplies the initial choice for rolling audio previews. Reopen the
  workbench to load its mirror default after editing configuration; rolling views
  also have their own toggle.
  Existing waveform geometry remains the default.

Audio Studio in both desktop applications, including video audio, uses the
existing waveform. MIDI piano rolls, digital-decoding signal plots, and
frequency-analysis views retain their respective note/frequency displays.

## Implementation

`little-clip-core::audio::dadaspec` ports the algorithm from
[dada-bots/dadaspec](https://github.com/dada-bots/dadaspec), revision
`983e8dfa1ff8d2356a78171689d1eec6dac5d1aa`, under its
[MIT license](../crates/little-clip-core/LICENSE-dadaspec).

The implementation uses the existing Rust RealFFT and image dependencies: linear
resampling to 32 kHz, uncentered 2048-sample periodic Hann windows at hop 2048,
30 Slaney-normalized mel bins, upstream dB/gamma contrast and RGB normalization,
and Lanczos resizing. No Python, numpy, Pillow, soundfile, or Dadaspec subprocess
is used at runtime. Existing guarded decoding/preparation still handles inputs.

Files strips request their physical pixel width, rounded up to a 256-pixel
bucket and bounded to 4096. Short file previews overlap the same 2048-sample
windows, with a minimum hop of 64 samples, instead of stretching a handful of
time slices. One second at 32 kHz produces 469 slices instead of 15. Clips
shorter than one FFT window still have only one padded observation; overlap
does not remove the window's 64 ms time/frequency tradeoff. A ready strip stays
visible while a wider replacement loads. The canonical `DadaspecBuilder::new`
and planar renderer retain upstream's non-overlapping hop.

PCM packets are fed in blocks; F32 decoder buffers are borrowed and other
formats reuse conversion storage. Workers share immutable FFT coefficients
and the FFT plan, while each request owns its mutable buffers and source data.
`fearless_simd` accelerates windowing and mel projection, alongside RealFFT's
existing SIMD implementation. Core's DSP loops are optimized in development
builds as well as release builds. See [profiling results](dadaspec-performance.md)
for the remaining waveform gap and the exploratory GPU timings.

File strips stream decoded packets without retaining PCM. Rolling snapshots retain their
existing mono editing data while analyzing the original stereo channels on the
snapshot worker. The sample rack retains its thumbnail inside the existing
bounded audio cache. Sample-browser hover cards reuse their bounded hover worker
and follow the Files audio-view choice.

Analysis retains at most 65,536 stereo mel frames (about 15 MiB), merging adjacent
power frames for unusually long inputs (roughly 70 minutes and longer). Output is
bounded to 4096 × 1024. Browser decoding keeps the existing ten-second budget.
The little screen retains the existing full-track/snippet policy; Shelf collected
file references use a short excerpt. Images are premultiplied RGBA instead of lossy JPEG. Silence is fully transparent;
spectral energy controls alpha. The colors have a neutral brightness floor for
blue visibility on dark surfaces. Painting applies a darker tint on light
surfaces without changing hue or alpha, so cached images follow theme changes.
Full-energy primaries and mixed hues meet a 3:1 contrast floor on all standard
light and dark application surfaces. Faint energy remains appropriately translucent.

## Reference checks

The small uncompressed RGBA fixtures in `little-clip-core/src/audio/fixtures`
come from upstream's numpy pipeline, before JPEG and at the native mel/frame
resolution, using synthetic deterministic signals. Tests apply the same transparency/color presentation to the reference pixels,
then compare native-rate and
44.1 kHz resampled Rust output, and cover stereo separation, mono mirroring,
silence, short clips, cancellation, bounds, and arbitrary decoder packet sizes.

The reference fixtures can be regenerated with
`python scripts/dadaspec_reference_fixtures.py /path/to/dadaspec.py`.

Architecture review separated FFT normalization/compositing, decoder lifetime and
menu responsibilities. The remaining comparison warnings concern the new visual
choice branches in existing UI functions, stereo extraction in the existing
snapshot loop, and fixture initializers carrying the new optional image field.
These preserve existing ownership and behavior; no second audio cache, decoder
service, UI dependency in core, or application dependency in the file manager was
introduced.
