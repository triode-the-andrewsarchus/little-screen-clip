use super::*;
use egui::{RawInput, Shape};

#[test]
fn test_average_color_computation_and_redaction() {
    // Create an image: left half is Red (200, 0, 0), right half is Blue (0, 0, 200)
    let mut img = RgbaImage::new(100, 100);
    for y in 0..100 {
        for x in 0..50 {
            img.put_pixel(x, y, image::Rgba([200, 0, 0, 255]));
        }
        for x in 50..100 {
            img.put_pixel(x, y, image::Rgba([0, 0, 200, 255]));
        }
    }

    // Box covering entire left half (x: 0..0.5, y: 0..1.0)
    let left_box = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.5, 1.0));
    let left_avg = compute_box_average_color(&img, left_box);
    assert_eq!(left_avg, Color32::from_rgb(200, 0, 0));

    let composed_left = compose_redactions_only(
        &img,
        &[RedactionBox {
            normalized_rect: left_box,
            style: RedactionStyle::AverageColor,
        }],
    );
    // Interior of left half should remain pure red (200, 0, 0)
    assert_eq!(
        *composed_left.get_pixel(25, 25),
        image::Rgba([200, 0, 0, 255])
    );

    // Black text on off-white background
    let mut doc_img = RgbaImage::from_pixel(100, 100, image::Rgba([245, 245, 240, 255]));
    // Add black text in the center (x: 30..70, y: 35..65)
    for y in 35..65 {
        for x in 30..70 {
            doc_img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
        }
    }
    // Redaction box enclosing the text with margin (x: 20..80, y: 25..75)
    let text_box = Rect::from_min_max(Pos2::new(0.2, 0.25), Pos2::new(0.8, 0.75));
    let edge_color = compute_box_average_color(&doc_img, text_box);
    assert_eq!(edge_color, Color32::from_rgb(245, 245, 240));

    let redacted_doc = compose_redactions_only(
        &doc_img,
        &[RedactionBox {
            normalized_rect: text_box,
            style: RedactionStyle::AverageColor,
        }],
    );

    // Center where text was should now match the off-white background exactly, not tainted by black text
    assert_eq!(
        *redacted_doc.get_pixel(50, 50),
        image::Rgba([245, 245, 240, 255])
    );
    assert_eq!(
        *redacted_doc.get_pixel(35, 45),
        image::Rgba([245, 245, 240, 255])
    );

    // Test gradient background redaction: top is dark gray (100, 100, 100), bottom is light gray (200, 200, 200)
    let mut grad_img = RgbaImage::new(100, 100);
    for y in 0..100 {
        let v = (100.0 + (y as f32 / 99.0) * 100.0).round() as u8;
        for x in 0..100 {
            grad_img.put_pixel(x, y, image::Rgba([v, v, v, 255]));
        }
    }
    // Black text in the center
    for y in 40..60 {
        for x in 30..70 {
            grad_img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
        }
    }
    let grad_box = Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.8, 0.8));
    let redacted_grad = compose_redactions_only(
        &grad_img,
        &[RedactionBox {
            normalized_rect: grad_box,
            style: RedactionStyle::AverageColor,
        }],
    );
    // Top edge of box (y = 20) matches background (~120)
    let top_px = redacted_grad.get_pixel(50, 20);
    assert!((top_px[0] as i16 - 120).abs() <= 2);
    // Bottom edge of box (y = 79) matches background (~180)
    let bot_px = redacted_grad.get_pixel(50, 79);
    assert!((bot_px[0] as i16 - 180).abs() <= 2);
    // Center (y = 50) where black text was is smoothly blurred to ~150 with zero black contamination
    let mid_px = redacted_grad.get_pixel(50, 50);
    assert!((mid_px[0] as i16 - 150).abs() <= 3);

    // Compose with Black style
    let composed_black = compose_redactions_only(
        &img,
        &[RedactionBox {
            normalized_rect: left_box,
            style: RedactionStyle::Black,
        }],
    );
    assert_eq!(
        *composed_black.get_pixel(25, 25),
        image::Rgba([0, 0, 0, 255])
    );
}

#[test]
fn test_mask_stamping_and_erasing() {
    let mut mask = image::GrayImage::new(100, 100);

    // Stamp a rectangle in the center (40..60, 40..60)
    let rect = Rect::from_min_max(Pos2::new(0.4, 0.4), Pos2::new(0.6, 0.6));
    stamp_rect_on_mask(&mut mask, rect, 255);
    assert_eq!(mask.get_pixel(50, 50)[0], 255);
    assert_eq!(mask.get_pixel(10, 10)[0], 0);

    // Erase a smaller circle in the center
    stamp_circle_on_mask(&mut mask, Pos2::new(0.5, 0.5), 5.0, 0);
    assert_eq!(mask.get_pixel(50, 50)[0], 0);
    assert_eq!(mask.get_pixel(42, 42)[0], 255);
}

#[test]
fn test_brush_size_adjustment() {
    let png = RgbaImage::new(10, 10);
    let mut cursor = std::io::Cursor::new(Vec::new());
    png.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
    let mut state = ImagePreviewState::from_bytes_with_meta(
        cursor.into_inner(),
        "image/png".into(),
        None,
        "Brush size test".into(),
        &little_clip_core::config::AppConfig::default(),
    )
    .unwrap();

    assert_eq!(state.redaction_state.brush_radius, 24.0);
    state.adjust_brush_radius(4.0);
    assert_eq!(state.redaction_state.brush_radius, 28.0);
    state.adjust_brush_radius(-8.0);
    assert_eq!(state.redaction_state.brush_radius, 20.0);

    // Check clamping
    state.adjust_brush_radius(500.0);
    assert_eq!(state.redaction_state.brush_radius, 128.0);
    state.adjust_brush_radius(-500.0);
    assert_eq!(state.redaction_state.brush_radius, 4.0);
}

#[test]
fn selected_redaction_size_badge_stays_inside_compact_and_wide_viewports() {
    for viewport_size in [Vec2::new(280.0, 220.0), Vec2::new(1040.0, 640.0)] {
        let mut png = Vec::new();
        RgbaImage::new(320, 240)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            png,
            "image/png".into(),
            None,
            "Redact badge test".into(),
            &little_clip_core::config::AppConfig::default(),
        )
        .unwrap();
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.8, 0.8)),
            style: RedactionStyle::Black,
        });
        state.redaction_state.selected_idx = Some(0);

        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        let mut viewport = None;
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, viewport_size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let image_rect = ui.max_rect();
                    let response = ui.allocate_rect(image_rect, egui::Sense::click_and_drag());
                    state.render_redact_overlay(
                        ui,
                        &ui.painter().clone(),
                        &response,
                        image_rect,
                        &palette,
                    );
                    viewport = Some(image_rect);
                });
            },
        );

        let viewport = viewport.expect("overlay viewport was allocated");
        let badge = output.shapes.iter().find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text().starts_with("192 × 144 px") => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        });
        let badge = badge.expect("selected redaction renders its size badge");
        assert!(
            viewport.contains_rect(badge),
            "badge {badge:?} must stay within {viewport:?} at {viewport_size:?}"
        );
    }
}

#[test]
fn test_native_lama_inpaint_execution() {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let model_path =
        std::path::PathBuf::from(home).join(".cache/little-screen-clip/models/big-lama.onnx");
    if !model_path.exists() {
        return;
    }

    // 64x64 test image: solid green (40, 200, 40) with a blue patch in the center
    let mut img = RgbaImage::from_pixel(64, 64, image::Rgba([40, 200, 40, 255]));
    for y in 24..40 {
        for x in 24..40 {
            img.put_pixel(x, y, image::Rgba([40, 40, 200, 255]));
        }
    }

    // Mask covering the blue patch
    let mut mask = image::GrayImage::new(64, 64);
    for y in 20..44 {
        for x in 20..44 {
            mask.put_pixel(x, y, image::Luma([255]));
        }
    }

    let result = run_native_lama_inpaint(&img, &mask, &model_path).unwrap();
    assert_eq!(result.dimensions(), (64, 64));

    // Outside the mask, pixels must be exactly identical to original
    assert_eq!(*result.get_pixel(10, 10), image::Rgba([40, 200, 40, 255]));
    // Inside the mask, the blue patch should have been replaced (no longer [40, 40, 200])
    assert_ne!(*result.get_pixel(32, 32), image::Rgba([40, 40, 200, 255]));
}

#[test]
fn test_inpaint_selection_modes_combine_onto_same_mask() {
    let mut mask = image::GrayImage::new(100, 100);

    // 1. Block Select: stamp a rectangle from norm (0.1, 0.1) to (0.3, 0.3)
    let rect = Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.3, 0.3));
    stamp_rect_on_mask(&mut mask, rect, 255);

    // Check rectangle was stamped (e.g. at pixel 20, 20)
    assert_eq!(mask.get_pixel(20, 20)[0], 255);
    assert_eq!(mask.get_pixel(50, 50)[0], 0);

    // 2. Paint Brush: stamp a circle at norm (0.5, 0.5) with radius 10px
    stamp_circle_on_mask(&mut mask, Pos2::new(0.5, 0.5), 10.0, 255);
    assert_eq!(mask.get_pixel(50, 50)[0], 255);

    // 3. Point-to-Select (SAM): stamp a segmented object at (70..90, 70..90)
    let mut sam_mask = image::GrayImage::new(100, 100);
    for y in 70..90 {
        for x in 70..90 {
            sam_mask.put_pixel(x, y, image::Luma([255]));
        }
    }
    crate::image_preview::tools::sam::stamp_sam_mask_on_inpaint_mask(&mut mask, &sam_mask, 255);
    assert_eq!(mask.get_pixel(80, 80)[0], 255);

    // All three modes are present simultaneously on the exact same mask!
    assert_eq!(mask.get_pixel(20, 20)[0], 255); // Block Select
    assert_eq!(mask.get_pixel(50, 50)[0], 255); // Paint Brush
    assert_eq!(mask.get_pixel(80, 80)[0], 255); // SAM Object
    assert_eq!(mask.get_pixel(5, 5)[0], 0); // Background

    // 4. SAM Erase: erase a portion of the block select rectangle (15..25, 15..25)
    let mut erase_sam = image::GrayImage::new(100, 100);
    for y in 15..25 {
        for x in 15..25 {
            erase_sam.put_pixel(x, y, image::Luma([255]));
        }
    }
    crate::image_preview::tools::sam::stamp_sam_mask_on_inpaint_mask(&mut mask, &erase_sam, 0);

    // Erased portion is now 0
    assert_eq!(mask.get_pixel(20, 20)[0], 0);
    // Rest of rectangle remains 255
    assert_eq!(mask.get_pixel(12, 12)[0], 255);
    // Paint brush and SAM object remain untouched!
    assert_eq!(mask.get_pixel(50, 50)[0], 255);
    assert_eq!(mask.get_pixel(80, 80)[0], 255);
}

#[test]
fn test_point_sam_mode_hover_and_commit() {
    let app_config = little_clip_core::AppConfig::default();
    let mut png = Vec::new();
    RgbaImage::from_pixel(100, 100, image::Rgba([120, 120, 120, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut state = ImagePreviewState::from_bytes_with_meta(
        png,
        "image/png".into(),
        None,
        "SAM test".into(),
        &app_config,
    )
    .unwrap();

    state.redaction_state.tool_type = RedactToolType::LamaInpaint;
    state.set_inpaint_mode(InpaintSelectionMode::PointSam);

    assert_eq!(
        state.redaction_state.inpaint_mode,
        InpaintSelectionMode::PointSam
    );

    // Simulate a SAM mask arriving from worker
    let mut sam_mask = image::GrayImage::new(100, 100);
    for y in 30..60 {
        for x in 30..60 {
            sam_mask.put_pixel(x, y, image::Luma([255]));
        }
    }
    state.sam_state.hovered_mask = Some(sam_mask);
    state.sam_state.hovered_pos = Some(Pos2::new(0.45, 0.45));

    let ctx = egui::Context::default();
    // Commit the hover mask into inpaint_mask
    state.commit_sam_hover(255, &ctx);

    let mask = state.ensure_inpaint_mask();
    assert_eq!(mask.get_pixel(45, 45)[0], 255);
    assert_eq!(mask.get_pixel(10, 10)[0], 0);
    assert!(state.inpaint_has_mask());
}

#[test]
fn test_grow_selection_expands_sam_and_inpaint_mask() {
    let app_config = little_clip_core::AppConfig::default();
    let mut png = Vec::new();
    RgbaImage::from_pixel(100, 100, image::Rgba([120, 120, 120, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut state = ImagePreviewState::from_bytes_with_meta(
        png,
        "image/png".into(),
        None,
        "Grow test".into(),
        &app_config,
    )
    .unwrap();

    state.redaction_state.tool_type = RedactToolType::LamaInpaint;
    state.redaction_state.grow_selection_px = 5.0;

    // 1. Check raw hover mask dilation via refresh_sam_hover_preview
    let mut raw_mask = image::GrayImage::new(100, 100);
    // Small 10x10 patch at (45..55, 45..55)
    for y in 45..55 {
        for x in 45..55 {
            raw_mask.put_pixel(x, y, image::Luma([255]));
        }
    }
    state.sam_state.raw_hovered_mask = Some(raw_mask);
    let ctx = egui::Context::default();
    state.refresh_sam_hover_preview(&ctx);

    let dilated_hover = state.sam_state.hovered_mask.as_ref().unwrap();
    // Point (40, 50) is 5 pixels away from x=45, should now be included
    assert_eq!(dilated_hover.get_pixel(40, 50)[0], 255);
    // Point (35, 50) is 10 pixels away, should be 0
    assert_eq!(dilated_hover.get_pixel(35, 50)[0], 0);

    // 2. Check growing an existing inpaint mask
    let mask = state.ensure_inpaint_mask();
    mask.put_pixel(50, 50, image::Luma([255]));
    state.grow_current_inpaint_mask(&ctx);

    let grown_mask = state.ensure_inpaint_mask();
    // Center is still 255
    assert_eq!(grown_mask.get_pixel(50, 50)[0], 255);
    // 5 pixels away is now 255
    assert_eq!(grown_mask.get_pixel(50, 45)[0], 255);
    assert_eq!(grown_mask.get_pixel(55, 50)[0], 255);
    // 10 pixels away is 0
    assert_eq!(grown_mask.get_pixel(50, 40)[0], 0);
}
