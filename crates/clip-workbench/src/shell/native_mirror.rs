//! Opt-in native painting of already prepared clipboard cards.
//! Content reads and classification remain in the existing mirror worker.
use crate::hex_to_c32;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};
use little_clip_core::{ClipPayload, SlotId, SlotManager, palette::AppPalette};
use little_clip_render::PreparedTextPreview;

pub(crate) struct NativeMirrorData {
    cursor: SlotId,
    pub(super) is_dark: bool,
    cards: Vec<(SlotId, NativeCard)>,
}

struct NativeCard {
    title: String,
    subtitle: String,
    body: NativeContent,
}

enum NativeContent {
    Text(PreparedTextPreview),
    Url(String),
    Color { hex: String, rgb: [u8; 3] },
    Audio(Option<little_clip_core::audio::AudioWaveform>),
    Raster,
}

impl NativeMirrorData {
    #[cfg(test)]
    pub(crate) fn is_empty_for_test(&self) -> bool {
        self.cards.is_empty()
    }

    /// Called only by the mirror worker; retains bounded display data, not source payloads.
    pub(crate) fn prepare(
        state: &SlotManager,
        mut text: Vec<(SlotId, PreparedTextPreview)>,
        is_dark: bool,
    ) -> Self {
        let cards = SlotId::all()
            .into_iter()
            .filter_map(|slot| {
                let item = state.get(slot)?;
                let body = match &item.payload {
                    ClipPayload::Text(_) => text
                        .iter()
                        .position(|(id, _)| *id == slot)
                        .map(|index| text.swap_remove(index).1)
                        .filter(|preview| {
                            preview.lines.len() <= 32
                                && preview.lines.iter().all(|line| {
                                    line.len() <= 128
                                        && line.iter().map(|span| span.text.len()).sum::<usize>()
                                            <= 2048
                                })
                        })
                        .map(NativeContent::Text)
                        .unwrap_or(NativeContent::Raster),
                    ClipPayload::Url(url) => {
                        NativeContent::Url(url.as_str().chars().take(256).collect())
                    }
                    ClipPayload::Color { hex, r, g, b } => NativeContent::Color {
                        hex: hex.chars().take(256).collect(),
                        rgb: [*r, *g, *b],
                    },
                    ClipPayload::AudioRef { waveform, .. } => match waveform {
                        Some(waveform)
                            if waveform.ch1_peaks.len() <= 8192
                                && waveform
                                    .ch2_peaks
                                    .as_ref()
                                    .is_none_or(|peaks| peaks.len() <= 8192)
                                && waveform
                                    .spectral_colors
                                    .as_ref()
                                    .is_none_or(|colors| colors.len() <= 8192) =>
                        {
                            NativeContent::Audio(Some(waveform.clone()))
                        }
                        None => NativeContent::Audio(None),
                        _ => NativeContent::Raster,
                    },
                    _ => NativeContent::Raster,
                };
                Some((
                    slot,
                    NativeCard {
                        title: item.title.chars().take(256).collect(),
                        subtitle: item.subtitle.chars().take(256).collect(),
                        body,
                    },
                ))
            })
            .collect();
        Self {
            cursor: state.cursor,
            is_dark,
            cards,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MirrorPresentation {
    Native,
    DisplayReference,
}

pub(super) fn paint(
    ui: &egui::Ui,
    palette: &AppPalette,
    rect: Rect,
    data: &NativeMirrorData,
    texture: Option<&egui::TextureHandle>,
    left_priors: usize,
) {
    let scale = rect.width() / super::screen_mirror::SCREEN_WIDTH;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, hex_to_c32(&palette.colors.canvas));
    for (slot, native) in super::screen_mirror::slot_rects(left_priors) {
        let target = Rect::from_min_size(
            rect.min + native.min.to_vec2() * scale,
            native.size() * scale,
        );
        let item = data
            .cards
            .iter()
            .find(|(id, _)| *id == slot)
            .map(|(_, card)| card);
        if item.is_some_and(|card| matches!(card.body, NativeContent::Raster)) {
            if let Some(texture) = texture {
                let uv = Rect::from_min_max(
                    Pos2::new(native.min.x / super::screen_mirror::SCREEN_WIDTH, 0.0),
                    Pos2::new(native.max.x / super::screen_mirror::SCREEN_WIDTH, 1.0),
                );
                painter.image(texture.id(), target, uv, Color32::WHITE);
            }
            continue;
        }
        let card = target.shrink(4.0 * scale);
        let ink = painter.with_clip_rect(card.shrink(5.0 * scale));
        let accent = hex_to_c32(palette.slot_border_hex(slot));
        let surface = match slot {
            SlotId::Undo => &palette.colors.surface_recessed,
            SlotId::LeftHand | SlotId::RightHand => &palette.colors.surface_raised,
            _ => &palette.colors.surface_panel,
        };
        painter.rect_filled(card, 0.0, hex_to_c32(surface));
        painter.rect_stroke(
            card,
            0.0,
            Stroke::new(
                scale.max(1.0),
                if data.cursor == slot {
                    hex_to_c32(&palette.cursor_border)
                } else {
                    accent
                },
            ),
        );
        let hero = matches!(slot, SlotId::LeftHand | SlotId::RightHand);
        let x = card.min.x + 14.0 * scale;
        label(
            &ink,
            Pos2::new(x, card.min.y + 14.0 * scale),
            slot.label(),
            if hero { 26.0 } else { 21.0 } * scale,
            accent,
        );
        let Some(item) = item else {
            label(
                &ink,
                Pos2::new(x, card.min.y + 54.0 * scale),
                "EMPTY",
                21.0 * scale,
                hex_to_c32(&palette.colors.text_muted),
            );
            continue;
        };
        label(
            &ink,
            Pos2::new(x, card.min.y + 52.0 * scale),
            &item.title,
            if hero { 28.0 } else { 21.0 } * scale,
            hex_to_c32(&palette.colors.text_primary),
        );
        label(
            &ink,
            Pos2::new(x, card.min.y + 88.0 * scale),
            &item.subtitle,
            if hero { 19.0 } else { 16.0 } * scale,
            hex_to_c32(&palette.colors.text_secondary),
        );
        let body = Rect::from_min_max(
            Pos2::new(x, card.min.y + 124.0 * scale),
            card.max - Vec2::splat(14.0 * scale),
        );
        let ink = ink.with_clip_rect(body);
        paint_content(
            &ink,
            body,
            &item.body,
            scale,
            ui.ctx().pixels_per_point(),
            palette,
        );
    }
}

fn paint_content(
    painter: &egui::Painter,
    body: Rect,
    content: &NativeContent,
    scale: f32,
    pixels_per_point: f32,
    palette: &AppPalette,
) {
    match content {
        NativeContent::Text(text) => {
            paint_text(
                painter,
                body,
                text,
                scale,
                hex_to_c32(&palette.colors.text_muted),
            );
        }
        NativeContent::Url(url) => label(
            painter,
            body.min,
            url,
            22.0 * scale,
            hex_to_c32(&palette.url_color),
        ),
        NativeContent::Color { hex, rgb } => {
            let swatch = Rect::from_min_size(body.min, Vec2::new(body.width(), 160.0 * scale));
            painter.rect_filled(swatch, 0.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            label(
                painter,
                swatch.left_bottom() + Vec2::new(0.0, 18.0 * scale),
                hex,
                32.0 * scale,
                hex_to_c32(&palette.colors.text_primary),
            );
        }
        NativeContent::Audio(waveform) => {
            if let Some(waveform) = waveform {
                let mut wave_rect = body;
                if waveform.is_snippet {
                    wave_rect.max.y -= 24.0 * scale;
                    let end_ms = waveform
                        .snippet_start_ms
                        .saturating_add(waveform.snippet_duration_ms);
                    label(
                        painter,
                        Pos2::new(body.min.x, wave_rect.max.y + 4.0 * scale),
                        &format!(
                            "Excerpt {:.1}–{:.1} s",
                            waveform.snippet_start_ms as f64 / 1000.0,
                            end_ms as f64 / 1000.0
                        ),
                        16.0 * scale,
                        hex_to_c32(&palette.colors.text_secondary),
                    );
                }
                paint_waveform(painter, wave_rect, waveform, pixels_per_point);
            } else {
                label(
                    painter,
                    body.min,
                    "No prepared waveform",
                    20.0 * scale,
                    hex_to_c32(&palette.colors.text_muted),
                );
            }
        }
        _ => {}
    }
}

fn label(painter: &egui::Painter, position: Pos2, text: &str, size: f32, color: Color32) {
    let text: String = text.chars().take(256).collect();
    painter.text(
        position,
        Align2::LEFT_TOP,
        text,
        FontId::proportional(size.max(8.0)),
        color,
    );
}

fn paint_text(
    painter: &egui::Painter,
    rect: Rect,
    text: &PreparedTextPreview,
    scale: f32,
    muted: Color32,
) {
    let font = FontId::monospace((18.0 * scale).max(9.0));
    let mut y = rect.min.y;
    let line_height = (24.0 * scale).max(font.size + 2.0);
    let rows = ((rect.height() - font.size) / line_height + 1.0).max(0.0) as usize;
    let needs_footer = text.lines.len() + usize::from(text.more_lines > 0) > rows;
    let visible_rows = rows.saturating_sub(usize::from(needs_footer));
    for line in text.lines.iter().take(visible_rows) {
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = rect.width();
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        for span in line {
            let [r, g, b, a] = span.color;
            job.append(
                &span.text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: Color32::from_rgba_unmultiplied(r, g, b, a),
                    ..Default::default()
                },
            );
        }
        let galley = painter.layout_job(job);
        painter.galley(Pos2::new(rect.min.x, y), galley, Color32::WHITE);
        y += line_height;
    }
    let remaining = text.more_lines + text.lines.len().saturating_sub(visible_rows);
    if remaining > 0 {
        label(
            painter,
            Pos2::new(rect.min.x, y),
            &format!("+{remaining} more lines"),
            16.0 * scale,
            muted,
        );
    }
}

fn paint_waveform(
    painter: &egui::Painter,
    rect: Rect,
    waveform: &little_clip_core::audio::AudioWaveform,
    pixels_per_point: f32,
) {
    let pixel_columns = (rect.width() * pixels_per_point).ceil().clamp(1.0, 512.0) as usize;
    let channels = [
        Some(waveform.ch1_peaks.as_slice()),
        waveform.ch2_peaks.as_deref(),
    ];
    let count = if channels[1].is_some() { 2 } else { 1 };
    for (channel, peaks) in channels.into_iter().flatten().enumerate() {
        let columns = pixel_columns.min(peaks.len().max(1));
        let lane_height = rect.height() / count as f32;
        let middle = rect.min.y + lane_height * (channel as f32 + 0.5);
        for column in 0..columns {
            let index = column * peaks.len() / columns;
            let peak = peaks.get(index).copied().unwrap_or(0.0);
            let height = if peak.is_finite() {
                peak.clamp(0.0, 1.0)
            } else {
                0.0
            } * lane_height
                * 0.45;
            let color = waveform
                .spectral_colors
                .as_ref()
                .and_then(|colors| colors.get(index))
                .copied()
                .unwrap_or([34, 197, 94]);
            let x = rect.min.x + (column as f32 + 0.5) * rect.width() / columns as f32;
            painter.line_segment(
                [Pos2::new(x, middle - height), Pos2::new(x, middle + height)],
                Stroke::new(1.0_f32, Color32::from_rgb(color[0], color[1], color[2])),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn prepared_cards_do_not_retain_the_clipboard_or_its_full_payloads() {
        let mut state = std::sync::Arc::new(SlotManager::default());
        let mut item = little_clip_core::ClipItem::from_text("x".repeat(100_000));
        item.title = "title".repeat(1000);
        std::sync::Arc::get_mut(&mut state).unwrap().push_left(item);
        let prepared = NativeMirrorData::prepare(
            &state,
            vec![(SlotId::LeftHand, PreparedTextPreview::default())],
            true,
        );
        assert!(
            std::sync::Arc::get_mut(&mut state).is_some(),
            "native display data must not force clipboard copy-on-write"
        );
        assert!(prepared.cards[0].1.title.chars().count() <= 256);
        assert!(matches!(prepared.cards[0].1.body, NativeContent::Text(_)));
        let oversized = NativeMirrorData::prepare(
            &state,
            vec![(
                SlotId::LeftHand,
                PreparedTextPreview {
                    lines: vec![vec![little_clip_render::PreparedTextSpan {
                        text: "x".repeat(4096),
                        color: [255; 4],
                    }]],
                    more_lines: 0,
                },
            )],
            true,
        );
        assert!(matches!(oversized.cards[0].1.body, NativeContent::Raster));
        drop(state);
        assert_eq!(prepared.cards.len(), 1);
    }

    #[test]
    #[ignore = "explicit headless CPU paint/tessellation comparison; excludes GPU presentation"]
    fn compare_native_and_reference_frames() {
        let mut state = SlotManager::default();
        let mut code = little_clip_core::ClipItem::from_text(
            "fn main() {\n    let café = 42;\n    // 東京\n}",
        );
        code.title = "sample.rs".into();
        state.push_left(code);
        state.right_prior1 = Some(little_clip_core::ClipItem::from_text("#42b6a4"));
        let mut audio = little_clip_core::ClipItem::from_text("prepared audio");
        audio.payload = ClipPayload::AudioRef {
            path: "/unavailable/audio.wav".into(),
            duration_ms: 12000,
            sample_rate: 48000,
            channels: 2,
            format: "WAV".into(),
            waveform: Some(little_clip_core::audio::AudioWaveform {
                channels: 2,
                sample_rate: 48000,
                duration_ms: 12000,
                ch1_peaks: [0.1, 0.7, 0.2, 0.9, 0.4, 0.6]
                    .into_iter()
                    .cycle()
                    .take(512)
                    .collect(),
                ch2_peaks: Some(
                    [0.8, 0.2, 0.6, 0.3, 0.9, 0.1]
                        .into_iter()
                        .cycle()
                        .take(512)
                        .collect(),
                ),
                spectral_colors: None,
                is_snippet: false,
                snippet_start_ms: 0,
                snippet_duration_ms: 0,
            }),
        };
        state.push_right(audio);
        state.left_prior2 = Some(little_clip_core::ClipItem::from_text(
            "# Markdown\n**Raster reference**",
        ));
        let config = little_clip_core::AppConfig::default();
        let renderer = little_clip_render::ScreenRenderer::fallback(config);
        let mut preparation = [Vec::new(), Vec::new()];
        for round in 0..24 {
            for offset in 0..2 {
                let variant = (round + offset) % 2;
                let start = Instant::now();
                if variant == 0 {
                    std::hint::black_box(renderer.render_pixmap(&state).unwrap());
                } else {
                    std::hint::black_box(renderer.render_pixmap_with_previews(&state).unwrap());
                }
                preparation[variant].push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let (raster, text) = renderer.render_pixmap_with_previews(&state).unwrap();
        let text_bytes: usize = text
            .iter()
            .flat_map(|(_, text)| text.lines.iter().flatten())
            .map(|span| span.text.capacity())
            .sum();
        let data = NativeMirrorData::prepare(&state, text, renderer.palette.is_dark);
        let palette = crate::ui_appearance::palette(Default::default());
        let mut results = Vec::new();
        for width in [900.0, 1800.0] {
            for pixels_per_point in [1.0, 2.0] {
                for native in [false, true] {
                    let ctx = egui::Context::default();
                    crate::ui_ext::configure_workbench_style(&ctx);
                    ctx.set_pixels_per_point(pixels_per_point);
                    let texture = ctx.load_texture(
                        "reference",
                        egui::ColorImage::from_rgba_premultiplied(
                            [raster.width() as usize, raster.height() as usize],
                            raster.data(),
                        ),
                        Default::default(),
                    );
                    let mut timings = Vec::new();
                    let mut vertices = 0;
                    for round in 0..230 {
                        let start = Instant::now();
                        let output = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(Rect::from_min_size(
                                    Pos2::ZERO,
                                    Vec2::new(width, 900.0),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    let mut dragging = None;
                                    super::super::screen_mirror::ScreenMirrorWidget::new(
                                        Some(&texture),
                                        2,
                                        &mut dragging,
                                        &[],
                                    )
                                    .with_native(native.then_some(&data))
                                    .show(ui, &palette);
                                });
                            },
                        );
                        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                        vertices = primitives
                            .iter()
                            .filter_map(|primitive| match &primitive.primitive {
                                egui::epaint::Primitive::Mesh(mesh) => Some(mesh.vertices.len()),
                                _ => None,
                            })
                            .sum::<usize>();
                        if round >= 30 {
                            timings.push(start.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    timings.sort_by(f64::total_cmp);
                    results.push(serde_json::json!({"width": width, "pixels_per_point": pixels_per_point, "native": native,
                        "median_ms": timings[100], "p95_ms": timings[190], "vertices": vertices}));
                }
            }
        }
        for timings in &mut preparation {
            timings.sort_by(f64::total_cmp);
        }
        println!(
            "NATIVE_MIRROR_MEASUREMENTS {}",
            serde_json::json!({
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                "scope": "headless CPU frame plus tessellation; no GPU presentation or physical input latency",
                "preparation_median_ms": {"reference": preparation[0][12], "with_native_text": preparation[1][12]},
                "prepared_text_capacity_bytes": text_bytes,
                "retained_reference_rgba_bytes": raster.data().len(), "frames": results
            })
        );
    }
}
