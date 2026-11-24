# Embroidery strip rendering performance

Keep the atlas in the File Manager list. Drawing the visible stitch prefixes directly reduced worker preparation and upload traffic. It made every redraw process the stitches again, including each successive prefix in a sampled row. The measured draw cost also applied to hover scrubbing with a single dense row.

## Native list comparison

Two runs per path used the same release binary, files, 1440 × 1500 viewport and pointer sequence in private Weston/Niri, in atlas–geometry–geometry–atlas order. The machine was a Ryzen 9 5900XT and the client reported Mesa llvmpipe (LLVM 23.1.5, 256-bit) with eight software rendering threads. Allowing hardware selection in an isolated pilot still selected llvmpipe.

Ten strips were visible in the initial folder, with three 240,000-stitch EXP files and seven copies of the 553-stitch Wildflower PES fixture. The mixed view published 220 samples representing about 5.21 million stitch instances per redraw. The test scrolled to an all-Wildflower viewport, reversed to the mixed view, and moved across the wide EXP scrubber. It also opened a folder containing only the wide EXP and repeated the hover. Each process started with empty app preview caches. The runner primed the filesystem page cache identically.

Frame numbers below are ranges of the two runs' per-run p50 and p95 measurements. The proxy times File Manager UI entry through a final Glow paint callback that waits for GL completion. It includes UI construction, tessellation, requested uploads and rendering. Earlier app logic, swap and compositor display latency are outside this interval. The runner forces redraws during the warm phases.

| Workload | Atlas p50 / p95 | Direct geometry p50 / p95 |
| --- | ---: | ---: |
| Warm mixed list | 12.8–13.2 / 16.3–17.9 ms | 1,140–1,383 / 1,183–1,411 ms |
| Scroll out of the dense rows | 13.0–13.4 / 16.9–17.3 ms | 25.3–39.3 / 1,243–1,316 ms |
| Warm all-Wildflower list | 12.7–13.5 / 15.9–16.5 ms | 25.3–33.7 / 28.9–52.1 ms |
| Scroll back to the dense rows | 13.5–14.0 / 16.0–17.7 ms | 27.1–33.6 / 1,341–1,457 ms |
| Scrub wide EXP in its own folder | 8.6–9.7 / 10.6–12.4 ms | 97.4–119.1 / 124.3–155.8 ms |

The warm mixed phase sampled 53–55 atlas frames and eight direct frames per run. In the fixed-rate mixed scrub, the atlas displayed all 11 distinct prefixes in both runs. Direct drawing displayed one distinct prefix in each run, with only two completed frames during each input burst. Both paths reached all 11 prefixes when scrubbing the wide EXP in isolation.

Cold time from the first visible-row frame to the first frame with all 10 strips ready was 107–123 ms with the atlas and 4.08–6.15 s with geometry. Offscreen list previews retired on both paths and were loaded again on the return scroll.

## CPU preparation and payloads

Stage timings are ranges of the per-run medians for completed preview loads. Decode includes the guarded read and parse.

| Stage | Atlas | Direct geometry |
| --- | ---: | ---: |
| Decode 553 stitches | 0.130–0.145 ms | 0.092–0.117 ms |
| Prepare 553 stitches | 2.61–3.09 ms | 0.006–0.009 ms |
| Decode 240,000 stitches | 6.54–6.56 ms | 4.42–12.78 ms |
| Prepare 240,000 stitches | 37.8–40.0 ms | 2.09–4.98 ms |

The sequence queued 52.64 MB of atlas pixels and submitted 33.88 MB of VBO uploads. Atlas conversion and texture enqueue took 0.25–0.30 ms median per ready image. The report measures byte payloads and texture enqueue CPU time. The synchronized frame proxy includes GL upload work.

The warm mixed viewport held 15.65 MB of atlas pixel payload in textures. Direct drawing held 14.47 MB in VBOs plus 14.48 MB of prepared CPU instances. The all-Wildflower viewport held 14.37 MB of atlas pixels or about 0.11 MB on each side of the direct path. A single dense wide EXP used 1.22 MB for its atlas or 4.80 MB each for CPU instances and its VBO. Figures describe payload sizes and exclude GL driver overhead.

The prototype reused the full viewer's packed instances and antialiased shader. It capped its strip cache at 48 MiB and 32 entries, and retired buffers using weak references to the row previews. The warm direct phases incurred zero VBO uploads. In the all-Wildflower phase the dense buffers had retired.

The direct path reused timeline fitting, prefix selection, color order and hover UI. At strip scale the shader's edge coverage and shading visibly changed the artwork. In paired captures, at least one RGB channel changed by more than 12 levels on 47.7% of the first square EXP strip crop and 18.4% of the first Wildflower strip crop. The atlas preserves the established appearance while reusing the completed samples across redraws. Hardware-accelerated timings remain unverified.

## Reproduce

The opt-in benchmark is archived locally in `d098ac63` on `experiment/embroidery-strip-geometry`. The script generates fixtures, opens the ordinary File Manager executable in a private compositor, drives folder navigation and pointer input, and writes JSON, frame logs and PNG captures. Build on that local branch and run the four cases using one fixture root.

```sh
cargo build --release --locked -p clip-file-manager
trial=$(mktemp -d /tmp/lsc-strip.XXXXXX)
for run in atlas-a geometry-a geometry-b atlas-b; do
    python3 scripts/embroidery_strip_experiment.py \
        --mode "${run%-?}" --fixture "$trial/fixture" --output "$trial/$run"
done
```
