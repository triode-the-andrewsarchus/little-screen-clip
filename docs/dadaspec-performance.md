# Dadaspec performance and strip resolution

Measured on a Ryzen 9 5900XT with an RTX 3090 on 2026-11-16. CPU results use
release builds, one CPU affinity, an owned deterministic stereo PCM16 WAV,
warm filesystem cache, one warmup and nine measured repetitions. These are
medians for the actual guarded streaming decoder/renderer. Metadata/cover
subprocesses, worker queue waits and UI painting are excluded. Other desktop
workloads remained active; small differences should not be overinterpreted.

## Current waveform gap

The waveform column uses the existing 512-bin renderer. Dadaspec below uses a
1024-pixel strip with adaptive overlapping windows, as requested by a typical
Files row. All times are milliseconds.

| Clip | Waveform | Dadaspec | Extra time |
| --- | ---: | ---: | ---: |
| 1 s, 48 kHz | 0.37 | 4.59 | 4.22 |
| 10 s, 48 kHz | 3.50 | 10.71 | 7.21 |
| 60 s, 48 kHz | 22.69 | 26.75 | 4.06 |
| 300 s, 44.1 kHz | 101.44 | 126.06 | 24.62 |

At the same 512-pixel output size, the old/new Dadaspec medians were 42.74/27.34
ms for 60 seconds and 229.74/131.53 ms for five minutes: about 36–43% less time.
Short clips deliberately spend some of that saving on detail. A one-second
strip now has 469 overlapping observations instead of 15, and takes 3.75 ms at
512 pixels versus the previous 1.39 ms. This is a frequency-energy thumbnail;
even a dense spectrogram cannot replace a waveform's sample-level amplitude
or transient detail.

The initial debug build took about 634 ms for the 60-second Dadaspec path and
571 ms for waveform. Cargo's dependency optimization wildcard excluded the
workspace-owned core crate, including its instantiated DSP loops. An explicit
core package optimization now keeps those loops optimized in desktop debug
builds, while retaining debug assertions and line tables.

## Changes

- Batch planar sample admission and cache the resampling ratio; native 32 kHz
  packets bypass resampling. Keep the same clamped interpolation coordinates
  across packet boundaries, including long clips whose f32 endpoints round up.
- Reuse packet conversion buffers and borrow already-planar F32 decoding.
- Share one immutable FFT/window/mel plan across requests. Mutable buffers,
  cancellation and bounded mel storage remain owned by each worker.
- Use the existing `fearless_simd` dependency for finite-value windowing and
  contiguous mel dot products. Compute each FFT bin's power once.
- Scale Files textures to physical width and overlap windows for short clips.
  Resolution refreshes reuse the existing bounded music queue and retain the
  displayed texture; source revisions and read guards still retire stale data.

Architecture comparison preserved dependency-cycle counts and average/max
complexity, and slightly reduced duplication percentage. Its one added clone
group is six lines of decoder setup in the opt-in GPU profile: existing
streaming summaries do not retain the PCM needed for that experiment. The
profile reuses the guarded source reader and conversion buffer. Its overall
regression label also includes unchanged clones in files modified by the
original integration and reordered cycle descriptions in unrelated tools.
The added mechanisms here are the block feeding path, immutable coefficients,
overlap policy, resolution refresh and profiling tools; no decoder service,
audio-content cache or GPU runtime dependency was added.

## GPU experiment: the three-file budget

The comparison below uses **three concurrent CPU waveform previews** as its
baseline, against one three-file batch of GPU Dadaspec previews. It measures
complete processing: guarded Symphonia decoding, PCM transfer, native CUDA
windowing/resampling/cuFFT/mel projection, power readback and the existing Rust
dB/RGB compositor and Lanczos resize. Final GPU-derived pixels are compared with
CPU Dadaspec (mean error below 0.5, maximum four channel values).

`scripts/dadaspec_gpu_probe.py` builds the opt-in C++/CUDA probe and runs its
ignored Rust profile. No application loads it. Each duration runs in a fresh
process, with three owned 44.1 kHz stereo PCM16 WAVs, 512-bin waveforms,
1024 × 60 Dadaspec images, and CPU affinity allowing three physical cores.
Times below are elapsed milliseconds for **all three files**, not their sum.
Warm medians cover nine runs after a second of GPU conditioning, so clocks and
power states can ramp up. That conditioning time is excluded from the warm
column. Cold times are individual first-batch observations, including library,
context, FFT-plan and first-kernel initialization, and varied across repeats.

| Length of each file | CPU waveforms | GPU Dadaspec, warm | GPU Dadaspec, cold | Warm GPU / CPU |
| --- | ---: | ---: | ---: | ---: |
| 1 s | 0.47 | 4.49 | 482 | 9.5× |
| 10 s | 3.99 | 8.29 | 160 | 2.1× |
| 60 s | 23.61 | 29.67 | 347 | 1.26× |
| 300 s | 121.59 | 144.00 | 544 | 1.18× |

**No amortization crossover was observed against waveform for this three-file
workload.** Even after conditioning, GPU Dadaspec's running cost was higher.
Without conditioning, its 60/300-second warm ratios were 1.31×/1.23×; the
conclusion did not depend on treating the GPU's first few bursts as steady state.
For five-minute files, the warm batch spent about 82 ms decoding/buffering,
52 ms on GPU analysis/transfers and 10 ms on CPU composition. A fast FFT alone
does not remove these other costs.

For repeated three-file batches, let `W` be CPU waveform batch time, `G` warm
GPU Dadaspec batch time and `S` GPU startup cost. Then `T_wave(k) = kW` and
`T_GPU(k) = S + kG`. Startup can be amortized only when `G < W`; break-even is
`ceil(S / (W - G))` batches. When `G >= W`, adding more batches cannot repay
startup. In the hypothetical serial example, with per-strip CPU cost `C`, GPU
startup `2C` and GPU running cost `C/6`, break-even is 2.4 strips, hence three
whole strips. Actual CPU concurrency must
be included before using that serial model.

These results rule out a speed advantage for this prototype and tested budget,
not every possible GPU design. It buffers complete PCM for measurement and uses
synchronous transfers; a production backend would require bounded streaming,
owned GPU admission/cancellation and fallback, and could test compressed/16-bit
uploads, overlapping transfers and larger batches. None of that is assumed in
the claimed ratios. The application keeps the optimized portable CPU renderer.

An earlier PyTorch compute-only exploration measured roughly 416 ms for CUDA
initialization, but excluded decoding/composition and had framework overhead.
It is superseded by the native, complete three-file measurements above.

## Reproducing

Build and run the CPU harness; its output is JSON Lines with every measured
sample as well as median/minimum. The optional fifth argument is an ordinary
audio file, or `-` for the generated WAV; the sixth is the requested strip width.
The independent `analysis` mode separates canonical PCM feeding from composition.

```sh
cargo build --release -p little-clip-core --example audio_preview_profile
target/release/examples/audio_preview_profile 60 48000 9 all - 1024
target/release/examples/audio_preview_profile 60 48000 9 dadaspec /path/to/audio.flac 1024
perf record -g --call-graph dwarf,8192 --user-callchains -e task-clock:u -F 199 -- \
  target/release/examples/audio_preview_profile 60 48000 30 dadaspec - 1024
perf report
python scripts/dadaspec_gpu_probe.py --cpus 6,8,10 --output-dir /tmp/dadaspec-native-profile
```

The pre-optimization executable used for comparison was retained outside the
repository during profiling. The original upstream pixel fixtures still test
the canonical path; additional tests exercise scalar/SIMD equivalence, planar
packet boundaries, overlap density, long resampling endpoints, physical-width
refresh, read guards and selection retention.
