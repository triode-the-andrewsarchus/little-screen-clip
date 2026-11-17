# Bundled document-unwarping models

The Deskew tool runs these FP32 ONNX graphs through the application's existing
ONNX Runtime, with CUDA and CPU fallback. Models are embedded in both desktop
binaries: no network, Python environment, checkpoint loading or working-directory
lookup is needed at runtime. Combined graph size is approximately 62.4 MiB.
CUDA uses full FP32 math for these coordinate predictors; CPU inference uses four
threads. Raster composition and resampling remain on the bounded image worker.

* `uvdoc.onnx`: UVDoc's published `best_model.pkl`, exporting only the 2D grid
  head. RGB input is float `[0, 1]`, NCHW `[1, 3, 712, 488]`. Output is
  `[1, 2, 45, 31]`, with x followed by y in normalized source coordinates.
* `syntheticdoc.onnx`: SyntheticDoc's `original_31x45.ckpt`. RGB input is float
  `[-1, 1]`, NCHW `[1, 3, 720, 512]`. Outputs are a `[1, 2, 31, 45]` grid and
  `[1, 3, 720, 512]` shadow map. Its grid orientation differs from UVDoc; do not
  transpose it. Illumination correction follows the published inference code's
  brightness attenuation, source-space division, and subsequent unwarping.

Source revisions, checkpoint hashes, artifact hashes and numerical export checks
are recorded in `manifest.json`. Upstream MIT notices are retained in the two
`*-LICENSE.txt` files and embedded in each ONNX graph's metadata, so the notices
also travel with the desktop binaries. Projects:

* https://github.com/tanguymagne/UVDoc
* https://github.com/tanguymagne/SyntheticDoc

Reproduce the exports from the repository root:

```sh
python scripts/export_document_models.py --cache-dir /tmp/clip-document-models
```

Add `--fixture-dir crates/clip-studios/assets/document-unwarp/test` to regenerate
the self-authored document images and their PyTorch reference results. Normal
studio tests run both bundled graphs through the actual asynchronous preview,
apply and undo/redo path, comparing the output pixels with these references.

```sh
cargo test -p clip-studios document_unwarp
cargo test -p little-clip-ops document_unwarp
# Opt-in hardware check; asserts CUDA kernel events in an ORT execution trace:
cargo test -p clip-studios bundled_document_models_execute_on_cuda -- --ignored --nocapture
# Private compositor; preserves the user's desktop, daemon and clipboard:
python scripts/document_unwarp_acceptance.py --output-dir /tmp/deskew-ui --appearance dark
```

Development dependencies: PyTorch (export tested with 2.15.0), onnx 1.21.0,
onnxruntime 1.24.4, numpy and Pillow. The script verifies source/checkpoint hashes,
uses weights-only checkpoint loading, checks the ONNX graphs, and compares their
outputs against the pinned PyTorch implementations. The app itself uses none of
these Python dependencies.

SyntheticDoc works best on a photo roughly cropped around one document. Its
illumination correction is optional and defaults off, allowing a direct geometry
comparison with UVDoc. Both preserve source dimensions; neither determines the
paper's original physical aspect ratio. The existing manual four-point tool can
crop/rectify a page before neural unwarping, or be used independently.
