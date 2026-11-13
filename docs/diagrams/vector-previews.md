# Desktop vector previews

Desktop diagram previews now attempt the existing retained SVG renderer for
all registered formats. The same path paints Markdown diagrams/equations,
standalone source previews, file-manager strips and their enlarged hover views.
Small thumbnail dimensions no longer determine native artwork resolution.

Pikchr and svgbob previously returned text-bearing SVG. Their exports now reuse
the exact bundled-font outlines already prepared for raster painting, rather
than resolving fonts again on the UI thread. Already outlined exports keep their
semantic group IDs and expandable editor planes. Passive previews retain the
original viewport crop.

Mermaid's standard zero-blur drop shadow is composited from source alpha at the
current device scale, with its original offset, flood color/opacity and filter
bounds. Group opacity still applies to the completed artwork. Shadow nesting is
limited before scene delivery so replay cannot multiply UI-thread work without
a bound. Normal paths and text remain vectors.

Raster fallback remains explicit for masks, other filter graphs, SVG patterns,
repeating gradients, exceeded geometry/image budgets or an unavailable native
host. Unsupported effects are preserved, not stripped. Photographs remain
embedded raster assets. The physical little screen still needs its final raster
framebuffer, and the bounded PNG remains available for that route and desktop
fallback. This change adds no browser engine or rendering dependency.

The existing preview jobs, source revisions, cancellation, row cache, native GL
runtime and teardown owners remain in use. Parsing, glyph outlining, resource
admission and geometry preparation stay on bounded workers. Camera/DPI changes
paint retained paths without reparsing authored source.

## Reproducing validation

- `cargo test --workspace -j4 -- --test-threads=1 --skip playback::embedded_tracks_render_subtitles_and_options_seek_at_the_requested_rate_and_loop`
- `cargo test -p clip-media-worker --test handles playback::embedded_tracks_render_subtitles_and_options_seek_at_the_requested_rate_and_loop -j4 -- --test-threads=1`
- `cargo clippy --workspace --all-targets -j4 -- -D warnings`
- `cargo build -p clip-file-manager -p clip-workbench -j4`

The workspace run passes 3,171 tests with 36 ignored. The filtered media seek case
passes in isolation; it failed with `Failed to seek` in two unfiltered workspace
runs. No media playback code or test limits were changed for this work.

`vector_context_acceptance.py --svg <outlined.svg>` exercises canonical SVG
through the real native runtime at 100% and 1600%, alongside egui controls,
window resizing and current-context teardown. It must report zero GL/renderer
errors and successful release. The normal diagram/text-resize acceptance scripts
exercise the actual application routes and source/Undo preservation.

Use a frozen native executable for GUI checks: the application's bounded workers
relaunch `current_exe`. Cargo can unlink/reinstall an ordinary build while its
GUI is open. A temporary hard link beside the built desktop binary preserves
both its inode and bundled-helper discovery without duplicating binary storage;
remove that link after every GUI/helper has exited.
