# Native drawing engine and scene model

The application uses excalidraw-render 0.1.0 and its matching
excalidraw-render-fonts 0.1.0+excalidraw.0.18.1 catalog (including bundled CJK
assets), and excalidraw-document 0.2.0. SVG rendering is selected without the
renderer library's optional PNG/PDF backends. Font decoding uses the same native
wuff version as the file manager's font viewer. There is no browser, V8, Node,
GPUI or source-driven filesystem/network service in this integration.

Original renderer source:

- https://github.com/sagikazarmark/excalidraw-render/tree/2f4c461ea40ba6850e143f70f310a25a6a50f94d
- https://crates.io/api/v1/crates/excalidraw-render/0.1.0/download

The pristine published sources were committed before the local extension under
`vendor/excalidraw-render`. The extension exposes `SceneViewport` on
`RenderOutcome`: the exact computed source-space origin and logical size before
output scaling. It carries the existing layout values through resolution and
encoding; it does not reproduce the bounds algorithm in the application.
Translated-scene, rotation, empty-scene and output-scaling integration tests cover
that correspondence. The released source package omits upstream qualification
fixtures, so this dependency is excluded from application workspace membership;
application integration tests exercise its real public API.

The unmodified document-model dependency is from
https://github.com/sagikazarmark/excaliplot/tree/29392d10d9c504ec22d67f37a4dd545a63d5b371
and the locked crates.io 0.2.0 release. Its root MIT and Apache license texts are
retained exactly at that revision.

Original license texts are retained in `excalidraw-render-LICENSE-BUNDLE.txt`,
`excalidraw-fonts-LICENSE-BUNDLE.txt`, and the document-model MIT/Apache files.
The renderer and font bundles are byte-identical originals and cover project
MIT/Apache, adapted MIT/BSD code, OFL font assets and CC-BY placeholder artwork.
Rendered artifacts' native notices must be retained when exporting incorporated
assets. The application adapter must preserve diagnostics and must not silently
replace the drawing's fonts or discard embedded images.
