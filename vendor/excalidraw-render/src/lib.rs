//! Renders [Excalidraw](https://excalidraw.com) scenes to SVG, PNG and PDF,
//! matching Excalidraw 0.18.1's exporter as closely as possible.
//!
//! It is pure Rust with every font bundled: rendering needs no browser,
//! Node.js, network access or installed fonts. Export a whole scene or each
//! frame on its own. Every render also returns a compatibility report that
//! lists what the renderer substituted, omitted or approximated, so
//! differences from Excalidraw are reported rather than silent.
//!
//! This release reproduces Excalidraw 0.18.1. In 0.x, any release may change
//! output and diagnostics; pin the renderer version and dependency lockfile
//! for reproducible output.
//! Linux x86-64 GNU is release-qualified; browser WASM is experimental.
//!
//! Throughout this documentation, *reference* means Excalidraw's own exporter
//! at the pinned release, v0.18.1: "reference frame-export semantics" and
//! "reference dark-mode colors" describe what Excalidraw itself produces. An
//! *admitted* scene is one [`SceneDocument::parse`] accepted.
//!
//! # Quick start
//!
//! Admit the scene once with [`SceneDocument::parse`], describe one output
//! with a [`RenderRequest`], and render it with a [`Renderer`]. A successful
//! render returns the complete [`Artifact`] together with a
//! [`CompatibilityReport`] listing every repair, substitution or omission the
//! renderer made to keep the artifact meaningful; an empty report means none
//! were needed.
//!
//! ```
//! use excalidraw_render::{
//!     FrameRenderRequest, OutputFormat, RenderRequest, Renderer, SceneDocument, Sizing,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // The contents of an `.excalidraw` file.
//! let bytes = br##"{
//!     "type": "excalidraw",
//!     "version": 2,
//!     "elements": [
//!         {"id": "overview", "type": "frame", "name": "Overview",
//!          "x": 0, "y": 0, "width": 200, "height": 140},
//!         {"id": "box", "type": "rectangle", "frameId": "overview",
//!          "x": 40, "y": 30, "width": 120, "height": 80}
//!     ],
//!     "appState": {"viewBackgroundColor": "#ffffff"},
//!     "files": {}
//! }"##;
//! let scene = SceneDocument::parse(bytes)?;
//! let renderer = Renderer::new();
//!
//! // Export the whole scene as SVG with the reference export defaults.
//! let request = RenderRequest::builder(OutputFormat::Svg).build()?;
//! let outcome = renderer.render(&scene, &request)?;
//! for diagnostic in outcome.report().diagnostics() {
//!     eprintln!(
//!         "{} {} at {}: {}",
//!         diagnostic.severity(),
//!         diagnostic.code(),
//!         diagnostic.json_pointer(),
//!         diagnostic.message()
//!     );
//! }
//! let svg = outcome.artifact().bytes();
//! assert!(svg.starts_with(b"<svg"));
//!
//! // Export every frame as a PNG no larger than 1024 pixels on either side.
//! # #[cfg(feature = "png")] {
//! let request = FrameRenderRequest::builder(OutputFormat::Png)
//!     .sizing(Sizing::MaxDimension(1024.0))
//!     .build()?;
//! for frame in scene.frames() {
//!     let outcome = renderer.render_frame(frame, &request)?;
//!     let png = outcome.artifact().bytes();
//!     println!("{}: {} bytes", frame.name().unwrap_or("unnamed"), png.len());
//!     assert!(png.starts_with(b"\x89PNG"));
//! }
//! # }
//! # Ok(())
//! # }
//! ```
//!
//! Each failure type ([`SceneParseError`], [`RequestError`], [`RenderError`],
//! [`FrameBatchError`]) carries a stable [`ErrorCode`], and `descriptor()` gives
//! its serializable form; the first three also carry typed details.
//! [`Renderer::render_frames`] renders a frame batch that shares scene
//! preparation across frames.
//!
//! # Serving untrusted input
//!
//! The renderer is built to render scene documents from untrusted sources, but
//! a service still has to decide how much CPU time and memory one request may
//! take.
//!
//! - **Share and move freely.** [`Renderer`] and [`SceneDocument`] are
//!   immutable, `Clone`, `Send` and `Sync`; one renderer can serve every
//!   request concurrently, and an admitted scene can be rendered to several
//!   formats from different threads.
//! - **Rendering is synchronous and CPU-bound.** In an async server, run it on
//!   a blocking pool (for example `tokio::task::spawn_blocking`) rather than on
//!   the executor's worker threads.
//! - **There is no deadline or cancellation.** Every render terminates, because
//!   all work is bounded by [`ResourceLimits`], but a render cannot be
//!   interrupted once started. When you need a hard wall-clock deadline, render
//!   in a separate process you can kill.
//! - **Bound raster sizes from the request.** For PNG and PDF, prefer
//!   [`Sizing::MaxDimension`] over a caller-chosen [`Sizing::Scale`] so a
//!   large scene cannot request an enormous PNG or page; then lower
//!   `max_raster_dimension` and `max_raster_surface_pixels` to what you are
//!   willing to allocate. SVG ignores `MaxDimension` and keeps the scene's
//!   logical size, which `max_svg_dimension` bounds instead.
//! - **`max_working_bytes` is not a memory cap.** It budgets the renderer's
//!   deterministic, logical working set so that the same input fails the same
//!   way on every machine; it does not measure or limit allocator usage.
//!   Actual peak memory is higher, dominated by raster surfaces: PNG
//!   rasterizes a 4× supersampled canvas, about 65 bytes per output pixel, so
//!   the default `max_raster_surface_pixels` (33,554,432) admits renders that
//!   need over 2 GiB. A ceiling of 4,194,304 pixels (2048 × 2048) keeps one
//!   render near 300 MB. Size worker memory (or a container limit) for your
//!   raster ceilings and concurrency.
//! - **Parsing has its own ceilings.** [`SceneDocument::parse`] accepts up to
//!   64 MiB of JSON whatever `max_scene_bytes` says; that limit is checked
//!   when rendering. Bound request bodies before parsing for a lower ceiling.
//! - **Limits apply per render.** [`Renderer::render_frames`] renders every
//!   selected frame within its own limits, so bound the frame count of an
//!   untrusted batch.
//! - **Build with optimizations.** Unoptimized builds are roughly 50 times
//!   slower, which turns ordinary scenes into multi-second renders and makes
//!   resource limits much easier to exhaust in wall-clock terms. Most of the
//!   time goes to dependencies (rasterization, compression, font decoding), so
//!   in a debug build of your service optimize every dependency:
//!
//!   ```toml
//!   [profile.dev.package."*"]
//!   opt-level = 2
//!   ```
//!
//! # Cargo features
//!
//! All three features are on by default. The API is the same with any
//! combination of them, so Cargo's feature unification can never change what
//! compiles; a feature only decides which requests a build can serve, and
//! turning one off never changes the bytes of the output that remains.
//!
//! - **`png`** renders [`OutputFormat::Png`]. It brings the `resvg`
//!   rasterizer and `tiny-skia`.
//! - **`pdf`** renders [`OutputFormat::Pdf`]. It brings the Krilla PDF writer,
//!   and the rasterizer for the isolated raster fallbacks of embedded SVG
//!   effects.
//! - **`cjk`** bundles Xiaolai, Excalifont's Chinese, Japanese and Korean
//!   fallback font (about 12 MiB). Without it, a render whose text needs
//!   Xiaolai fails with [`ErrorCode::RENDER_FONT_UNAVAILABLE`]
//!   (`render.font-unavailable`, a [`RenderErrorCategory::Capability`]
//!   failure) rather than substituting another font. Every other font,
//!   including Noto Emoji, is always bundled.
//!
//! SVG output is always available. Without `png` or `pdf`, building a
//! [`RenderRequest`] or [`FrameRenderRequest`] for that format fails with
//! [`ErrorCode::REQUEST_FORMAT_UNAVAILABLE`] (`request.format-unavailable`),
//! whose message names the feature to enable. An SVG-only build, for example
//! for WebAssembly, depends on the renderer like this:
//!
//! ```toml
//! [dependencies]
//! excalidraw-render = { version = "0.1", default-features = false, features = ["cjk"] }
//! ```

#![warn(missing_docs)]
#![warn(clippy::missing_errors_doc)]

mod catalog;
#[cfg(test)]
mod cms_failure_contract;
#[cfg(test)]
#[path = "../tests/common/codec_fixtures.rs"]
mod codec_fixtures;
mod color;
mod compatibility;
mod css_syntax;
mod diagnostic_registry;
mod font_metadata;
mod fonts;
mod frame;
#[cfg(test)]
#[path = "../tests/common/fuzz_seed.rs"]
mod fuzz_seed;
#[cfg(test)]
mod metadata_failure_contract;
mod normalization;
mod outcome;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/common/png_color_reference.rs"]
mod png_color_reference;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/common/png_icc_safety.rs"]
mod png_icc_safety;
#[cfg(test)]
mod png_projection_contract;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/common/png_stage4.rs"]
mod png_stage4;
#[cfg(all(test, target_arch = "wasm32"))]
#[path = "../tests/common/png_stage4_composition.rs"]
mod png_stage4_composition;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/common/png_stage4_reference.rs"]
mod png_stage4_reference;
#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/common/png_stage4_review.rs"]
mod png_stage4_review;
mod portable_math;
mod relationships;
mod render;
mod request;
mod resource_accounting;
mod resource_limits;
mod scene;
#[cfg(test)]
mod scene_fuzz;
mod scene_surface;
mod shaping;
#[cfg(test)]
mod stage4_diagnostic_contract;

/// Compiles the crate README's examples, so the crates.io page stays in step
/// with the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use compatibility::{
    AssetChainEntry, DiagnosticDetails, ErrorCode, ErrorDescriptor, ErrorFamily, FiniteValue,
    LogicalBounds, Measurement, MetadataComponent, MetadataOperation, NestedPath,
    NestedPathSegment, NestedSegmentKind, Recovery, Renderer, ResourceMeasurement, ResourceValue,
    SvgFontSubsetStage, ValueKind,
};
#[doc(hidden)]
pub use compatibility::{CompatibilityRevision, ReferenceProfileIdentity};
pub use frame::{
    FrameBatchError, FrameBatchErrorKind, FrameKind, FrameRef, FrameRefs, FrameRenderCursor,
    FrameRenderEntry,
};
pub use outcome::{
    Artifact, CompatibilityDiagnostic, CompatibilityReport, DiagnosticCode, DiagnosticEffect,
    DiagnosticSeverity, RenderError, RenderErrorCategory, RenderOutcome, SceneViewport,
};
pub use request::{
    ColorMode, FrameOptions, FrameRenderRequest, FrameRenderRequestBuilder, MetadataOptions,
    OutputFormat, PdfOptions, PdfRasterFallback, RenderRequest, RenderRequestBuilder, RequestError,
    Sizing, SupplementalAsset, SupplementalAssets, SupplementalAssetsBuilder,
};
pub use resource_limits::{ResourceLimits, ResourceLimitsBuilder};
pub use scene::{SceneDocument, SceneParseError};

#[cfg(all(test, target_arch = "wasm32"))]
pub mod wasm_tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);

    #[wasm_bindgen_test]
    fn png_sdr_color_reference_in_wasm() {
        png_color_cases_in_wasm(crate::png_color_reference::evidence());
    }

    #[wasm_bindgen_test]
    fn png_gain_map_reference_in_wasm() {
        png_color_cases_in_wasm(crate::png_color_reference::gain_map_evidence());
    }

    #[wasm_bindgen_test]
    fn png_stage4_icc_reference_in_wasm() {
        png_color_cases_in_wasm(crate::png_stage4_reference::evidence());
    }
    #[wasm_bindgen_test]
    fn cms_allocation_failures_in_wasm() {
        crate::cms_failure_contract::cms_out_of_memory_is_fatal_at_every_operation_boundary();
    }
    #[wasm_bindgen_test]
    fn png_stage4_profile_composition_in_wasm() {
        png_color_cases_in_wasm(crate::png_stage4_composition::evidence());
    }
    #[wasm_bindgen_test]
    fn png_stage4_review_regressions_in_wasm() {
        png_stage4_bytes(crate::png_icc_safety::headers());
        png_color_cases_in_wasm(crate::png_icc_safety::sampled_evidence());
        png_stage4_bytes(crate::png_stage4_review::adam7());
        png_stage4_bytes(crate::png_stage4_review::icc_extents());
        png_stage4_bytes(crate::png_stage4_review::lower_keys());
        png_color_cases_in_wasm(crate::png_stage4_review::gray_lut_equivalence());
    }
    #[wasm_bindgen_test]
    fn png_profile_projection_limits_in_wasm() {
        crate::png_projection_contract::profile_limits(Some(0));
    }
    #[wasm_bindgen_test]
    fn png_profile_rounded_limits_in_wasm() {
        crate::png_projection_contract::profile_limits(Some(1));
    }
    #[wasm_bindgen_test]
    fn png_profile_frame_mask_limits_in_wasm() {
        crate::png_projection_contract::profile_limits(Some(2));
    }

    #[wasm_bindgen_test]
    fn png_stage4_byte_contracts_in_wasm() {
        use crate::png_stage4 as v;
        for rows in [
            v::orientations(),
            v::invalid_orientation(),
            v::chunk_cases(),
            v::scanline_cases(),
            v::palette_cases(),
            v::apng_controls(),
            v::apng_malformed(),
            v::invalid_cicp(),
            v::large_streams(),
            v::composition(&[(*b"gAMA", 100000_u32.to_be_bytes().to_vec())], true),
            v::hdr_composition(16),
            v::hdr_composition(18),
        ] {
            png_stage4_bytes(rows);
        }
    }

    #[wasm_bindgen_test]
    fn png_stage4_ihdr_contracts_in_wasm() {
        png_stage4_bytes(crate::png_stage4::ihdr_cases());
    }

    fn png_stage4_bytes(rows: Vec<crate::png_stage4::Case>) {
        use crate::png_stage4::{self as v, Outcome};
        for row in rows {
            let source = v::scene(&row);
            let scene = crate::SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
            let request = crate::RenderRequest::builder(crate::OutputFormat::Png)
                .padding(0.0)
                .include_background(false)
                .build()
                .unwrap();
            let first = crate::Renderer::new().render(&scene, &request).unwrap();
            let second = crate::Renderer::new().render(&scene, &request).unwrap();
            assert_eq!(first.artifact().bytes(), second.artifact().bytes());
            assert_eq!(first.report(), second.report());
            v::report(&row, "png", serde_json::to_value(first.report()).unwrap());
            if matches!(row.outcome, Outcome::Accepted(_)) {
                let actual = crate::png_color_reference::png(first.artifact().bytes());
                assert_eq!(
                    (actual.width() as usize, actual.height() as usize),
                    (row.width, row.height)
                );
                assert_eq!(actual.rgba(), row.pixels, "{}", row.id);
            } else {
                let mut source = source;
                source["files"] = serde_json::json!({});
                let control =
                    crate::SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
                assert_eq!(
                    first.artifact().bytes(),
                    crate::Renderer::new()
                        .render(&control, &request)
                        .unwrap()
                        .artifact()
                        .bytes()
                );
            }
        }
    }

    fn png_color_cases_in_wasm(evidence: serde_json::Value) {
        for row in evidence["cases"].as_array().unwrap() {
            let scene =
                crate::SceneDocument::parse(&serde_json::to_vec(&row["scene"]).unwrap()).unwrap();
            let request = crate::RenderRequest::builder(crate::OutputFormat::Png)
                .padding(0.0)
                .include_background(false)
                .build()
                .unwrap();
            let first = crate::Renderer::new().render(&scene, &request).unwrap();
            let second = crate::Renderer::new().render(&scene, &request).unwrap();
            assert_eq!(first.artifact().bytes(), second.artifact().bytes());
            assert_eq!(
                serde_json::to_vec(first.report()).unwrap(),
                serde_json::to_vec(second.report()).unwrap()
            );
            if row["group"] == "composition" {
                let late = crate::SceneDocument::parse(
                    &serde_json::to_vec(&crate::png_stage4_review::late_scene(row)).unwrap(),
                )
                .unwrap();
                let late = crate::Renderer::new().render(&late, &request).unwrap();
                assert_eq!(first.artifact().bytes(), late.artifact().bytes());
                assert_eq!(first.report(), late.report());
            }
            if row["group"] == "composition" {
                crate::png_stage4_composition::report(
                    row,
                    "png",
                    serde_json::to_value(first.report()).unwrap(),
                );
            } else {
                crate::png_color_reference::report(
                    row,
                    "png",
                    serde_json::to_value(first.report()).unwrap(),
                );
            }
            if let Some(equivalent) = crate::png_color_reference::bt1886_equivalent(&evidence, row)
                .or_else(|| crate::png_color_reference::ignored_cicp_equivalent(&evidence, row))
                .or_else(|| crate::png_color_reference::gain_map_equivalent(&evidence, row))
            {
                let scene =
                    crate::SceneDocument::parse(&serde_json::to_vec(&equivalent["scene"]).unwrap())
                        .unwrap();
                let control = crate::Renderer::new().render(&scene, &request).unwrap();
                assert_eq!(
                    first.artifact().bytes(),
                    control.artifact().bytes(),
                    "{} exact metadata equivalence",
                    row["id"]
                );
                assert_eq!(first.report(), control.report());
            }
            if crate::png_color_reference::recovery(row, "png").is_some() {
                let mut control = row["scene"].clone();
                control["files"] = serde_json::json!({});
                let control =
                    crate::SceneDocument::parse(&serde_json::to_vec(&control).unwrap()).unwrap();
                assert_eq!(
                    first.artifact().bytes(),
                    crate::Renderer::new()
                        .render(&control, &request)
                        .unwrap()
                        .artifact()
                        .bytes()
                );
            } else {
                crate::png_stage4_reference::anchor(
                    row,
                    crate::png_color_reference::png(first.artifact().bytes()).rgba(),
                );
                crate::png_color_reference::appearance(
                    row,
                    crate::png_color_reference::png(first.artifact().bytes()),
                    false,
                );
            }
        }
    }

    #[wasm_bindgen_test]
    fn png_precision_reference_in_wasm() {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};
        let bytes = include_bytes!("../../../tests/oracle/png-precision/reference.json");
        let evidence: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        let decode_png = |bytes: &[u8]| {
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
                .read_info()
                .unwrap();
            let mut rgba = vec![0; reader.output_buffer_size().expect("PNG buffer fits")];
            let info = reader.next_frame(&mut rgba).unwrap();
            assert_eq!(info.color_type, png::ColorType::Rgba);
            assert_eq!(info.bit_depth, png::BitDepth::Eight);
            rgba.truncate(info.buffer_size());
            (info.width, info.height, rgba)
        };
        for row in evidence["cases"].as_array().unwrap() {
            let scene =
                crate::SceneDocument::parse(&serde_json::to_vec(&row["scene"]).unwrap()).unwrap();
            let request = crate::RenderRequest::builder(crate::OutputFormat::Png)
                .padding(0.0)
                .include_background(row["options"]["exportBackground"].as_bool().unwrap())
                .build()
                .unwrap();
            let first = crate::Renderer::new().render(&scene, &request).unwrap();
            let second = crate::Renderer::new().render(&scene, &request).unwrap();
            assert_eq!(first.artifact().bytes(), second.artifact().bytes());
            assert_eq!(first.report(), second.report());
            assert_eq!(
                serde_json::to_value(first.report()).unwrap(),
                serde_json::json!({"schema_version":1,"diagnostics":[]})
            );
            let expected = base64::engine::general_purpose::STANDARD
                .decode(row["png_base64"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                decode_png(first.artifact().bytes()),
                decode_png(&expected),
                "{}",
                row["id"]
            );
        }
    }

    #[wasm_bindgen_test]
    fn complete_font_subset_qualification_corpus_in_wasm() {
        crate::render::assert_complete_font_subset_qualification_corpus();
    }

    #[wasm_bindgen_test]
    fn font_subset_fuzz_regression_corpus_in_wasm() {
        crate::render::assert_font_subset_fuzz_regression_corpus();
    }

    #[wasm_bindgen_test]
    fn font_subset_fuzz_fixed_seed_smoke_in_wasm() {
        crate::render::assert_font_subset_fuzz_smoke();
    }

    #[wasm_bindgen_test]
    fn font_subset_resource_trace_matrix_in_wasm() {
        crate::render::assert_font_subset_resource_trace_matrix();
    }

    #[wasm_bindgen_test]
    fn scene_fuzz_regression_corpus_in_wasm() {
        crate::scene_fuzz::assert_scene_fuzz_regression_corpus();
    }

    #[wasm_bindgen_test]
    fn scene_fuzz_fixed_seed_smoke_in_wasm() {
        crate::scene_fuzz::assert_scene_fuzz_smoke();
    }
}

/// Returns the complete package-wide license and attribution bundle.
pub const fn license_bundle() -> &'static str {
    include_str!("../LICENSE-BUNDLE.txt")
}
