# Waveform preview performance

Measured 2026-10-06 on a Ryzen 9 5900XT (32 logical CPUs), using the eight MP3s in `/home/vega/Documents/type_testing/Ashes Against The Grain` (59.88 minutes of audio). Input files were only read. Measurements use optimized Rust builds and warm filesystem caches.

## Results

The benchmark pumps the actual Files preview cache every 16 ms and waits until all eight previews, including metadata and cover textures, are ready. Each sample starts with an empty application cache. Median of three runs:

| Album preview readiness | Before | After | Speedup |
| --- | --- | --- | --- |
| All eight visible music previews | 6,665.156 ms | 674.911 ms | 9.88× |

Before runs: 6,665.156 / 6,824.573 / 6,505.071 ms. Final runs: 691.434 / 658.930 / 674.911 ms.

Individual waveform generation is also measured independently from metadata and covers. Medians across three runs per track:

| Track | Before (ms) | After (ms) | Speedup |
| --- | --- | --- | --- |
| 01 | 957.3 | 597.1 | 1.60× |
| 02 | 940.5 | 586.6 | 1.60× |
| 03 | 159.8 | 101.4 | 1.58× |
| 04 | 1009.6 | 632.2 | 1.60× |
| 05 | 900.1 | 562.0 | 1.60× |
| 06 | 521.2 | 323.0 | 1.61× |
| 07 | 622.5 | 384.9 | 1.62× |
| 08 | 667.8 | 406.7 | 1.64× |

Across the 24 per-track observations, waveform-only medians are 786.101 → 484.103 ms (1.62×). The approximately 10× result applies to preparing the folder's previews together, not decoding one isolated track. Throughput gains depend on available CPU concurrency.

## What changed

- Removed the one-file-at-a-time music admission bottleneck. A separate bounded executor admits up to eight visible music jobs, capped by available CPUs, while the shared executor remains available for clipboard, viewer and file operations. Ordinary gallery/list/Shelf work retains one-job admission.
- Reused Symphonia directly at packet boundaries. Native float channels are borrowed without interleaving or per-sample iterator dispatch; other sample formats reuse one packet-sized conversion buffer. Bucket boundaries are computed per block rather than with floating-point division and channel remainders for every sample.
- Kept the same shared `WaveformColor` crossover, channel histories, frequency colors and absolute-peak behavior. Every decoded sample contributes; no sparse sampling or track-sized PCM cache is used. Completed previews retain only 512 peaks/colors and the existing bounded metadata/cover data.
- Overlapped bounded metadata and cover extraction with decoding. At most one scoped details helper accompanies each admitted music job. Cancellation and stale-source checks remain; retired jobs are rejected before publication. Decode cancellation and the ten-second budget are checked between packets.
- Initialized rate/channel layout from the first decoded packet, supporting AAC containers that omit channel layout in their initial codec parameters.

The measured steps were approximately 6.67 s originally, 1.03 s with parallel admission and an interleaved bucket loop, 0.76 s with planar decoding, then 0.675 s with details overlapped. The interleaved loop did not improve individual-track time and was replaced. A run overlapping compilation was excluded from the final comparison. GPU/FFT acceleration was not required; the preview color calculation is a crossover filter, and packet decoding plus concurrency addressed the measured costs.

## Regression tests and validation

`preview_waveform_synthetic_stereo_matches_scalar_across_packet_boundaries` generates 768,034 synthetic samples: bass/mid/high tones, opposite-phase channels, a late isolated peak, NaN and infinity. It compares every peak and color to a scalar reference across 37-, 1,152- and 4,096-frame packets. Release measurements of this pure summary calculation were 1.63 / 1.59 / 1.65 ms. The test asserts output correctness without a machine-dependent timing threshold.

`parallel_music_admission_is_bounded_and_retirement_cancels_every_job` checks the admission limit, cancellation of every retired token, and absence of stale publication. Existing WAV/MP3/Vorbis/AAC tests validate decoded summaries, excerpts and cancellation.

- `cargo test --workspace --offline -- --test-threads=1`: 1,895 passed, 23 ignored. Workbench library: 717 passed, 9 ignored.
- Release synthetic test passes; album benchmark passes with nonempty waveforms for every file.
- `./launch.sh build --bin clip-file-manager`, formatting and diff whitespace checks pass.
- Architecture comparison against `pre-waveform-speed`: maximum complexity 196 and cycle count 2 unchanged; no new functions above CCN 15. Clone count remains 1,296, duplication 7.85% → 7.84%. Flagged clones in the changed waveform modules match pre-existing fragments.

Reproduce the real-data benchmark (ignored during ordinary unit tests):

```bash
FILES_WAVEFORM_BENCH_DIR='/home/vega/Documents/type_testing/Ashes Against The Grain' \
  cargo test -p clip-workbench --lib --release --offline \
  benchmark_music_preview_album -- --ignored --nocapture --test-threads=1
```

Run the synthetic test:

```bash
cargo test -p little-clip-core --release --offline preview_waveform_synthetic -- --nocapture
```

Local logs: `/tmp/waveform-album-cache-before.log`, `/tmp/waveform-album-final-quiet.log`, `/tmp/waveform-synthetic-test.log`, `/tmp/waveform-speed-workspace-tests.log`.
