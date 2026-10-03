These small fixtures are generated test data, not third-party scenes.

- `mesh.glb`: four-vertex tetrahedron with four triangles and a blue material;
  validates the ordinary GLB mesh fallback without needing a GPU.
- `packed.compressed.ply`: two Gaussians exported by PlayCanvas SplatTransform
  3.9.0 (`435b972a97a278c655d8f7015b1355215cba0d71`) using its CPU converter.
  Input positions are `(-0.5, 0, 0)` and `(0.5, 0.3, 0.2)`, log scales
  `(-2.3, -1.6, -1.9)`, DC coefficients `(1, -1, -0.5)`, opacity logit `3`,
  and identity rotations. This tests the real exporter's packed representation.
- `packed-v4.spz`: the same two-Gaussian input exported by SplatTransform 3.9.0
  with `--spz-version 4`; validates the five-stream degree-zero SPZ container.
- `two-gaussians.sog`: the same synthetic input exported as bundled SOG v2;
  exercises continuous workbench navigation on a tiny decoded scene.
