use super::*;

#[test]
fn test_markup_defaults() {
    let state = MarkupState::default();
    assert_eq!(state.active_tool, MarkupToolType::Arrow);
    assert_eq!(state.active_color, Color32::from_rgb(255, 42, 42));
    assert_eq!(state.active_stroke_width, 4.0);
    assert_eq!(state.active_font_size, 20.0);
    assert_eq!(state.next_badge_number, 1);
    assert!(state.items.is_empty());
}

#[test]
fn test_markup_shape_operations() {
    let mut shape = MarkupShape::Arrow {
        start: Pos2::new(0.1, 0.1),
        end: Pos2::new(0.5, 0.5),
        color: Color32::RED,
        width: 4.0,
    };

    assert_eq!(shape.type_name(), "Arrow");
    assert_eq!(shape.type_icon(), Icon::ArrowRight);
    assert_eq!(shape.color(), Color32::RED);
    assert_eq!(shape.width(), Some(4.0));

    shape.set_color(Color32::GREEN);
    assert_eq!(shape.color(), Color32::GREEN);

    shape.set_width(8.0);
    assert_eq!(shape.width(), Some(8.0));

    shape.translate(Vec2::new(0.1, 0.2));
    match shape {
        MarkupShape::Arrow { start, end, .. } => {
            assert!((start.x - 0.2).abs() < 1e-5);
            assert!((start.y - 0.3).abs() < 1e-5);
            assert!((end.x - 0.6).abs() < 1e-5);
            assert!((end.y - 0.7).abs() < 1e-5);
        }
        _ => panic!("Expected arrow"),
    }
}

#[test]
fn test_markup_badge_auto_increment() {
    let mut state = MarkupState::default();
    assert_eq!(state.next_badge_number, 1);
    assert_eq!(state.next_available_badge_number(), 1);

    // Create badges 1, 2, 3, 4, 5
    for _ in 1..=5 {
        let num = state.next_available_badge_number();
        state.items.push(MarkupShape::StepBadge {
            center: Pos2::new(0.5, 0.5),
            radius: 28.0,
            number: num,
            color: state.active_color,
        });
        state.sync_next_badge_number();
    }
    assert_eq!(state.items.len(), 5);
    assert_eq!(state.next_badge_number, 6);

    // Delete 2 and 5 (leaving 1, 3, 4)
    state.items.retain(|item| {
        if let MarkupShape::StepBadge { number, .. } = item {
            *number != 2 && *number != 5
        } else {
            true
        }
    });
    state.sync_next_badge_number();
    assert_eq!(state.items.len(), 3);

    // Next badge created must fill hole 2
    assert_eq!(state.next_available_badge_number(), 2);
    assert_eq!(state.next_badge_number, 2);
    let num2 = state.next_available_badge_number();
    state.items.push(MarkupShape::StepBadge {
        center: Pos2::new(0.5, 0.5),
        radius: 28.0,
        number: num2,
        color: state.active_color,
    });
    state.sync_next_badge_number();
    assert_eq!(num2, 2);

    // Next badge created must fill hole 5
    assert_eq!(state.next_available_badge_number(), 5);
    assert_eq!(state.next_badge_number, 5);
    let num5 = state.next_available_badge_number();
    state.items.push(MarkupShape::StepBadge {
        center: Pos2::new(0.5, 0.5),
        radius: 28.0,
        number: num5,
        color: state.active_color,
    });
    state.sync_next_badge_number();
    assert_eq!(num5, 5);

    // Next badge must move onto 6
    assert_eq!(state.next_available_badge_number(), 6);
    assert_eq!(state.next_badge_number, 6);
    let num6 = state.next_available_badge_number();
    state.items.push(MarkupShape::StepBadge {
        center: Pos2::new(0.5, 0.5),
        radius: 28.0,
        number: num6,
        color: state.active_color,
    });
    state.sync_next_badge_number();
    assert_eq!(num6, 6);

    // Next badge is 7
    assert_eq!(state.next_available_badge_number(), 7);
}

#[test]
fn test_markup_rasterize_to_image() {
    let base = RgbaImage::new(100, 100);
    let items = vec![
        MarkupShape::Arrow {
            start: Pos2::new(0.1, 0.1),
            end: Pos2::new(0.8, 0.8),
            color: Color32::RED,
            width: 4.0,
        },
        MarkupShape::Rectangle {
            rect: Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.6, 0.6)),
            color: Color32::YELLOW,
            width: 2.0,
            filled: true,
            rounding: 0.0,
        },
        MarkupShape::Text {
            pos: Pos2::new(0.1, 0.5),
            text: "Hello".to_string(),
            font_size: 16.0,
        },
    ];

    let out = rasterize_markup(&base, &items);
    assert_eq!(out.width(), 100);
    assert_eq!(out.height(), 100);
}

#[test]
fn test_infer_font_size_from_box_height() {
    // A box height of 31px should infer ~20px font
    let f20 = infer_font_size_from_box_height(31.0);
    assert!((f20 - 20.0).abs() < 1.0);

    // A box height of 62px should infer ~40px font
    let f40 = infer_font_size_from_box_height(62.0);
    assert!((f40 - 40.0).abs() < 1.0);

    // Clamping ensures reasonable bounds
    assert_eq!(infer_font_size_from_box_height(5.0), 10.0);
    assert_eq!(infer_font_size_from_box_height(1000.0), 300.0);
}

#[test]
fn test_markup_text_bounding_rect_expands_with_typing() {
    let short_text = MarkupShape::Text {
        pos: Pos2::new(0.1, 0.1),
        text: "Hi".to_string(),
        font_size: 20.0,
    };
    let long_text = MarkupShape::Text {
        pos: Pos2::new(0.1, 0.1),
        text: "This is a significantly longer annotation string".to_string(),
        font_size: 20.0,
    };

    let rect_short = short_text.bounding_rect_norm(1000.0, 1000.0);
    let rect_long = long_text.bounding_rect_norm(1000.0, 1000.0);

    // Heights should be identical because font_size is unchanged
    assert!((rect_short.height() - rect_long.height()).abs() < 1e-4);
    // Width must grow as user types more characters
    assert!(rect_long.width() > rect_short.width());
}

#[test]
fn test_step_badge_font_size() {
    // Single digit gets 1.25x radius
    let font_1 = step_badge_font_size(24.0, 5);
    assert!((font_1 - 30.0).abs() < 1e-3);

    // Double digit gets 0.95x radius to fit cleanly inside circle
    let font_2 = step_badge_font_size(24.0, 42);
    assert!((font_2 - 22.8).abs() < 1e-3);

    // Triple digit gets 0.70x radius
    let font_3 = step_badge_font_size(24.0, 100);
    assert!((font_3 - 16.8).abs() < 1e-3);

    // Clamping ensures readable minimum
    assert_eq!(step_badge_font_size(4.0, 1), 8.0);
}

#[test]
fn test_step_badge_scale_high_res() {
    let badge = MarkupShape::StepBadge {
        center: Pos2::new(0.5, 0.5),
        radius: 80.0,
        number: 1,
        color: Color32::RED,
    };

    // Bounding rect on 4K image (3840x2160)
    let rect_4k = badge.bounding_rect_norm(3840.0, 2160.0);
    let screen_w = rect_4k.width() * 3840.0;
    let screen_h = rect_4k.height() * 2160.0;
    assert!((screen_w - 160.0).abs() < 1e-3);
    assert!((screen_h - 160.0).abs() < 1e-3);

    let font_size = step_badge_font_size(80.0, 1);
    assert_eq!(font_size, 100.0);
}

#[test]
fn burned_circle_fits_preview_bounds_on_non_square_images() {
    for (width, height) in [(120, 40), (40, 120)] {
        let canvas = RgbaImage::new(width, height);
        let result = rasterize_markup(
            &canvas,
            &[MarkupShape::Circle {
                rect: Rect::from_min_max(Pos2::new(0.25, 0.25), Pos2::new(0.75, 0.75)),
                color: Color32::RED,
                width: 1.0,
                filled: true,
            }],
        );
        let (cx, cy) = (width / 2, height / 2);
        assert_eq!(result.get_pixel(cx, cy)[3], 255);
        for (x, y) in [(cx - 8, cy), (cx, cy - 8)] {
            assert_eq!(result.get_pixel(x, y)[3], 255);
        }
        for (x, y) in [(cx - 11, cy), (cx, cy - 11)] {
            assert_eq!(result.get_pixel(x, y)[3], 0);
        }
    }
}

#[test]
fn burned_rectangles_preserve_rounded_corners_for_fill_and_outline() {
    let canvas = RgbaImage::new(40, 40);
    for filled in [true, false] {
        let rect = Rect::from_min_max(Pos2::new(0.25, 0.25), Pos2::new(0.75, 0.75));
        let render = |rounding| {
            rasterize_markup(
                &canvas,
                &[MarkupShape::Rectangle {
                    rect,
                    color: Color32::RED,
                    width: 2.0,
                    filled,
                    rounding,
                }],
            )
        };
        let rounded = render(8.0);
        let square = render(0.0);
        assert_eq!(rounded.get_pixel(10, 10)[3], 0);
        assert_eq!(square.get_pixel(10, 10)[3], 255);
        assert_eq!(rounded.get_pixel(20, 10)[3], 255);
        assert_eq!(rounded.get_pixel(20, 20)[3], if filled { 255 } else { 0 });
    }
}

#[test]
fn rectangle_preview_scales_image_space_rounding_with_zoom() {
    let mut state = crate::image_preview::tests::image_state_from_png((40, 40), "markup");
    state.markup_state.items.push(MarkupShape::Rectangle {
        rect: Rect::from_min_max(Pos2::new(0.25, 0.25), Pos2::new(0.75, 0.75)),
        color: Color32::RED,
        width: 1.0,
        filled: true,
        rounding: 8.0,
    });
    let ctx = egui::Context::default();
    for (viewport_size, expected_radius) in [(20.0, 4.0), (80.0, 16.0)] {
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_markup_passive(
                    ui.painter(),
                    Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(viewport_size)),
                );
            });
        });
        let rect = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.fill == Color32::RED => Some(rect),
                _ => None,
            })
            .expect("markup rectangle painted");
        assert_eq!(rect.rounding, Rounding::same(expected_radius));
    }
}
