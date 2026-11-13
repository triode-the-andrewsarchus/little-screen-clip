use structurizr_dsl::parse;
use structurizr_render::SvgRenderer;

#[test]
fn orthogonal_arrows_attach_to_facing_boundaries_in_every_layout_direction() {
    for (direction, source_position, target_position, start, end) in [
        ("lr", (0, 0), (600, 0), (400.0, 125.0), (600.0, 125.0)),
        ("rl", (600, 0), (0, 0), (600.0, 125.0), (400.0, 125.0)),
        ("tb", (0, 0), (0, 500), (200.0, 250.0), (200.0, 500.0)),
        ("bt", (0, 500), (0, 0), (200.0, 500.0), (200.0, 250.0)),
    ] {
        for custom in [false, true] {
            let element = if custom { "element" } else { "softwareSystem" };
            let kind = if custom { "custom" } else { "systemLandscape" };
            let source = format!(
                r#"workspace {{
                model {{ a = {element} "A" b = {element} "B" a -> b "Sends" }}
                views {{ {kind} "Flow" {{ include * autoLayout {direction} }} }}
            }}"#
            );
            let mut workspace = parse(&source).unwrap();
            let properties = if custom {
                &mut workspace.views_mut().custom_views[0].properties
            } else {
                &mut workspace.views_mut().system_landscape_views[0].properties
            };
            for (view, (x, y)) in properties
                .elements
                .iter_mut()
                .zip([source_position, target_position])
            {
                view.x = Some(x);
                view.y = Some(y);
            }
            let renderer = SvgRenderer::new(0, 0);
            let svg = if custom {
                renderer.render_custom(&workspace, &workspace.views().custom_views[0])
            } else {
                renderer.render_system_landscape(
                    &workspace,
                    &workspace.views().system_landscape_views[0],
                )
            }
            .unwrap();
            let edge = svg
                .lines()
                .find(|line| line.contains("data-routing=\"orthogonal\""))
                .unwrap();
            let path = edge
                .split("d=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap();
            let coordinates: Vec<f64> = path
                .split_whitespace()
                .filter_map(|word| word.parse().ok())
                .collect();
            let points: Vec<_> = coordinates
                .chunks_exact(2)
                .map(|pair| (pair[0], pair[1]))
                .collect();
            assert_eq!(points.first(), Some(&start), "{kind} {direction}: {path}");
            assert_eq!(points.last(), Some(&end), "{kind} {direction}: {path}");
            assert!(edge.contains("marker-end="));
            for segment in points.windows(2) {
                assert!(
                    segment[0].0 == segment[1].0 || segment[0].1 == segment[1].1,
                    "orthogonal segment: {path}"
                );
            }
            let before = points[points.len() - 2];
            let faces_target = match direction {
                "lr" => before.0 < end.0 && before.1 == end.1,
                "rl" => before.0 > end.0 && before.1 == end.1,
                "tb" => before.1 < end.1 && before.0 == end.0,
                "bt" => before.1 > end.1 && before.0 == end.0,
                _ => unreachable!(),
            };
            assert!(
                faces_target,
                "arrowhead must approach from outside: {kind} {direction}: {path}"
            );
        }
    }
}
