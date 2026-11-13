# Native vector renderer

FemtoVG 0.28.0 is used under its MIT option; its original license is retained in
`femtovg-MIT.txt`. Source: <https://crates.io/crates/femtovg/0.28.0> and
<https://github.com/femtovg/femtovg/tree/dde7023118892974f14d4c7dd2c44efba7ccb1d0>.
The library is unmodified. Its default image-loading and text-layout features
are disabled; the application already owns bounded image admission and native
drawing glyph outlines. No example font or artwork is imported.

Initially this is a development-only dependency for validating context sharing
with the existing egui 0.27 / eframe OpenGL renderer. The probe must demonstrate
native vector drawing, ordinary egui painting after the callback, zoom/pan,
resize and resource cleanup on the same context before studio integration.
There is no separate windowing or browser engine dependency in the renderer.
