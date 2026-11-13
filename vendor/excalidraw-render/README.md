# excalidraw-render

Pure-Rust rendering of [Excalidraw](https://excalidraw.com) scene documents to
**SVG, PNG, and vector-first PDF**, for a whole scene or frame by frame. Fonts
and other rendering assets are bundled: rendering needs no browser, Node.js,
network access, or installed fonts.

![A hand-drawn architecture diagram with hatched boxes in several colors, curved and straight arrows, and labels in three Excalidraw fonts](https://raw.githubusercontent.com/sagikazarmark/excalidraw-render/main/docs/images/elasticsearch-spec-overview.svg)

<sub>Rendered to SVG with this crate's default request (the command-line
equivalent is `excalidraw-render overview.excalidraw -o overview.svg`).
Scene: [`overview.excalidraw`](https://github.com/elastic/elasticsearch-specification/blob/3d51338565d6558c96b55dad9142e4c2a500ca38/overview.excalidraw)
from elastic/elasticsearch-specification, © Elastic and contributors,
[Apache-2.0](https://github.com/sagikazarmark/excalidraw-render/blob/main/tests/public-scenes/elasticsearch-spec-overview/LICENSE).</sub>

The compatibility target is Excalidraw **0.18.1**. Output is close to, but not
pixel-identical with, the browser exporter; every render returns a
compatibility report describing the substitutions, omissions, and fallbacks it
made. See the [known limitations](../../docs/reference/known-limitations.md).

For a command-line tool, see
[`excalidraw-render-cli`](https://crates.io/crates/excalidraw-render-cli).

## Usage

```toml
[dependencies]
excalidraw-render = "0.1"
```

```rust,no_run
use excalidraw_render::{OutputFormat, RenderRequest, Renderer, SceneDocument, Sizing};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scene = SceneDocument::parse(&std::fs::read("drawing.excalidraw")?)?;
    let request = RenderRequest::builder(OutputFormat::Png)
        .sizing(Sizing::Scale(2.0))
        .build()?;

    let outcome = Renderer::new().render(&scene, &request)?;
    for diagnostic in outcome.report().diagnostics() {
        eprintln!("{diagnostic}");
    }
    std::fs::write("drawing.png", outcome.artifact().bytes())?;
    Ok(())
}
```

`Renderer::render_frames` exports each frame of a scene separately. The
[API documentation](https://docs.rs/excalidraw-render) covers requests,
resource limits, frame export, and the error and diagnostic types. The
[guides](../../docs/README.md) walk through
[the library](../../docs/guides/library.md),
[exporting frames](../../docs/guides/frames.md) and
[features and WebAssembly](../../docs/guides/features-and-wasm.md), and the
repository's [examples](examples) are complete programs.

## Untrusted input and servers

`Renderer` and `SceneDocument` are `Send + Sync`. Rendering is synchronous and
CPU-bound: call it from `tokio::task::spawn_blocking` or a worker pool in async
code. `ResourceLimits` bounds the work a scene can request, and
`Sizing::MaxDimension` bounds raster size, but there is no deadline or
cancellation: run renders in a separate process when you need a hard time or
memory ceiling. [Serving untrusted input](../../docs/guides/untrusted-input.md)
covers each of these.

The defaults admit large drawings rather than protect a shared server:

- A PNG render holds a 4× supersampled canvas, about 65 bytes per output
  pixel. At the default `max_raster_surface_pixels` (33,554,432) one render can
  need over 2 GiB; a limit of 4,194,304 (2048 × 2048) keeps it near 300 MB.
- `SceneDocument::parse` accepts up to 64 MiB of JSON whatever the request's
  `max_scene_bytes`, which is checked when rendering. Reject larger request
  bodies before parsing when you need a lower ceiling.
- `ResourceLimits` apply to each render. `render_frames` renders every selected
  frame, so bound the frame count, or render frames one at a time with
  `FrameRenderRequest`, when the scene is untrusted.

```rust,no_run
use excalidraw_render::{OutputFormat, RenderRequest, ResourceLimits, Sizing};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let limits = ResourceLimits::builder()
    .max_raster_surface_pixels(4_194_304)
    .max_total_rasterized_pixels(8_388_608)
    .build()?;
let request = RenderRequest::builder(OutputFormat::Png)
    .sizing(Sizing::MaxDimension(2048.0))
    .resource_limits(limits)
    .build()?;
# let _ = request;
# Ok(())
# }
```

Unoptimized builds render roughly 50× slower. In development, optimize the
renderer even in debug builds:

```toml
[profile.dev.package."*"]
opt-level = 2
```

## Cargo features

All three features are on by default. The API is the same with any
combination of them; a feature only decides which requests a build can serve.

- `png`: PNG output. Brings the `resvg` rasterizer and `tiny-skia`.
- `pdf`: PDF output. Brings the Krilla PDF writer, and the rasterizer for the
  isolated raster fallbacks of embedded SVG effects.
- `cjk`: bundles Xiaolai, Excalifont's Chinese/Japanese/Korean fallback
  (about 12 MiB). Without it, a render that needs Xiaolai fails with
  `render.font-unavailable` instead of substituting another font. All other
  fonts, including Noto Emoji, are always bundled.

SVG output is always available. Without `png` or `pdf`, building a request for
that format fails with `request.format-unavailable`, and turning a feature off
never changes the bytes of the output that remains. For an SVG-only build, for
example for WebAssembly:

```toml
[dependencies]
excalidraw-render = { version = "0.1", default-features = false, features = ["cjk"] }
```

Features are additive, so another dependency can enable one again; disabling
one removes its crates from the build, not necessarily from your lockfile or
`cargo fetch`.

## Reproducibility

Output is deterministic for a given renderer release, input, options, and
dependency versions. Dependencies take ordinary semver ranges, so a patch
release of a decoder, rasterizer or compressor can change output bytes; the
release's gates test the versions in its repository `Cargo.lock`. Later 0.x
releases may also intentionally change rendering. When exact output bytes
matter, pin the renderer (`=0.1.0`) and commit your `Cargo.lock`; see
[reproducible output](../../docs/guides/reproducible-output.md).

Linux x86-64 (glibc) is the release-qualified platform. Other native platforms
build and run but are not release-qualified; browser WASM is experimental.

## License

Code is licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option. Bundled fonts and adapted
third-party code carry their own licenses, collected in
[`LICENSE-BUNDLE.txt`](LICENSE-BUNDLE.txt). Exports carry the attribution
required by the bundled resources they use.
