// SPDX-License-Identifier: MPL-2.0
//! Contract tests for the native scene builder. Whole-document parity with
//! Go is measured by scripts/scene-scoreboard.py --tier scenebuild; these
//! pin the individual contracts that parity depends on.

use d2_rs_scene::{Matrix, Point, Primitive};
use d2_rs_target::Diagram;

use crate::{LinkBudget, Options, build};

fn diagram(value: serde_json::Value) -> Diagram {
    serde_json::from_value(value).expect("diagram JSON")
}

fn probe_options() -> Options {
    Options {
        pad: Some(0),
        appendix: true,
        link_budget: LinkBudget {
            max_regions: 4096,
            max_string_bytes: 1 << 20,
        },
        ..Default::default()
    }
}

fn rectangle(id: &str, x: i64, label: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "type": "rectangle", "pos": {"x": x, "y": 0}, "width": 100, "height": 66,
        "opacity": 1, "strokeWidth": 2, "fill": "B6", "stroke": "B1",
        "label": label, "fontSize": 16, "fontFamily": "DEFAULT", "bold": true, "color": "N1",
        "labelWidth": 40, "labelHeight": 21, "labelPosition": "INSIDE_MIDDLE_CENTER",
        "zIndex": 0, "level": 1
    })
}

#[test]
fn inlined_matrix_point_fuses_the_first_product() {
    // stable/cross_arrowhead: crossTransform.E = -Rotate(pi/4).Point(origin).X
    // + width/2 is exactly 8.5 in Go; fusing the second product instead
    // yields 8.499999999999998.
    let origin = Point::new(8.5, 8.5);
    let rotated = Matrix::rotate(std::f64::consts::PI / 4.0).point(origin);
    assert_eq!(-rotated.x + 8.5, 8.5);
}

#[test]
fn go_folds_untyped_constants_exactly() {
    // -12*math.Pi/180 is rounded once from the exact value.
    assert_ne!(
        -12.0 * std::f64::consts::PI / 180.0,
        -0.209_439_510_239_319_56
    );
}

#[test]
fn build_rejects_nested_boards_and_unknown_themes() {
    let mut nested =
        diagram(serde_json::json!({"name": "", "root": {}, "layers": [{"name": "x", "root": {}}]}));
    assert_eq!(
        build(&nested, &probe_options()).unwrap_err().to_string(),
        "scene: diagram uses unsupported nested boards in a single scene"
    );
    nested.layers = None;
    let options = Options {
        theme_id: Some(12345),
        ..probe_options()
    };
    assert_eq!(
        build(&nested, &options).unwrap_err().to_string(),
        "scene: unknown theme 12345"
    );
}

#[test]
fn shape_label_materializes_glyphs_and_fonts() {
    let d = diagram(serde_json::json!({
        "name": "", "root": {"fill": "N7"}, "shapes": [rectangle("a", 0, "hello")], "connections": []
    }));
    let document = build(&d, &probe_options()).expect("build");
    let root = document.root.as_ref().expect("root");
    let ids: Vec<&str> = root.children().iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["root:background", "a"]);
    let label = &root.children()[1].children()[1];
    assert_eq!(label.id, "a:label:0");
    let Some(Primitive::TextRun(run)) = &label.primitive else {
        panic!("text run")
    };
    assert_eq!(run.glyphs.as_ref().map(Vec::len), Some(5));
    assert!(document.assets.contains_key("font:SourceSansPro:bold"));
    assert_eq!(document.view_box.width, document.logical_width);
}

#[test]
fn metadata_requires_a_link_budget() {
    let mut shape = rectangle("a", 0, "x");
    shape["link"] = "https://example.com".into();
    let d = diagram(serde_json::json!({"name": "", "root": {}, "shapes": [shape]}));
    let options = Options {
        link_budget: LinkBudget::default(),
        ..probe_options()
    };
    assert_eq!(
        build(&d, &options).unwrap_err().to_string(),
        "scene: options field \"linkBudget\" has invalid value {0 0}: must provide positive MaxRegions and MaxStringBytes for link/tooltip metadata"
    );
    let document = build(&d, &probe_options()).expect("build");
    let links = document.links.expect("links");
    assert_eq!(links[0].url, "https://example.com");
    // A bare link has no appendix text, so no badge layout exists.
    let ids: Vec<&str> = document
        .root
        .as_ref()
        .unwrap()
        .children()
        .iter()
        .map(|n| n.id.as_str())
        .collect();
    assert!(!ids.contains(&"appendix"));
    // A tooltip adds an appendix row, which numbers both badges.
    let mut shape = rectangle("a", 0, "x");
    shape["link"] = "https://example.com".into();
    shape["tooltip"] = "tip".into();
    let d = diagram(serde_json::json!({"name": "", "root": {}, "shapes": [shape]}));
    let document = build(&d, &probe_options()).expect("build");
    let ids: Vec<&str> = document
        .root
        .as_ref()
        .unwrap()
        .children()
        .iter()
        .map(|n| n.id.as_str())
        .collect();
    assert!(
        ids.contains(&"appendix:shape:0:tooltip")
            && ids.contains(&"appendix:shape:0:link")
            && ids.contains(&"appendix")
    );
}

#[test]
fn board_link_targets_are_classified() {
    let mut a = rectangle("a", 0, "x");
    a["link"] = "root.layers.x".into();
    let d = diagram(serde_json::json!({"name": "", "root": {}, "shapes": [a]}));
    let links = build(&d, &probe_options()).unwrap().links.unwrap();
    assert_eq!(
        (links[0].url.as_str(), links[0].target.as_str()),
        ("", "root.layers.x")
    );
}

#[test]
fn dangerous_links_follow_master_normalization() {
    use crate::markdown::{is_dangerous_link, safe_markdown_link};
    assert!(is_dangerous_link("java\tscript:alert(1)"));
    assert!(is_dangerous_link("&#106;avascript:alert(1)"));
    assert!(is_dangerous_link("%6Aavascript:alert(1)"));
    assert!(!is_dangerous_link("https://d2lang.com"));
    assert_eq!(safe_markdown_link("  root.html "), "root.html");
}

#[test]
fn builtin_paper_pattern_matches_its_topology() {
    let source = crate::pattern_sources::shared_paper_pattern_source().expect("paper source");
    assert_eq!(
        source.paths.len(),
        crate::pattern_sources::PAPER_PATTERN_PATH_COUNT
    );
    let commands: usize = source.paths.iter().map(|p| p.commands.len()).sum();
    assert_eq!(
        commands,
        crate::pattern_sources::PAPER_PATTERN_COMMAND_COUNT
    );
    assert_eq!(
        crate::pattern_sources::shared_sketch_streak_commands()
            .expect("streak")
            .len(),
        687
    );
}

#[test]
fn svg_path_parser_resolves_relative_and_smooth_commands() {
    use crate::svgimport_path::{PathLimits, parse_path};
    use d2_rs_scene::PathCommandKind;
    let limits = || PathLimits {
        max_bytes: 1024,
        max_commands: 64,
    };
    let commands = parse_path("t", "m1 2 l3 4 h1 v-1 s1 1 2 2 z", limits()).expect("path");
    let kinds: Vec<PathCommandKind> = commands.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [
            PathCommandKind::MOVE,
            PathCommandKind::LINE,
            PathCommandKind::LINE,
            PathCommandKind::LINE,
            PathCommandKind::CUBIC,
            PathCommandKind::CLOSE
        ]
    );
    assert_eq!(commands[1].p1, Point::new(4.0, 6.0));
    // A smooth cubic after a line uses the current point as its control.
    assert_eq!(commands[4].p1, Point::new(5.0, 5.0));
    assert_eq!(
        parse_path("t", "M,1 2", limits()).unwrap_err(),
        "d2svgimport: t path byte 1: comma is not allowed before a command's first argument"
    );
    assert_eq!(
        parse_path("", "M0 0 L1", limits()).unwrap_err(),
        "d2svgimport: SVG asset path byte 7: expected a finite number"
    );
}

#[test]
fn gradients_become_typed_paints() {
    let mut a = rectangle("a", 0, "");
    a["fill"] = "linear-gradient(to right, red 0%, #0000ff 100%)".into();
    let d = diagram(serde_json::json!({"name": "", "root": {}, "shapes": [a]}));
    let document = build(&d, &probe_options()).unwrap();
    let outline = &document.root.as_ref().unwrap().children()[1].children()[0];
    let Some(Primitive::Rect(rect)) = &outline.primitive else {
        panic!("rect")
    };
    let Some(d2_rs_scene::Paint::LinearGradient(gradient)) = &rect.fill else {
        panic!("gradient")
    };
    assert_eq!(
        (gradient.start, gradient.end),
        (Point::new(0.0, 0.5), Point::new(1.0, 0.5))
    );
    assert_eq!(
        gradient.stops[1].color,
        d2_rs_scene::Nrgba::new(0, 0, 255, 255)
    );
}

#[test]
fn latex_admission_matches_go() {
    let mut a = rectangle("a", 0, "\\frac{1}{2}");
    a["language"] = "latex".into();
    let d = diagram(serde_json::json!({"name": "", "root": {}, "shapes": [a]}));
    assert_eq!(
        build(&d, &probe_options()).unwrap_err().to_string(),
        "scene: shape \"a\" uses unsupported label language latex without configured SVG import limits"
    );
    let deep = "{".repeat(129);
    assert_eq!(
        crate::latex::validate_latex_input("shape \"a\"", &deep)
            .unwrap_err()
            .to_string(),
        "scene: shape \"a\" latex group nesting depth 129 exceeds limit 128"
    );
}

#[test]
fn sketch_ordinary_shapes_keep_typed_paths_and_split_streaks() {
    use d2_rs_scene::{FillRule, Paint, PathCommandKind};
    // d2scenebuild/sketch_shapes.go:115–200,862–918: two rough sets,
    // followed by a streak overlay with 85 subpaths in 84 local primitives.
    let d = diagram(serde_json::json!({
        "root": {}, "shapes": [rectangle("a", 0, "")]
    }));
    let options = Options {
        sketch: true,
        sketch_budget: crate::SketchBudget {
            max_operation_sets: 2,
            max_operations: 1000,
            max_path_commands: 1000,
        },
        ..probe_options()
    };
    let document = build(&d, &options).unwrap();
    let shape = &document.root.as_ref().unwrap().children()[1];
    assert_eq!(shape.children().len(), 3);
    assert_eq!(shape.children()[0].id, "a:sketch:set:0");
    assert_eq!(shape.children()[1].id, "a:sketch:set:1");
    let Some(Primitive::Rect(overlay)) = &shape.children()[2].primitive else {
        panic!("streak rectangle")
    };
    let Some(Paint::PatternPaint(pattern)) = &overlay.fill else {
        panic!("streak paint")
    };
    let streaks = pattern.root.as_ref().unwrap().children();
    assert_eq!(streaks.len(), 84);
    let commands: Vec<_> = streaks
        .iter()
        .flat_map(|n| {
            let Some(Primitive::Path(p)) = &n.primitive else {
                panic!("streak path")
            };
            assert_eq!(p.fill_rule, FillRule::EVEN_ODD);
            assert_eq!(p.commands.last().unwrap().kind, PathCommandKind::CLOSE);
            p.commands.iter()
        })
        .collect();
    assert_eq!(
        commands
            .iter()
            .filter(|c| c.kind == PathCommandKind::MOVE)
            .count(),
        85
    );
    let Some(Primitive::Path(compound)) = &streaks[16].primitive else {
        panic!("compound path")
    };
    assert_eq!(
        compound
            .commands
            .iter()
            .filter(|c| c.kind == PathCommandKind::MOVE)
            .count(),
        2
    );
    assert_eq!(commands.len(), 687);

    // sketch_shapes.go:464–560: 3D faces stay precise; multiple uses a
    // precise duplicate; double borders use two rough outlines and one streak.
    let mut options = options.clone();
    options.sketch_budget.max_operation_sets = 4;
    for (effect, ids) in [
        (
            serde_json::json!({"3d": true}),
            vec!["a:3d-main", "a:3d-sides", "a:3d-border"],
        ),
        (
            serde_json::json!({"multiple": true}),
            vec![
                "a:multiple",
                "a:main:sketch:set:0",
                "a:main:sketch:set:1",
                "a:main:sketch:streak:0",
            ],
        ),
        (
            serde_json::json!({"double-border": true}),
            vec![
                "a:double-border:outer:sketch:set:0",
                "a:double-border:outer:sketch:set:1",
                "a:double-border:inner:sketch:set:0",
                "a:double-border:inner:sketch:set:1",
                "a:double-border:streak:0",
            ],
        ),
    ] {
        let mut value = rectangle("a", 0, "");
        value
            .as_object_mut()
            .unwrap()
            .extend(effect.as_object().unwrap().clone());
        let d = diagram(serde_json::json!({"root": {}, "shapes": [value]}));
        let document = build(&d, &options).unwrap();
        let shape = &document.root.as_ref().unwrap().children()[1];
        assert_eq!(
            shape
                .children()
                .iter()
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            ids
        );
    }

    // sketch_shapes.go:592–733: structured headers and separators use rough
    // geometry; class streaks cover the header, table streaks the whole box.
    options.sketch_budget.max_operation_sets = 5;
    for (kind, ids) in [
        (
            "class",
            vec![
                "a:outline:sketch:set:0",
                "a:outline:sketch:set:1",
                "a:class-header:sketch:set:0",
                "a:class-header:sketch:set:1",
                "a:class-header:streak:0",
                "a:class-separator:sketch:set:0",
            ],
        ),
        (
            "sql_table",
            vec![
                "a:outline:sketch:set:0",
                "a:outline:sketch:set:1",
                "a:table-header:sketch:set:0",
                "a:table-header:sketch:set:1",
                "a:table:streak:0",
            ],
        ),
    ] {
        let mut value = rectangle("a", 0, "");
        value["type"] = kind.into();
        let d = diagram(serde_json::json!({"root": {}, "shapes": [value]}));
        let document = build(&d, &options).unwrap();
        let children = document.root.as_ref().unwrap().children()[1].children();
        assert_eq!(
            children.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            ids
        );
        if kind == "class" {
            let Some(Primitive::Path(separator)) = &children.last().unwrap().primitive else {
                panic!("rough separator")
            };
            assert!(separator.fill.is_some());
            assert!(separator.stroke.is_none());
        }
    }

    // sketch_connection.go: preserve the route ID and mask, roughen every
    // arrowhead at both endpoints, and retain precise animated split tracks.
    options.sketch_budget = crate::SketchBudget {
        max_operation_sets: 1000,
        max_operations: 10000,
        max_path_commands: 10000,
    };
    let arrowheads = [
        "arrow",
        "triangle",
        "unfilled-triangle",
        "line",
        "filled-diamond",
        "diamond",
        "filled-circle",
        "circle",
        "cross",
        "filled-box",
        "box",
        "cf-one",
        "cf-many",
        "cf-one-required",
        "cf-many-required",
    ];
    for arrowhead in arrowheads {
        for animated in [false, true] {
            let d = diagram(serde_json::json!({
                "root": {}, "shapes": [rectangle("a", 0, ""), rectangle("b", 200, "")],
                "connections": [{"id":"edge", "src":"a", "dst":"b",
                    "srcArrow":arrowhead,"dstArrow":arrowhead,"stroke":"N1",
                    "strokeWidth":2,"opacity":1,"animated":animated,
                    "label":"edge","fontSize":16,"fontFamily":"DEFAULT","color":"N1",
                    "labelWidth":32,"labelHeight":18,"labelPosition":"INSIDE_MIDDLE_CENTER",
                    "route":[{"x":100,"y":33},{"x":200,"y":33}]}]
            }));
            let document = build(&d, &options).unwrap();
            let group = document
                .root
                .as_ref()
                .unwrap()
                .children()
                .iter()
                .find(|n| n.id == "edge")
                .unwrap();
            let geometry = &group.children()[0];
            assert!(geometry.mask.is_some());
            let ids: Vec<_> = geometry.children().iter().map(|n| n.id.as_str()).collect();
            if animated {
                assert_eq!(
                    ids,
                    [
                        "edge:path:reverse",
                        "edge:src-arrowhead",
                        "edge:path:forward",
                        "edge:dst-arrowhead"
                    ]
                );
                let precise = build(
                    &d,
                    &Options {
                        sketch: false,
                        ..options.clone()
                    },
                )
                .unwrap();
                let precise_group = precise
                    .root
                    .as_ref()
                    .unwrap()
                    .children()
                    .iter()
                    .find(|n| n.id == "edge")
                    .unwrap();
                for index in [0, 2] {
                    let actual = &geometry.children()[index];
                    let expected = &precise_group.children()[0].children()[index];
                    assert_eq!(actual.primitive, expected.primitive);
                    assert_eq!(actual.animations, expected.animations);
                }
            } else {
                assert_eq!(
                    ids,
                    ["edge:path", "edge:src-arrowhead", "edge:dst-arrowhead"]
                );
                let Some(Primitive::Path(path)) = &geometry.children()[0].primitive else {
                    panic!("rough route")
                };
                assert_eq!(path.commands.len(), 4);
            }
            for arrow in geometry
                .children()
                .iter()
                .filter(|n| n.id.ends_with("arrowhead"))
            {
                assert!(matches!(arrow.primitive, Some(Primitive::Path(_))));
            }
        }
    }
}

#[test]
fn sketch_admission_accounts_for_rough_sets_and_shared_streak_commands() {
    let mut shapes = vec![rectangle("a", 0, ""), rectangle("b", 200, "")];
    for s in &mut shapes {
        s["fill"] = "#ffffff".into();
    }
    let d = diagram(serde_json::json!({"root": {}, "shapes": shapes}));
    let mut options = Options {
        sketch: true,
        sketch_budget: crate::SketchBudget {
            max_operation_sets: 4,
            max_operations: 1000,
            max_path_commands: 727,
        },
        ..probe_options()
    };
    // Each rough rectangle retains 20 commands (4 fill + 16 outline).
    // The 687-command same-category streak pattern is charged once.
    let document = build(&d, &options).unwrap();
    let shapes = &document.root.as_ref().unwrap().children()[1..];
    let roots: Vec<_> = shapes
        .iter()
        .map(|shape| {
            let Some(Primitive::Rect(overlay)) = &shape.children().last().unwrap().primitive else {
                panic!("streak rectangle")
            };
            let Some(d2_rs_scene::Paint::PatternPaint(pattern)) = &overlay.fill else {
                panic!("streak paint")
            };
            pattern.root.as_ref().unwrap()
        })
        .collect();
    assert!(std::sync::Arc::ptr_eq(roots[0], roots[1]));
    // Pinned d2raster.Render on these two rectangles reports this work with
    // MaxEvenOddClipWork=1. Deep-copying the streak root charges it twice.
    let mut frame = d2_rs_raster::FrameOptions {
        scale: 1.0,
        max_width: 32_768,
        max_height: 32_768,
        max_pixels: 64 * 1024 * 1024,
        max_nodes: 1_000_000,
        max_depth: 1_024,
        max_path_commands: 10_000_000,
        max_text_runes_per_run: 100_000,
        max_font_faces_per_text: 64,
        max_text_coverage_checks: 10_000_000,
        max_text_shaping_runs: 100_000,
        max_animation_tracks: 1_000_000,
        max_animation_keyframes: 10_000_000,
        max_assets: 4_096,
        max_asset_bytes: 512 * 1024 * 1024,
        max_decoded_asset_bytes: 512 * 1024 * 1024,
        max_import_depth: 1_024,
        max_offscreen_bytes: 512 * 1024 * 1024,
        max_even_odd_clip_work: 1,
        max_scanline_work: 4_000_000_000,
        ..Default::default()
    };
    assert_eq!(
        d2_rs_raster::preflight(&document, &frame)
            .unwrap_err()
            .to_string(),
        "d2raster: even-odd clip work 3354352 exceeds limit 1"
    );
    frame.max_even_odd_clip_work = 3_354_352;
    d2_rs_raster::preflight(&document, &frame).unwrap();
    options.sketch_budget.max_path_commands -= 1;
    assert_eq!(
        build(&d, &options).unwrap_err().to_string(),
        "scene: shape \"b\" sketch path command count exceeds limit 726"
    );
    options.sketch_budget.max_path_commands = 1000;
    options.sketch_budget.max_operation_sets = 3;
    assert_eq!(
        build(&d, &options).unwrap_err().to_string(),
        "scene: shape \"b\" sketch geometry: d2sketch: rough operation set count 2 exceeds limit 1"
    );
    options.sketch_budget.max_operation_sets = 4;
    build(&d, &options).unwrap();
}
