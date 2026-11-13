//! Plan-level tests: each format's projection policy asserted on its plan,
//! built from `resolve_for_test`'s graph without a session, so with no
//! accounting, no metadata selection and no font subsetting. The plans are
//! thin projections of the resolved scene; what they decide (PNG surface size
//! and shared clip masks, the PDF raster closures a nested SVG's effects need,
//! the SVG plan's one definition per shared clip) is tested here once, and the
//! bytes those decisions become are the encoder unit tests' and the
//! end-to-end smokes' business. A format's tests build only with its feature.

use super::font_demand::{build_svg_font_bundle, svg_font_demand, svg_font_text_order};
#[cfg(feature = "pdf")]
use super::pdf::PdfPlan;
#[cfg(feature = "png")]
use super::png::PngPlan;
#[cfg(feature = "png")]
use super::resolve_tests::NESTED_FRAME_SCENE;
use super::resolve_tests::{request, scene, NAMED_FRAME_SCENE};
use super::svg::SvgPlan;
use super::*;
#[cfg(feature = "png")]
use crate::FrameOptions;

#[cfg(feature = "png")]
fn png_plan(scene: &SceneDocument, request: &RenderRequest) -> PngPlan {
    let (resolved, svg_trees) =
        resolve_with_svg_trees_for_test(scene, request).expect("scene resolves");
    PngPlan::new(resolved, request, &svg_trees).expect("PNG plan builds")
}

// --- PNG surface ----------------------------------------------------------

#[cfg(feature = "png")]
#[test]
fn png_surface_truncates_the_fractional_frame_name_extent() {
    // The name box makes the scene 600 x 620.5 logical pixels; the surface
    // truncates the half pixel.
    let plan = png_plan(&scene(NAMED_FRAME_SCENE), &request(OutputFormat::Png));
    assert_eq!((plan.width(), plan.height()), (600, 620));
}

/// A coverage owner, so it compiles in every build (ADR-0018); without the
/// `png` feature it checks that a PNG request is refused instead.
#[test]
fn png_surface_follows_the_sizing_rule() {
    #[cfg(not(feature = "png"))]
    assert_eq!(
        RenderRequest::builder(OutputFormat::Png)
            .build()
            .expect_err("PNG is not in this build")
            .code(),
        ErrorCode::REQUEST_FORMAT_UNAVAILABLE
    );
    #[cfg(feature = "png")]
    {
        // A 20 x 10 rectangle with the default 10 px padding: 40 x 30 logical.
        let scene = scene(br##"{
            "type":"excalidraw","version":2,
            "elements":[{"id":"box","type":"rectangle","x":0,"y":0,"width":20,"height":10,"roughness":0,"updated":1}],
            "appState":{},"files":{}
        }"##);
        for (sizing, expected) in [
            (Sizing::Scale(1.0), (40, 30)),
            (Sizing::Scale(1.5), (60, 45)),
            (Sizing::Scale(2.0), (80, 60)),
            // A ceiling below the largest dimension scales both by its ratio.
            (Sizing::MaxDimension(20.0), (20, 15)),
            // A ceiling above it leaves the scene at scale 1.
            (Sizing::MaxDimension(100.0), (40, 30)),
        ] {
            let request = RenderRequest::builder(OutputFormat::Png)
                .sizing(sizing)
                .build()
                .expect("sized request is valid");
            let plan = png_plan(&scene, &request);
            assert_eq!((plan.width(), plan.height()), expected, "{sizing:?}");
        }
    }
}

// --- PNG frame clip masks -------------------------------------------------

#[cfg(feature = "png")]
#[test]
fn png_builds_one_clip_mask_per_clipping_frame() {
    let plan = png_plan(&scene(NAMED_FRAME_SCENE), &request(OutputFormat::Png));
    let masks = plan.frame_clip_masks().expect("masks are enumerable");
    // Three clipped cells share the one frame's mask, owned by the frame's own
    // element (plan index 0).
    let [mask] = masks.as_slice() else {
        panic!("one mask for one frame: {masks:?}");
    };
    assert_eq!(mask.frame_element_index, Some(0));
    assert!(mask.pixels > 0);
    assert!(
        mask.pixels <= u64::from(plan.width()) * u64::from(plan.height()),
        "the mask is clamped to the surface: {mask:?}"
    );
}

#[cfg(feature = "png")]
#[test]
fn png_nested_frames_each_own_a_clip_mask_only_when_clipping_is_on() {
    let nested = scene(NESTED_FRAME_SCENE);

    let plan = png_plan(&nested, &request(OutputFormat::Png));
    let masks = plan.frame_clip_masks().expect("masks are enumerable");
    assert_eq!(
        masks
            .iter()
            .map(|mask| mask.frame_element_index)
            .collect::<Vec<_>>(),
        [Some(0), Some(1)],
        "the outer frame's mask clips the inner frame, the inner's clips the child"
    );
    assert!(
        masks[1].pixels < masks[0].pixels,
        "the inner frame covers fewer device pixels: {masks:?}"
    );

    let unclipped = RenderRequest::builder(OutputFormat::Png)
        .frame_options(FrameOptions::default().with_clip(false))
        .build()
        .expect("unclipped request is valid");
    let plan = png_plan(&nested, &unclipped);
    assert!(plan
        .frame_clip_masks()
        .expect("masks are enumerable")
        .is_empty());
}

// --- PDF raster closures --------------------------------------------------

#[cfg(feature = "pdf")]
fn pdf_plan(scene: &SceneDocument, request: &RenderRequest) -> PdfPlan {
    let (resolved, svg_trees) =
        resolve_with_svg_trees_for_test(scene, request).expect("scene resolves");
    PdfPlan::new(resolved, request, &svg_trees).expect("PDF plan builds")
}

fn data_url(media_type: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{media_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// One `width` x `height` image element at the origin over the nested SVG.
#[cfg(feature = "pdf")]
fn nested_svg_scene(svg: &[u8], width: u32, height: u32) -> SceneDocument {
    scene(
        format!(
            r#"{{
            "type":"excalidraw","version":2,
            "elements":[{{
                "id":"image","type":"image","x":0,"y":0,
                "width":{width},"height":{height},"angle":0,"opacity":100,
                "fileId":"asset","status":"saved","scale":[1,1],"seed":1,"updated":1
            }}],
            "appState":{{}},
            "files":{{"asset":{{
                "id":"asset","mimeType":"image/svg+xml",
                "dataURL":"{}","created":1
            }}}}
        }}"#,
            data_url("image/svg+xml", svg)
        )
        .as_bytes(),
    )
}

/// The default PDF request's plan for one 20 x 10 nested SVG, whose
/// `raster_closures` the effect tests read.
#[cfg(feature = "pdf")]
fn pdf_plan_of(nested: &[u8]) -> PdfPlan {
    pdf_plan(
        &nested_svg_scene(nested, 20, 10),
        &request(OutputFormat::Pdf),
    )
}

#[cfg(feature = "pdf")]
fn bounds(closure: &pdf::PdfRasterClosure) -> (f64, f64, f64, f64) {
    let bounds = closure.logical_bounds;
    (bounds.x(), bounds.y(), bounds.width(), bounds.height())
}

const BLURRED_RECT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blur" x="0" y="0" width="100%" height="100%"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="20" height="10" fill="#00ff00" filter="url(#blur)"/></svg>"##;

#[cfg(feature = "pdf")]
#[test]
fn pdf_plans_one_isolated_closure_for_a_filtered_nested_rect() {
    let plan = pdf_plan_of(BLURRED_RECT);
    let [closure] = plan.raster_closures() else {
        panic!("one closure: {:#?}", plan.raster_closures());
    };
    assert_eq!(closure.element_id, "image");
    assert_eq!(closure.json_pointer, "/files/asset/dataURL");
    // The whole nested SVG at its 10 px padded placement, on the 1:1 grid.
    assert_eq!(bounds(closure), (10.0, 10.0, 20.0, 10.0));
    assert_eq!((closure.pixel_width, closure.pixel_height), (20, 10));
    assert!(!closure.causes.is_empty());
    assert!(!closure.unisolatable);
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_plans_a_closure_for_an_inline_style_filter() {
    let plan = pdf_plan_of(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blur"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="20" height="10" style="fill:#00ff00;filter:url(#blur)"/></svg>"##,
    );
    assert_eq!(
        plan.raster_closures().len(),
        1,
        "{:#?}",
        plan.raster_closures()
    );
}

#[cfg(feature = "pdf")]
#[test]
fn a_root_local_effect_does_not_require_external_backdrop_capture() {
    let plan = pdf_plan_of(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10" filter="url(#blur)"><defs><filter id="blur"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="20" height="10"/></svg>"##,
    );
    let [closure] = plan.raster_closures() else {
        panic!("one closure: {:#?}", plan.raster_closures());
    };
    assert!(!closure.unisolatable);
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_plans_no_closure_for_empty_noop_or_fully_clipped_effects() {
    for (case, nested) in [
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="effect"></filter></defs><rect width="20" height="10" filter="url(#effect)"/></svg>"##.as_slice(),
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="effect"><feGaussianBlur stdDeviation="0"/></filter></defs><rect width="20" height="10" filter="url(#effect)"/></svg>"##.as_slice(),
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="effect"><feColorMatrix type="matrix" values="1 0 0 0 0 0 1 0 0 0 0 0 1 0 0 0 0 0 1 0"/></filter></defs><rect width="20" height="10" filter="url(#effect)"/></svg>"##.as_slice(),
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="effect"><feComponentTransfer><feFuncR type="identity"/><feFuncA type="identity"/></feComponentTransfer></filter></defs><rect width="20" height="10" filter="url(#effect)"/></svg>"##.as_slice(),
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="effect"><feGaussianBlur stdDeviation="1"/></filter><clipPath id="empty"><path d=""/></clipPath></defs><rect width="20" height="10" clip-path="url(#empty)" filter="url(#effect)"/></svg>"##.as_slice(),
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="effect"><feGaussianBlur stdDeviation="1"/></filter><clipPath id="away"><circle cx="15" cy="5" r="2"/></clipPath></defs><rect width="2" height="2" clip-path="url(#away)" filter="url(#effect)"/></svg>"##.as_slice(),
    ]
    .into_iter()
    .enumerate()
    {
        let plan = pdf_plan_of(nested);
        assert!(
            plan.raster_closures().is_empty(),
            "case {case}: {:#?}",
            plan.raster_closures()
        );
    }
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_merges_dependent_effects_to_the_outermost_required_closure() {
    let plan = pdf_plan_of(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="outer" x="0" y="0" width="100%" height="100%"><feGaussianBlur stdDeviation="1"/></filter><filter id="inner" x="0" y="0" width="100%" height="100%"><feColorMatrix type="saturate" values="0.5"/></filter></defs><g filter="url(#outer)"><rect width="20" height="10" fill="#00ff00" filter="url(#inner)"/></g></svg>"##,
    );
    let [closure] = plan.raster_closures() else {
        panic!("one merged closure: {:#?}", plan.raster_closures());
    };
    assert!(
        closure.causes.len() >= 4,
        "merged closure retains every cause: {:#?}",
        closure.causes
    );
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_plans_sibling_effect_closures_in_expanded_tree_order() {
    let plan = pdf_plan_of(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="a" x="0" y="0" width="100%" height="100%"><feGaussianBlur stdDeviation="1"/></filter><filter id="b" x="0" y="0" width="100%" height="100%"><feColorMatrix type="saturate" values="0.5"/></filter></defs><rect width="10" height="10" filter="url(#a)"/><rect x="10" width="10" height="10" filter="url(#b)"/></svg>"##,
    );
    assert_eq!(
        plan.raster_closures()
            .iter()
            .map(|closure| (closure.logical_bounds.x(), closure.logical_bounds.width()))
            .collect::<Vec<_>>(),
        [(10.0, 10.0), (20.0, 10.0)]
    );
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_shared_filter_occurrences_keep_their_own_causative_nodes() {
    let plan = pdf_plan_of(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blur"><feGaussianBlur stdDeviation="1"/></filter></defs><rect width="10" height="10" filter="url(#blur)"/><rect x="10" width="10" height="10" filter="url(#blur)"/></svg>"##,
    );
    assert_eq!(
        plan.raster_closures()
            .iter()
            .map(|closure| closure.causes[0].segments()[0].ordinal())
            .collect::<Vec<_>>(),
        [4, 5]
    );
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_plans_each_expanded_effect_reuse_occurrence() {
    let plan = pdf_plan_of(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blur" x="0" y="0" width="100%" height="100%"><feGaussianBlur stdDeviation="1"/></filter><g id="shape" filter="url(#blur)"><rect width="10" height="10"/></g></defs><use href="#shape"/><use href="#shape" x="10"/></svg>"##,
    );
    assert_eq!(
        plan.raster_closures()
            .iter()
            .map(|closure| closure.logical_bounds.x())
            .collect::<Vec<_>>(),
        [10.0, 20.0]
    );
}

#[cfg(feature = "pdf")]
#[test]
fn an_effect_that_reads_the_backdrop_is_marked_unisolatable() {
    let plan = pdf_plan_of(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><defs><filter id="blend" x="0" y="0" width="100%" height="100%"><feBlend in="SourceGraphic" in2="BackgroundImage" mode="multiply"/></filter></defs><rect width="20" height="10" filter="url(#blend)"/></svg>"##,
    );
    let [closure] = plan.raster_closures() else {
        panic!("one closure: {:#?}", plan.raster_closures());
    };
    assert!(closure.unisolatable);
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_effect_closure_resolution_uses_the_final_device_grid() {
    let request = RenderRequest::builder(OutputFormat::Pdf)
        .sizing(Sizing::Scale(2.0))
        .build()
        .expect("scaled request is valid");
    let plan = pdf_plan(&nested_svg_scene(BLURRED_RECT, 20, 10), &request);
    let [closure] = plan.raster_closures() else {
        panic!("one closure: {:#?}", plan.raster_closures());
    };
    assert_eq!(
        bounds(closure),
        (10.0, 10.0, 20.0, 10.0),
        "logical bounds do not scale"
    );
    assert_eq!((closure.pixel_width, closure.pixel_height), (40, 20));
}

#[cfg(feature = "pdf")]
#[test]
fn pdf_effect_closure_bounds_intersect_the_image_crop() {
    let scene = scene(
        format!(
            r#"{{"type":"excalidraw","version":2,"elements":[{{
                "id":"image","type":"image","x":0,"y":0,"width":10,"height":10,
                "fileId":"asset","status":"saved","scale":[1,1],"seed":1,"updated":1,
                "crop":{{"x":10,"y":0,"width":10,"height":10,"naturalWidth":20,"naturalHeight":10}}
            }}],"appState":{{}},"files":{{
                "asset":{{"id":"asset","mimeType":"image/svg+xml","dataURL":"{}","created":1}}
            }}}}"#,
            data_url("image/svg+xml", BLURRED_RECT)
        )
        .as_bytes(),
    );
    let plan = pdf_plan(&scene, &request(OutputFormat::Pdf));
    let [closure] = plan.raster_closures() else {
        panic!("one closure: {:#?}", plan.raster_closures());
    };
    assert_eq!(
        (
            closure.logical_bounds.x(),
            closure.logical_bounds.width(),
            closure.pixel_width
        ),
        (10.0, 10.0, 10)
    );
}

#[cfg(feature = "pdf")]
#[test]
fn repeated_effect_occurrences_plan_one_closure_each() {
    let scene = scene(
        format!(
            r#"{{"type":"excalidraw","version":2,"elements":[
                {{"id":"one","type":"image","x":0,"y":0,"width":20,"height":10,"fileId":"asset","status":"saved","scale":[1,1],"seed":1,"updated":1}},
                {{"id":"two","type":"image","x":20,"y":0,"width":20,"height":10,"fileId":"asset","status":"saved","scale":[1,1],"seed":2,"updated":1}}
            ],"appState":{{}},"files":{{
                "asset":{{"id":"asset","mimeType":"image/svg+xml","dataURL":"{}","created":1}}
            }}}}"#,
            data_url("image/svg+xml", BLURRED_RECT)
        )
        .as_bytes(),
    );
    let plan = pdf_plan(&scene, &request(OutputFormat::Pdf));
    assert_eq!(
        plan.raster_closures()
            .iter()
            .map(|closure| (closure.element_id.as_str(), closure.logical_bounds.x()))
            .collect::<Vec<_>>(),
        [("one", 10.0), ("two", 30.0)]
    );
}

// --- SVG plan -------------------------------------------------------------

/// Builds the SVG plan with the complete official font chunks in place of
/// derived subsets: the subset itself is the font-subset tests' subject, and
/// skipping it keeps every other SVG plan test off the WOFF2 encoder.
fn svg_plan(scene: &SceneDocument, request: &RenderRequest) -> SvgPlan {
    let resolved = resolve_for_test(scene, request).expect("scene resolves");
    let text_order = svg_font_text_order(&resolved).expect("text order is enumerable");
    let demand = svg_font_demand(&resolved, &text_order).expect("font demand is enumerable");
    let mut session = svg_fonts::Session::new();
    let bundle = build_svg_font_bundle(demand, svg_fonts::Payload::CompleteChunks, &mut session)
        .expect("complete chunks bundle without subsetting");
    assert_eq!(
        session.stats(),
        (0, 0),
        "no subset was derived or looked up"
    );
    SvgPlan::new(resolved, request, bundle).expect("SVG plan builds")
}

#[test]
fn svg_plan_defines_each_shared_frame_clip_once_and_measures_exactly_what_it_encodes() {
    // The SVG plan's only observable surface is its serialization: `measure`
    // and `encode_exact` are its accessors, and what this reads off them is
    // the plan's two decisions, not the encoder's byte layout.
    let plan = svg_plan(&scene(NAMED_FRAME_SCENE), &request(OutputFormat::Svg));
    // The frame and its three clipped cells; nothing is dropped.
    assert_eq!(
        (0..plan.source_count())
            .map(|index| plan.source(index).expect("indexed source").0)
            .collect::<Vec<_>>(),
        [
            Some("clip-frame"),
            Some("cell-0"),
            Some("cell-1"),
            Some("cell-2")
        ]
    );

    // Spec §13.2: the measurement pass predicts the emission pass exactly.
    let measured = plan.measure().expect("the plan measures");
    let encoded = plan.encode_exact(measured).expect("the plan encodes");
    assert_eq!(encoded.len(), measured);
    let svg = std::str::from_utf8(&encoded).expect("SVG is UTF-8");

    // The one clip the three children share is defined once and referenced by
    // each of them, keyed by the shared frame source index.
    assert_eq!(svg.matches("<clipPath").count(), 1);
    assert_eq!(svg.matches("<clipPath id=\"frame-clip-0\">").count(), 1);
    assert_eq!(
        svg.matches("<g clip-path=\"url(#frame-clip-0)\">").count(),
        3
    );
    // The frame name's face is the complete Liberation Sans chunk under its
    // reserved-name alias, not a derived subset.
    assert!(svg.contains("<text data-frame-name=\"clip-frame\""));
    assert!(svg.contains("font-family:'ExcalidrawRender-9'"));
}

#[test]
fn svg_plan_defines_each_shared_image_once_and_measures_exactly_what_it_encodes() {
    // Three image elements over two file records that carry the same PNG
    // bytes and one nested SVG: two semantic asset identities, the first drawn
    // by elements 0 and 2, the second by element 1.
    let png = "data:image/png;base64,\
               iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
    let nested = data_url("image/svg+xml", BLURRED_RECT);
    let scene = scene(
        format!(
            r#"{{
            "type":"excalidraw","version":2,
            "elements":[
                {{"id":"first","type":"image","x":0,"y":0,"width":20,"height":10,"fileId":"one","status":"saved","scale":[1,1],"seed":1,"updated":1}},
                {{"id":"nested","type":"image","x":30,"y":0,"width":20,"height":10,"fileId":"svg","status":"saved","scale":[1,1],"seed":2,"updated":1}},
                {{"id":"second","type":"image","x":60,"y":0,"width":40,"height":10,"fileId":"two","status":"saved","scale":[-1,1],"seed":3,"updated":1}}
            ],
            "appState":{{}},
            "files":{{
                "one":{{"id":"one","mimeType":"image/png","dataURL":"{png}","created":1}},
                "two":{{"id":"two","mimeType":"image/png","dataURL":"{png}","created":1}},
                "svg":{{"id":"svg","mimeType":"image/svg+xml","dataURL":"{nested}","created":1}}
            }}
        }}"#
        )
        .as_bytes(),
    );
    let plan = svg_plan(&scene, &request(OutputFormat::Svg));
    assert_eq!(plan.source_count(), 3);

    // Spec §13.2: the measurement pass predicts the emission pass exactly, so
    // the output-bytes charge is the shared document's size.
    let measured = plan.measure().expect("the plan measures");
    let encoded = plan.encode_exact(measured).expect("the plan encodes");
    assert_eq!(encoded.len(), measured);
    let svg = std::str::from_utf8(&encoded).expect("SVG is UTF-8");

    // One definition per semantic asset identity, keyed by the first drawing
    // element's source index, in first-use order; one `<use>` per element.
    assert_eq!(svg.matches("<symbol ").count(), 2);
    assert_eq!(svg.matches("<image ").count(), 2);
    let first_symbol = svg
        .find("<symbol id=\"image-0\">")
        .expect("the PNG is defined");
    let second_symbol = svg
        .find("<symbol id=\"image-1\">")
        .expect("the SVG is defined");
    assert!(first_symbol < second_symbol);
    assert_eq!(svg.matches("<use href=\"#image-0\" ").count(), 2);
    assert_eq!(svg.matches("<use href=\"#image-1\" ").count(), 1);
    assert!(svg.contains("<use href=\"#image-0\" width=\"20\" height=\"10\"/>"));
    assert!(svg.contains("<use href=\"#image-0\" width=\"40\" height=\"10\"/>"));
    assert!(svg.contains(&format!("<image href=\"{png}\" ")));
    assert_eq!(svg.matches(&png).count(), 1);
}
