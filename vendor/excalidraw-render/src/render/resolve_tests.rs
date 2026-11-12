//! Resolve-level tests: pre-encode behavior asserted once on the
//! format-neutral semantic graph (spec §9) that `resolve_for_test` returns,
//! rather than once per output format on the encoded bytes. Everything a
//! format then does with the graph is that format's projection policy and is
//! tested at its plan (`plan_tests`) or by the one three-format smoke each
//! behavior family keeps end to end.

use super::geometry::FRAME_RADIUS;
use super::resolve::{
    ResolvedClip, ResolvedElement, ResolvedFallback, ResolvedFrame, ResolvedImage,
    ResolvedImageCropPlacement, ResolvedImageProjection, ResolvedImageSourceRect,
    ResolvedLabelMask, ResolvedOutline, ResolvedTextElement, ResolvedTransform,
};
use super::*;
use crate::color::CanonicalColor;
use crate::{ColorMode, FrameOptions};

pub(super) fn scene(json: &[u8]) -> SceneDocument {
    SceneDocument::parse(json).expect("test scene is admitted")
}

pub(super) fn request(format: OutputFormat) -> RenderRequest {
    RenderRequest::builder(format)
        .build()
        .expect("default request is valid")
}

/// Resolves under a default SVG request: the format only matters once a plan
/// projects the graph, except where a test says otherwise.
fn resolve(scene: &SceneDocument) -> ResolvedScene {
    resolve_with(scene, &request(OutputFormat::Svg))
}

fn resolve_with(scene: &SceneDocument, request: &RenderRequest) -> ResolvedScene {
    resolve_for_test(scene, request).expect("scene resolves")
}

#[test]
fn a_coordinate_rejected_occurrence_still_fires_the_cjk_subset_trigger() {
    let scene = scene(br#"{"type":"excalidraw","version":2,"elements":[
        {"id":"trigger","type":"text","x":99,"width":2,"height":10,"fontSize":10,"fontFamily":5,"text":"\u4e00","updated":0},
        {"id":"survivor","type":"text","width":20,"height":10,"fontSize":10,"fontFamily":5,"text":"A","updated":0}
    ],"appState":{},"files":{}}"#);
    let request = RenderRequest::builder(OutputFormat::Svg)
        .padding(0.0)
        .resource_limits(
            crate::ResourceLimits::builder()
                .max_coordinate_magnitude(100.0)
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    let resolved = resolve_with(&scene, &request);
    assert!(resolved.cjk_subset_trigger.selects_xiaolai());
    assert_eq!(paint_order(&resolved), vec![Some("survivor")]);
}

/// The resolved elements' ids in paint order.
fn paint_order(resolved: &ResolvedScene) -> Vec<Option<&str>> {
    (0..resolved.source_count())
        .map(|index| resolved.source(index).expect("indexed source").0)
        .collect()
}

/// The one element `pick` selects by id, or a panic that prints the graph.
fn element<'a, T>(
    resolved: &'a ResolvedScene,
    id: &str,
    pick: impl Fn(&'a ResolvedElement) -> Option<(&'a str, &'a T)>,
) -> &'a T {
    resolved
        .elements
        .iter()
        .find_map(|element| pick(element).filter(|(candidate, _)| *candidate == id))
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("{id} resolves: {resolved:#?}"))
}

fn text<'a>(resolved: &'a ResolvedScene, id: &str) -> &'a ResolvedTextElement {
    element(resolved, id, |element| match element {
        ResolvedElement::Text(text) => Some((text.source.id(), text)),
        _ => None,
    })
}

fn outline<'a>(resolved: &'a ResolvedScene, id: &str) -> &'a ResolvedOutline {
    element(resolved, id, |element| match element {
        ResolvedElement::Outline(outline) => Some((outline.source.id(), outline)),
        _ => None,
    })
}

fn frame<'a>(resolved: &'a ResolvedScene, id: &str) -> &'a ResolvedFrame {
    element(resolved, id, |element| match element {
        ResolvedElement::Frame(frame) => Some((frame.source.id(), frame)),
        _ => None,
    })
}

fn approx(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1e-9
}

/// Every element's frame clips, by element id, in paint order.
fn frame_clips(resolved: &ResolvedScene) -> Vec<(Option<&str>, &[ResolvedClip])> {
    resolved
        .elements
        .iter()
        .enumerate()
        .map(|(index, element)| {
            let id = resolved.source(index).expect("indexed source").0;
            let clips = match element {
                ResolvedElement::Outline(outline) => outline.frame_clips.as_slice(),
                ResolvedElement::Freedraw(freedraw) => freedraw.frame_clips.as_slice(),
                ResolvedElement::Frame(frame) => frame.frame_clips.as_slice(),
                ResolvedElement::Text(text) => text.frame_clips.as_slice(),
                ResolvedElement::Image(image) => image.frame_clips.as_slice(),
                ResolvedElement::ImageFallback(image) => image.frame_clips.as_slice(),
                ResolvedElement::Fallback(fallback) => fallback.frame_clips.as_slice(),
            };
            (id, clips)
        })
        .collect()
}

fn frame_request(frame_options: FrameOptions) -> RenderRequest {
    RenderRequest::builder(OutputFormat::Svg)
        .frame_options(frame_options)
        .build()
        .expect("frame request is valid")
}

// --- Bound text placement -------------------------------------------------

#[test]
fn shape_bound_text_inherits_the_container_rotation() {
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"box","type":"rectangle","x":0,"y":0,"width":100,"height":60,"angle":0.5,"boundElements":[{"type":"text","id":"label"}],"roughness":0,"updated":1},
            {"id":"label","type":"text","x":20,"y":20,"width":60,"height":20,"angle":0,"containerId":"box","fontSize":16,"fontFamily":5,"text":"Bound","textAlign":"center","verticalAlign":"middle","lineHeight":1.25,"updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    let label = text(&resolved, "label").transform;
    let box_transform = outline(&resolved, "box").transform;
    assert_eq!(
        label.angle_radians, 0.5,
        "the label turns with its container"
    );
    assert_eq!((label.center_x, label.center_y), (30.0, 10.0));
    // The label keeps its own stored offset inside the container.
    assert!(approx(
        label.translation_x - box_transform.translation_x,
        20.0
    ));
    assert!(approx(
        label.translation_y - box_transform.translation_y,
        20.0
    ));
}

#[test]
fn shape_bound_text_paints_at_its_stored_coordinates() {
    // The label is stored at (400, 400), far outside its 120x80 container, and
    // paints there rather than being re-derived from the container box and the
    // align values. focused.bound-text-probe.v1 holds the official evidence: a
    // label stored outside its container renders outside it and expands the
    // scene bounds to include it.
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"box","type":"rectangle","x":0,"y":0,"width":120,"height":80,"boundElements":[{"type":"text","id":"label"}],"roughness":0,"updated":1},
            {"id":"label","type":"text","x":400,"y":400,"width":30,"height":20,"containerId":"box","fontSize":16,"fontFamily":5,"text":"Fit","textAlign":"right","verticalAlign":"bottom","lineHeight":1.25,"updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    let label = text(&resolved, "label").transform;
    // Stored coordinates plus the 10 px export padding.
    assert_eq!((label.translation_x, label.translation_y), (410.0, 410.0));
    assert_eq!(
        (resolved.logical_width, resolved.logical_height),
        (450.0, 440.0),
        "the scene bounds include the label"
    );
}

#[test]
fn arrow_bound_text_uses_the_resolved_arrow_midpoint() {
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"arrow","type":"arrow","x":0,"y":20,"width":100,"height":0,"points":[[0,0],[100,0]],"boundElements":[{"type":"text","id":"label"}],"roughness":0,"endArrowhead":"arrow","updated":1},
            {"id":"label","type":"text","x":400,"y":400,"width":40,"height":20,"angle":0,"containerId":"arrow","fontSize":16,"fontFamily":5,"text":"Label","textAlign":"center","verticalAlign":"middle","lineHeight":1.25,"updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    assert!(
        resolved.logical_width < 150.0,
        "stale text coordinates do not expand the bounds: {}",
        resolved.logical_width
    );
    let arrow = outline(&resolved, "arrow");
    let label = text(&resolved, "label");
    // The label is centred on the arrow's midpoint (50, 20) in scene space.
    let label_center_x = label.transform.translation_x + label.width / 2.0;
    let label_center_y = label.transform.translation_y + label.height / 2.0;
    assert!(approx(label_center_x - arrow.transform.translation_x, 50.0));
    assert!(approx(label_center_y - arrow.transform.translation_y, 0.0));
    let mask = arrow.label_mask.expect("the arrow masks its label's box");
    // The mask is the label's canvas box.
    assert_eq!(
        mask,
        ResolvedLabelMask {
            x: label.transform.translation_x,
            y: label.transform.translation_y,
            width: 40.0,
            height: 20.0,
        }
    );
}

#[test]
fn bound_text_paints_with_its_reciprocal_container() {
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"label","type":"text","x":10,"y":10,"width":40,"height":20,"containerId":"box","fontSize":16,"fontFamily":5,"text":"Label","textAlign":"center","verticalAlign":"middle","lineHeight":1.25,"updated":1},
            {"id":"box","type":"rectangle","x":0,"y":0,"width":60,"height":40,"boundElements":[{"type":"text","id":"label"}],"strokeColor":"#e03131","roughness":0,"updated":1},
            {"id":"later","type":"rectangle","x":80,"y":0,"width":20,"height":20,"strokeColor":"#1971c2","roughness":0,"updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    assert_eq!(
        paint_order(&resolved),
        [Some("box"), Some("label"), Some("later")],
        "the label paints right after its container even when it precedes it in source order"
    );
}

#[test]
fn one_way_container_reference_does_not_create_an_arrow_mask_or_label() {
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"arrow","type":"arrow","x":0,"y":20,"width":100,"height":0,"points":[[0,0],[100,0]],"roughness":0,"updated":1},
            {"id":"label","type":"text","x":30,"y":10,"width":40,"height":20,"containerId":"arrow","fontSize":16,"fontFamily":5,"text":"Hidden","textAlign":"center","verticalAlign":"middle","lineHeight":1.25,"updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    assert_eq!(paint_order(&resolved), [Some("arrow")]);
    assert!(outline(&resolved, "arrow").label_mask.is_none());
}

#[test]
fn arrow_binding_does_not_turn_standalone_text_into_a_bound_label() {
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"label","type":"text","x":0,"y":0,"width":100,"height":20,"containerId":null,"boundElements":[{"id":"arrow","type":"arrow"}],"fontSize":16,"fontFamily":5,"text":"Standalone","lineHeight":1.25,"updated":1},
            {"id":"arrow","type":"arrow","x":120,"y":10,"width":80,"height":0,"points":[[0,0],[80,0]],"startBinding":{"elementId":"label","focus":0,"gap":1},"roughness":0,"updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    // Standalone text keeps its source-order position rather than following
    // the arrow as a bound label would.
    assert_eq!(paint_order(&resolved), [Some("label"), Some("arrow")]);
    let label = text(&resolved, "label").transform;
    assert_eq!((label.translation_x, label.translation_y), (10.0, 10.0));
    assert!(outline(&resolved, "arrow").label_mask.is_none());
}

// --- Frames ---------------------------------------------------------------

pub(super) const NESTED_FRAME_SCENE: &[u8] = br##"{
    "type":"excalidraw","version":2,
    "elements":[
        {"id":"outer","type":"frame","x":0,"y":0,"width":100,"height":60,"name":"Outer","updated":1},
        {"id":"inner","type":"frame","x":30,"y":10,"width":50,"height":40,"frameId":"outer","opacity":50,"updated":1},
        {"id":"child","type":"rectangle","x":60,"y":20,"width":80,"height":20,"frameId":"inner","strokeColor":"#e03131","backgroundColor":"#e03131","fillStyle":"solid","roughness":0,"opacity":50,"updated":1}
    ],
    "appState":{},"files":{}
}"##;

/// The #115 scene: one named 580x580 frame clipping three filled rectangles,
/// the last of which straddles the frame's right edge.
pub(super) const NAMED_FRAME_SCENE: &[u8] = br##"{
    "type":"excalidraw","version":2,
    "elements":[
        {"id":"clip-frame","type":"frame","name":"Clip","x":0,"y":0,"width":580,"height":580,"roughness":0,"updated":1},
        {"id":"cell-0","type":"rectangle","x":35,"y":35,"width":30,"height":30,"frameId":"clip-frame","strokeColor":"#1971c2","backgroundColor":"#a5d8ff","fillStyle":"solid","roughness":0,"updated":1},
        {"id":"cell-1","type":"rectangle","x":75,"y":35,"width":30,"height":30,"frameId":"clip-frame","strokeColor":"#1971c2","backgroundColor":"#a5d8ff","fillStyle":"solid","roughness":0,"updated":1},
        {"id":"cell-2","type":"rectangle","x":555,"y":35,"width":30,"height":30,"frameId":"clip-frame","strokeColor":"#1971c2","backgroundColor":"#a5d8ff","fillStyle":"solid","roughness":0,"updated":1}
    ],
    "appState":{},"files":{}
}"##;

/// The first baseline Excalidraw's Helvetica metadata metrics (`2048`, `1577`,
/// `-471`) give a 14 px line in the 17.5 px line box `FRAME_STYLE.nameFontSize`
/// 14 and `nameLineHeight` 1.25 make: `17.5 / 2 + 14 * (1577 - 471) / 2048 / 2`.
/// The box sits `nameOffsetY` 3 px above the frame, so its top is 20.5 px above
/// the frame's top edge.
const FRAME_NAME_BASELINE: f64 = 12.530_273_437_5;

#[test]
fn frame_name_box_extends_the_canvas_by_the_reference_metrics() {
    let resolved = resolve(&scene(NAMED_FRAME_SCENE));
    assert_eq!(
        (resolved.logical_width, resolved.logical_height),
        (600.0, 620.5),
        "the name box is 20.5 px tall above the frame"
    );
}

#[test]
fn frame_name_is_left_aligned_helvetica_text_above_the_frame() {
    let resolved = resolve(&scene(NAMED_FRAME_SCENE));
    let frame = frame(&resolved, "clip-frame");
    let name = frame.name.as_ref().expect("the frame resolves its name");

    // The name is an unrotated text element at the frame's x, 20.5 px above
    // its y; after padding that is (10, 10) with the frame at (10, 30.5).
    assert_eq!(
        (name.transform.translation_x, name.transform.translation_y),
        (10.0, 10.0)
    );
    assert_eq!(name.transform.angle_radians, 0.0);
    assert_eq!(
        (
            frame.transform.translation_x,
            frame.transform.translation_y,
            frame.transform.center_x,
            frame.transform.center_y
        ),
        (10.0, 30.5, 290.0, 290.0)
    );
    assert_eq!(name.color, CanonicalColor::from_rgb8(0x99, 0x99, 0x99));
    assert_eq!(name.label.text, "Clip");
    assert_eq!(name.label.display_text, "Clip");
    let first = name.label.glyphs.first().expect("the name has glyphs");
    assert!(
        first.x.abs() < 1e-9,
        "left aligned at the frame's x, not centered: {}",
        first.x
    );
    assert!(
        approx(name.label.baseline, FRAME_NAME_BASELINE),
        "baseline from Helvetica's metadata metrics: {}",
        name.label.baseline
    );
}

#[test]
fn frame_name_uses_the_dark_theme_name_color_in_dark_mode() {
    for (format, expected) in [
        // SVG keeps `FRAME_STYLE.nameColorDarkTheme` #7a7a7a under its root
        // filter, as the reference SVG does.
        (
            OutputFormat::Svg,
            CanonicalColor::from_rgb8(0x7a, 0x7a, 0x7a),
        ),
        // PNG and PDF lower the dark-mode filter into it.
        (
            OutputFormat::Png,
            CanonicalColor::from_rgb8(0x84, 0x84, 0x84),
        ),
        (
            OutputFormat::Pdf,
            CanonicalColor::from_rgb8(0x84, 0x84, 0x84),
        ),
    ] {
        let request = RenderRequest::builder(format)
            .color_mode(ColorMode::Dark)
            .build()
            .expect("dark request is valid");
        let resolved = resolve_with(&scene(NAMED_FRAME_SCENE), &request);
        let name = frame(&resolved, "clip-frame")
            .name
            .as_ref()
            .expect("the frame resolves its name");
        assert_eq!(name.color, expected, "{format:?}");
    }
}

#[test]
fn frame_name_is_truncated_to_the_frame_width_with_dots() {
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"narrow","type":"frame","name":"A very long frame name that cannot fit","x":0,"y":0,"width":60,"height":40,"updated":1},
            {"id":"wide","type":"frame","name":"Fits","x":100,"y":0,"width":200,"height":40,"updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    let narrow = frame(&resolved, "narrow");
    let name = narrow.name.as_ref().expect("the narrow frame keeps a name");
    assert!(
        name.label.text.ends_with("..."),
        "the name is cut with `...`: {:?}",
        name.label.text
    );
    assert!(name.label.text.len() < "A very long frame name that cannot fit".len());
    // The synthesized element is as wide as the frame once truncated, so its
    // rotation centre is at the frame's half width.
    assert_eq!(name.transform.center_x, 30.0);

    let wide = frame(&resolved, "wide").name.as_ref().expect("name");
    assert_eq!(wide.label.text, "Fits");
    assert!(
        wide.transform.center_x < 30.0,
        "measured advance, not the frame width"
    );
}

#[test]
fn frame_name_is_neither_rotated_nor_clipped_with_its_frame() {
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"outer","type":"frame","x":0,"y":0,"width":200,"height":200,"updated":1},
            {"id":"tilted","type":"frame","name":"Tilted","x":50,"y":50,"width":100,"height":60,"angle":0.5,"frameId":"outer","updated":1}
        ],
        "appState":{},"files":{}
    }"##));

    let tilted = frame(&resolved, "tilted");
    assert_eq!(tilted.transform.angle_radians, 0.5);
    assert_eq!(
        tilted
            .frame_clips
            .iter()
            .map(|clip| clip.frame_source_index)
            .collect::<Vec<_>>(),
        [0],
        "the nested frame is clipped by the outer frame"
    );
    // The name is a synthesized text element of its own: unrotated, and
    // (`ResolvedFrameName` carries no clips) outside every frame clip.
    let name = tilted
        .name
        .as_ref()
        .expect("the tilted frame keeps its name");
    assert_eq!(name.transform.angle_radians, 0.0);
}

#[test]
fn every_child_shares_the_frames_one_rounded_clip() {
    let resolved = resolve(&scene(NAMED_FRAME_SCENE));

    let frame = frame(&resolved, "clip-frame");
    let outline = frame
        .outline
        .as_ref()
        .expect("the frame paints its outline");
    assert_eq!(
        outline.stroke_color,
        CanonicalColor::from_rgb8(0xbb, 0xbb, 0xbb)
    );
    assert!(
        frame.frame_clips.is_empty(),
        "a root frame is clipped by nothing"
    );

    let expected = ResolvedClip::for_frame(0, 580.0, 580.0, true, frame.transform)
        .expect("the frame's own clip");
    for (id, clips) in frame_clips(&resolved) {
        if id == Some("clip-frame") {
            continue;
        }
        let [clip] = clips else {
            panic!("{id:?} carries exactly one clip: {clips:#?}");
        };
        assert_eq!(clip.frame_source_index, 0, "{id:?}");
        assert_eq!(
            (clip.width, clip.height, clip.radius),
            (580.0, 580.0, FRAME_RADIUS)
        );
        assert_eq!(clip.transform, frame.transform, "{id:?}");
        assert_eq!(
            clip.path, expected.path,
            "{id:?} shares the frame's outline"
        );
    }
}

#[test]
fn a_nested_frames_name_extends_the_scene_bounds_like_any_root_element() {
    // The inner frame is a clipped child of the outer one, so its own box
    // never widens the scene; its name belongs to no frame and does, as the
    // reference's synthesized text element has no frameId.
    let resolved = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"outer","type":"frame","x":0,"y":0,"width":200,"height":200,"updated":1},
            {"id":"inner","type":"frame","name":"Inner","x":100,"y":10,"width":150,"height":60,"frameId":"outer","updated":1}
        ],
        "appState":{"frameRendering":{"enabled":true,"clip":true,"name":true,"outline":true}},
        "files":{}
    }"##));
    // Outer frame 0..200 with its own name above it: width 220 and, with the
    // inner name's box top at y -10.5 inside the outer name's 20.5 px reach,
    // height 200 + 20.5 + 20.
    assert_eq!(
        (resolved.logical_width, resolved.logical_height),
        (220.0, 240.5)
    );

    let unnamed_outer = resolve(&scene(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"outer","type":"frame","x":0,"y":0,"width":200,"height":200,"updated":1},
            {"id":"inner","type":"frame","name":"Inner","x":100,"y":100,"width":150,"height":60,"frameId":"outer","updated":1}
        ],
        "appState":{},"files":{}
    }"##));
    // The inner name is "Inner" at 14 px, so it never reaches x 200; the
    // inner frame's own overflow to x 250 is clipped away and adds nothing.
    assert_eq!(
        (unnamed_outer.logical_width, unnamed_outer.logical_height),
        (220.0, 240.5)
    );
}

#[test]
fn frame_controls_are_independent() {
    let scene = scene(NESTED_FRAME_SCENE);

    let unclipped = resolve_with(
        &scene,
        &frame_request(FrameOptions::default().with_clip(false)),
    );
    assert!(frame_clips(&unclipped)
        .iter()
        .all(|(_, clips)| clips.is_empty()));
    let outer = frame(&unclipped, "outer");
    assert!(outer.outline.is_some() && outer.name.is_some());

    let clipped = resolve_with(&scene, &frame_request(FrameOptions::default()));
    assert!(
        unclipped.logical_width > clipped.logical_width,
        "the unclipped child overflow widens the scene"
    );
    assert_eq!(
        frame_clips(&clipped)
            .iter()
            .map(|(id, clips)| (
                *id,
                clips
                    .iter()
                    .map(|clip| clip.frame_source_index)
                    .collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>(),
        [
            (Some("outer"), vec![]),
            (Some("inner"), vec![0]),
            (Some("child"), vec![1]),
        ],
        "one clip per frame: the outer frame clips the inner, the inner clips the child"
    );

    let no_decorations = resolve_with(
        &scene,
        &frame_request(
            FrameOptions::default()
                .with_show_name(false)
                .with_outline(false),
        ),
    );
    assert!(!frame_clips(&no_decorations)[2].1.is_empty());
    let outer = frame(&no_decorations, "outer");
    assert!(outer.outline.is_none() && outer.name.is_none());

    let disabled = resolve_with(
        &scene,
        &frame_request(FrameOptions::default().with_enabled(false)),
    );
    assert_eq!(
        paint_order(&disabled),
        [Some("child")],
        "no frame element is emitted"
    );
    assert!(frame_clips(&disabled)[0].1.is_empty());
    assert_eq!(
        outline(&disabled, "child").opacity,
        0.25,
        "the frame's opacity still multiplies its child's"
    );
}

// --- Compatibility fallbacks ----------------------------------------------

fn fallback<'a>(resolved: &'a ResolvedScene, id: &str) -> &'a ResolvedFallback {
    element(resolved, id, |element| match element {
        ResolvedElement::Fallback(fallback) => {
            fallback.source.id.as_deref().map(|id| (id, fallback))
        }
        _ => None,
    })
}

#[test]
fn compatibility_labels_use_full_ellipsis_or_omitted_fit_behavior() {
    let resolved = resolve(&scene(
        br#"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"wide","type":"future","x":0,"y":0,"width":200,"height":60,"updated":1},
            {"id":"narrow","type":"","x":220,"y":0,"width":20,"height":30,"updated":1},
            {"id":"tiny","type":"future","x":260,"y":0,"width":8,"height":8,"updated":1}
        ],
        "appState":{},"files":{}
    }"#,
    ));

    // The whole label fits the wide placeholder.
    let wide = fallback(&resolved, "wide")
        .label
        .as_ref()
        .expect("the wide placeholder is labelled");
    assert_eq!(wide.text, "Unsupported: future");
    assert_eq!(wide.display_text, "Unsupported: future");
    assert!(wide.missing_codepoints.is_empty());

    // Too narrow for the label: the ellipsis alone.
    let narrow = fallback(&resolved, "narrow")
        .label
        .as_ref()
        .expect("the narrow placeholder is labelled");
    assert_eq!(narrow.text, "\u{2026}");
    assert_eq!(narrow.display_text, "\u{2026}");

    // Shorter than the 14 px label: no label at all.
    let tiny = fallback(&resolved, "tiny");
    assert!(tiny.label.is_none(), "tiny label: {:?}", tiny.label);
    assert_eq!((tiny.width, tiny.height), (8.0, 8.0));
}

#[test]
fn compatibility_placeholder_omits_components_that_do_not_fit_source_bounds() {
    let resolved = resolve(&scene(
        br#"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"tiny","type":"future","x":0,"y":0,"width":1,"height":1,"updated":1},
            {"id":"small","type":"future","x":10,"y":0,"width":2,"height":2,"updated":1}
        ],
        "appState":{},"files":{}
    }"#,
    ));

    // Under 2 x 2 only the fill rectangle is painted (5 path commands); from
    // 2 x 2 the dashed stroke and the cross join it (9), each format drawing
    // the same components the resolved count prices.
    let tiny = fallback(&resolved, "tiny");
    assert!(tiny.label.is_none());
    assert_eq!(tiny.geometry_command_count(), 5);
    let small = fallback(&resolved, "small");
    assert!(small.label.is_none());
    assert_eq!(small.geometry_command_count(), 9);
}

#[test]
fn generated_labels_report_missing_glyphs_through_the_liberation_substitute() {
    let resolved = resolve(&scene(br#"{
        "type":"excalidraw","version":2,
        "elements":[{"id":"emoji","type":"future\ud83d\ude00","x":0,"y":0,"width":200,"height":60,"updated":1}],
        "appState":{},"files":{}
    }"#));

    let label = fallback(&resolved, "emoji")
        .label
        .as_ref()
        .expect("the placeholder is labelled");
    assert_eq!(label.text, "Unsupported: future\u{1f600}");
    assert_eq!(label.display_text, "Unsupported: future?");
    assert_eq!(label.missing_codepoints, [0x1f600]);
}

// --- CJK subset trigger ---------------------------------------------------

fn excalifont_scene(text: &str) -> SceneDocument {
    let source = format!(
        "{{\"type\":\"excalidraw\",\"version\":2,\"elements\":[{{\"id\":\"text\",\"type\":\"text\",\"width\":160,\"height\":40,\"fontSize\":20,\"fontFamily\":5,\"text\":{},\"updated\":1}}],\"appState\":{{}},\"files\":{{}}}}",
        serde_json::to_string(text).expect("test text serializes")
    );
    scene(source.as_bytes())
}

/// Every run of a text element as `(face family, shown text)`, line by line.
fn runs(text: &ResolvedTextElement) -> Vec<(&'static str, &str)> {
    text.text
        .lines
        .iter()
        .flat_map(|line| line.runs.iter())
        .map(|run| (run.font_family, run.display_text.as_str()))
        .collect()
}

#[test]
fn excalifont_uses_ordered_cjk_and_emoji_fallbacks() {
    let resolved = resolve(&excalifont_scene("A\u{4e2d}\u{1f600}"));
    assert!(resolved.cjk_subset_trigger.selects_xiaolai());
    let text = text(&resolved, "text");
    assert_eq!(
        runs(text),
        [
            ("Excalifont", "A"),
            ("Xiaolai", "\u{4e2d}"),
            ("Noto Emoji", "\u{1f600}"),
        ]
    );
    assert!(text.text.noto_emoji_substituted);
    assert!(text.text.missing_codepoints.is_empty());
}

#[test]
fn xiaolai_only_joins_excalifont_fallback_when_the_cjk_subset_trigger_fires() {
    // U+20A9 WON SIGN is outside every Excalifont declared range and outside
    // the frozen CJK scalar table, yet inside a Xiaolai chunk's declared range.
    // Without a CJK scalar in Excalifont demand, Xiaolai is not selected, so the
    // sign is a missing codepoint shown by the Liberation Sans replacement
    // rather than a Xiaolai glyph.
    let resolved = resolve(&excalifont_scene("A\u{20a9}"));
    assert!(!resolved.cjk_subset_trigger.selects_xiaolai());
    let text = text(&resolved, "text");
    assert_eq!(runs(text), [("Excalifont", "A"), ("Liberation Sans", "?")]);
    assert_eq!(text.text.missing_codepoints, [0x20a9]);
    assert!(
        !resolved
            .official_fonts
            .iter()
            .any(|font| font.identity.starts_with("fonts/Xiaolai/")),
        "no format declares a face the trigger did not select"
    );
}

#[test]
fn the_cjk_subset_trigger_extends_xiaolai_to_every_excalifont_text_in_the_scene() {
    // The trigger is evaluated over the scene's complete Excalifont demand, so
    // a CJK scalar in a later element already lets Xiaolai serve the won sign in
    // an earlier one.
    let resolved = resolve(&scene(
        br#"{"type":"excalidraw","version":2,"elements":[
            {"id":"won","type":"text","width":160,"height":40,"fontSize":20,"fontFamily":5,"text":"\u20a9","updated":1},
            {"id":"han","type":"text","x":180,"width":160,"height":40,"fontSize":20,"fontFamily":5,"text":"\u4e2d","updated":2}
        ],"appState":{},"files":{}}"#,
    ));
    assert!(resolved.cjk_subset_trigger.selects_xiaolai());
    for id in ["won", "han"] {
        let text = text(&resolved, id);
        assert!(
            runs(text).iter().all(|(family, _)| *family == "Xiaolai"),
            "{id}: {:?}",
            runs(text)
        );
        assert!(text.text.missing_codepoints.is_empty(), "{id}");
    }
    // The won sign and the han character live in different Xiaolai chunks;
    // each selected chunk is resolved once for the scene.
    let xiaolai_chunks = resolved
        .official_fonts
        .iter()
        .filter(|font| font.identity.starts_with("fonts/Xiaolai/"))
        .map(|font| font.identity)
        .collect::<Vec<_>>();
    assert_eq!(xiaolai_chunks.len(), 2, "{xiaolai_chunks:?}");
    assert_ne!(xiaolai_chunks[0], xiaolai_chunks[1]);
}

#[test]
fn excalifont_cjk_text_uses_bundled_xiaolai_without_host_fallback() {
    let resolved = resolve(&excalifont_scene("\u{6f22}\u{6f22}A"));
    let text = text(&resolved, "text");
    assert_eq!(
        runs(text),
        [("Xiaolai", "\u{6f22}\u{6f22}"), ("Excalifont", "A")]
    );
    assert!(text.text.missing_codepoints.is_empty());
    assert!(!text.text.noto_emoji_substituted);
}

// --- Image crop and scale mapping -----------------------------------------

fn image<'a>(resolved: &'a ResolvedScene, id: &str) -> &'a ResolvedImage {
    element(resolved, id, |element| match element {
        ResolvedElement::Image(image) => Some((image.source.id(), image)),
        _ => None,
    })
}

fn fixture_png(width: u32, height: u32, pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = ::png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(::png::ColorType::Rgba);
        encoder.set_depth(::png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("fixture PNG header encodes");
        let rgba: Vec<u8> = pixels.iter().flatten().copied().collect();
        writer
            .write_image_data(&rgba)
            .expect("fixture PNG pixels encode");
    }
    bytes
}

/// A scene with one saved image element completed by `element_fields` over a
/// 3 x 2 PNG.
fn image_scene(element_fields: &str) -> SceneDocument {
    use base64::Engine as _;
    let png = fixture_png(
        3,
        2,
        &[
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [0, 255, 255, 255],
            [255, 0, 255, 255],
            [255, 255, 0, 255],
        ],
    );
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    scene(
        format!(
            r#"{{
            "type":"excalidraw","version":2,
            "elements":[{{
                "id":"image","type":"image","x":0,"y":0,"fileId":"asset","status":"saved",
                "seed":1,"updated":1,{element_fields}
            }}],
            "appState":{{}},"files":{{"asset":{{
                "id":"asset","mimeType":"image/png","dataURL":"{data_url}","created":1
            }}}}
        }}"#
        )
        .as_bytes(),
    )
}

#[test]
fn crop_and_negative_scale_map_the_source_rect_and_the_reference_viewport() {
    let resolved = resolve(&image_scene(
        r#""width":20,"height":20,"scale":[-1,-1],
           "crop":{"x":1,"y":0,"width":2,"height":2,"naturalWidth":3,"naturalHeight":2}"#,
    ));
    let image = image(&resolved, "image");

    // The canvas source rectangle is the crop in the asset's own pixel grid.
    assert_eq!(
        image.source_rect,
        ResolvedImageSourceRect {
            x: 1.0,
            y: 0.0,
            width: 2.0,
            height: 2.0,
        }
    );
    // The reference SVG draws the whole 3 x 2 source at its uncropped 30 x 20
    // size, shifted left by the 10 px crop offset, through the 20 x 20 viewport.
    assert_eq!(
        image.projection,
        ResolvedImageProjection {
            viewport_width: 20.0,
            viewport_height: 20.0,
            crop: Some(ResolvedImageCropPlacement {
                offset_x: 10.0,
                offset_y: 0.0,
                uncropped_width: 30.0,
                uncropped_height: 20.0,
            }),
        }
    );
    assert_eq!(image.projection.content_rect(), (-10.0, 0.0, 30.0, 20.0));
    assert_eq!(image.projection.viewport_rect(), (-10.0, 0.0, 30.0, 20.0));
    // The flip is carried as the element's scale about its unrounded centre.
    assert_eq!(image.scale, [-1, -1]);
    assert_eq!((image.width, image.height), (20.0, 20.0));
    assert_eq!(
        image.transform,
        ResolvedTransform {
            translation_x: 10.0,
            translation_y: 10.0,
            angle_radians: 0.0,
            center_x: 10.0,
            center_y: 10.0,
        }
    );
    assert!(image.corner_clip.is_none());
}

#[test]
fn a_crop_declared_against_a_larger_natural_size_scales_into_the_asset_grid() {
    // The crop record speaks in the file's natural 6 x 4 size; the asset the
    // renderer decoded is 3 x 2, so the source rectangle halves.
    let resolved = resolve(&image_scene(
        r#""width":20,"height":20,"scale":[1,1],
           "crop":{"x":2,"y":0,"width":4,"height":4,"naturalWidth":6,"naturalHeight":4}"#,
    ));
    let image = image(&resolved, "image");
    assert_eq!(
        image.source_rect,
        ResolvedImageSourceRect {
            x: 1.0,
            y: 0.0,
            width: 2.0,
            height: 2.0,
        }
    );
    // The reference's own arithmetic uses the natural size, so the viewport
    // placement is the same as for the 3 x 2 declaration.
    assert_eq!(image.projection.content_rect(), (-10.0, 0.0, 30.0, 20.0));
}

#[test]
fn an_uncropped_fractional_image_rounds_only_its_viewport() {
    let resolved = resolve(&image_scene(r#""width":40.5,"height":40.4,"scale":[1,1]"#));
    let image = image(&resolved, "image");

    assert_eq!(
        image.source_rect,
        ResolvedImageSourceRect {
            x: 0.0,
            y: 0.0,
            width: 3.0,
            height: 2.0,
        },
        "uncropped: the whole asset"
    );
    assert_eq!(
        image.projection,
        ResolvedImageProjection {
            viewport_width: 41.0,
            viewport_height: 40.0,
            crop: None,
        },
        "the viewport is the only rounded image value (spec §8.6)"
    );
    assert_eq!((image.width, image.height), (40.5, 40.4));
    assert_eq!(
        (image.transform.center_x, image.transform.center_y),
        (20.25, 20.2)
    );
}

#[test]
fn the_corner_clip_stays_in_element_space_when_the_image_is_cropped_and_flipped() {
    let resolved = resolve(&image_scene(
        r#""width":20,"height":20,"scale":[-1,-1],"roundness":{"type":3,"value":8},
           "crop":{"x":1,"y":0,"width":2,"height":2,"naturalWidth":3,"naturalHeight":2}"#,
    ));
    let image = image(&resolved, "image");
    let clip = image
        .corner_clip
        .as_ref()
        .expect("a roundness record clips the corners");
    // `getCornerRadius(min(width, height))`: adaptive radius 8 capped at a
    // quarter of the 20 px side.
    assert_eq!(clip.radius, 5.0);
    let (min_x, min_y, max_x, max_y) = clip
        .path
        .cubic_bounds()
        .expect("the clip is a closed rounded rectangle");
    assert!(
        approx(min_x, 0.0) && approx(min_y, 0.0) && approx(max_x, 20.0) && approx(max_y, 20.0),
        "the clip is the element's own rectangle, before crop and flip: {clip:?}"
    );
}
