//! Custom in-node interactive widgets and last-run telemetry visualizations.

use super::drawing::NodeCardResponse;
use super::*;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use little_clip_core::ipc::NodeTelemetry;
use little_clip_ops::pipeline::{CdpProcessMeta, NodeKind};

/// Extra height (in unscaled canvas units) needed for a node's custom UI & telemetry widgets.
pub(super) fn extra_custom_ui_height(
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
) -> f32 {
    match &node.kind {
        NodeKind::CdpProcess => {
            let brk_count = node.props.keys().filter(|k| k.starts_with("brk_")).count();
            let mod_row = if node.props.contains_key("mod_target") {
                26.0
            } else {
                0.0
            };
            116.0 + mod_row + brk_count as f32 * 56.0
        }
        NodeKind::AudioNormalize
        | NodeKind::AudioTrimSilence
        | NodeKind::AudioAdjustGain
        | NodeKind::AudioConcat => 84.0,
        NodeKind::ImagePadFrame
        | NodeKind::ImageAddTitle
        | NodeKind::ImageCrop
        | NodeKind::ImageResize
        | NodeKind::ImageConvertWebp
        | NodeKind::ImageRotate
        | NodeKind::ImageStitch
        | NodeKind::ImageStripExif
        | NodeKind::ImageDiff => {
            let mut h = match node.kind {
                NodeKind::ImageCrop => 58.0,
                NodeKind::ImagePadFrame => 52.0,
                NodeKind::ImageRotate
                | NodeKind::ImageResize
                | NodeKind::ImageConvertWebp
                | NodeKind::ImageStitch
                | NodeKind::ImageAddTitle => 32.0,
                _ => 26.0,
            };
            if telemetry.is_some_and(|t| !t.thumbnail_png.is_empty()) {
                h += 76.0;
            }
            h
        }
        NodeKind::TextTrim
        | NodeKind::TextSlugify
        | NodeKind::TextCase
        | NodeKind::TextRegexReplace
        | NodeKind::TextSplitLines
        | NodeKind::TextJoin
        | NodeKind::TextJsonQuery
        | NodeKind::TextBase64 => {
            let mut h = 26.0;
            if telemetry.is_some_and(|t| t.text_preview.is_some()) {
                h += 34.0;
            }
            h
        }
        NodeKind::LeftHandSource
        | NodeKind::RightHandSource
        | NodeKind::SlotSource(_)
        | NodeKind::ConstantText
        | NodeKind::ConstantNumber
        | NodeKind::PromptOrHandText => {
            let mut h = 34.0;
            if telemetry.is_some_and(|t| !t.thumbnail_png.is_empty()) {
                h += 76.0;
            } else if telemetry.is_some_and(|t| t.text_preview.is_some()) {
                h += 34.0;
            }
            h
        }
        NodeKind::FileSha256
        | NodeKind::FileZip
        | NodeKind::FileExtractName
        | NodeKind::SetLeftHand
        | NodeKind::SetRightHand
        | NodeKind::SaveToDisk
        | NodeKind::PasteToApp
        | NodeKind::ImageMagickChain
        | NodeKind::AudioFilterChain => {
            let mut h = 30.0;
            if telemetry.is_some_and(|t| !t.thumbnail_png.is_empty()) {
                h += 76.0;
            } else if telemetry.is_some_and(|t| t.text_preview.is_some()) {
                h += 34.0;
            }
            h
        }
        NodeKind::PythonScript | NodeKind::BashCommand => {
            let mut h = 26.0;
            if telemetry.is_some_and(|t| t.text_preview.is_some()) {
                h += 34.0;
            }
            h
        }
        NodeKind::AutomationLfo
        | NodeKind::AutomationEnvelope
        | NodeKind::AutomationDraw
        | NodeKind::AutomationSteps => 104.0,
    }
}

fn clipped_painter(ui: &egui::Ui, rect: Rect) -> egui::Painter {
    ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()))
}

fn allocate_dark_pad(
    ui: &mut egui::Ui,
    zoom: f32,
    base_height: f32,
) -> (Rect, egui::Response, egui::Painter) {
    let width = ui.available_width().max(120.0);
    let height = (base_height * zoom).clamp(48.0, 140.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let painter = clipped_painter(ui, rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 12, 16));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, Color32::from_rgb(48, 56, 72)),
    );
    (rect, resp, painter)
}

fn cdp_param_f64(node: &NodeInstance, meta: &CdpProcessMeta, key: &str, fallback: f64) -> f64 {
    node.props
        .get(key)
        .and_then(|s| s.trim().parse::<f64>().ok())
        .or_else(|| {
            meta.parameters
                .iter()
                .find(|p| p.key == key)
                .and_then(|p| p.default_value.as_f64())
        })
        .unwrap_or(fallback)
}

fn freq_to_log_t(freq: f64) -> f32 {
    ((freq.clamp(20.0, 20000.0) / 20.0).log10() / 3.0).clamp(0.0, 1.0) as f32
}

fn log_t_to_freq(t: f64) -> f64 {
    (20.0 * (1000.0_f64).powf(t.clamp(0.0, 1.0))).clamp(20.0, 20000.0)
}

/// Renders an interactive custom graph/pad + telemetry for every CDP node.
pub(super) fn render_cdp_custom_graph(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) -> bool {
    match meta.id.as_str() {
        "filter_lohi_1" => render_filter_lohi_ui(ui, node, meta, telemetry, zoom, response),
        "filter_bank_1" | "filter_bank_2" | "filter_bank_5" | "filter_bank_6" => {
            render_filter_bank_ui(ui, node, meta, telemetry, zoom, response)
        }
        "hilite_trace_1" => render_hilite_trace_ui(ui, node, meta, telemetry, zoom, response),
        "modify_radical_4" => render_bitcrush_xy_pad(ui, node, meta, telemetry, zoom, response),
        "synth_wave_1"
        | "synth_wave_2"
        | "synth_wave_3"
        | "synth_wave_4"
        | "synth_noise"
        | "synspline_synspline" => render_synth_xy_pad(ui, node, meta, telemetry, zoom, response),
        "rmverb" | "bounce_bounce" => {
            render_decay_tap_pad(ui, node, meta, telemetry, zoom, response)
        }
        "sfedit_cut_1" | "sfedit_excise_1" | "spec_cut" => {
            render_range_cut_pad(ui, node, meta, telemetry, zoom, response)
        }
        _ => {
            let sliders: Vec<_> = meta
                .parameters
                .iter()
                .filter(|p| p.uitype == "hslider")
                .collect();
            if sliders.len() >= 2 {
                render_cdp_xy_pad(
                    ui, node, meta, sliders[0], sliders[1], telemetry, zoom, response,
                );
            } else if sliders.len() == 1 {
                render_cdp_1d_pad(ui, node, meta, sliders[0], telemetry, zoom, response);
            } else {
                render_cdp_glyph_pad(ui, node, meta, telemetry, zoom, response);
            }
        }
    }
    true
}

fn draw_cdp_backdrop(
    painter: &egui::Painter,
    rect: Rect,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
) {
    if meta.category == "pvoc" {
        draw_spectrum_grid(painter, rect, zoom);
        if let Some(tel) = telemetry {
            draw_hold_spectrum_traces(painter, rect, tel, zoom);
        }
    } else {
        let grid_col = Color32::from_rgba_unmultiplied(255, 255, 255, 18);
        for frac in [0.25_f32, 0.5, 0.75] {
            let x = rect.min.x + frac * rect.width();
            let y = rect.min.y + frac * rect.height();
            painter.line_segment(
                [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
                Stroke::new(1.0_f32, grid_col),
            );
            painter.line_segment(
                [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
                Stroke::new(1.0_f32, grid_col),
            );
        }
        if let Some(tel) = telemetry
            && !tel.waveform_peaks.is_empty()
        {
            let mid_y = rect.center().y;
            let count = tel.waveform_peaks.len();
            for (i, &pk) in tel.waveform_peaks.iter().enumerate() {
                let norm = (pk as f32 / 255.0).clamp(0.02, 1.0);
                let half_h = norm * (rect.height() * 0.42);
                let x = rect.min.x + (i as f32 + 0.5) * (rect.width() / count as f32);
                painter.line_segment(
                    [Pos2::new(x, mid_y - half_h), Pos2::new(x, mid_y + half_h)],
                    Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(168, 85, 247, 90)),
                );
            }
        }
    }
}

fn param_norm(
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    p: &little_clip_ops::pipeline::CdpParamMeta,
) -> (f64, f64, f64, f64) {
    let min = p.minrange.unwrap_or(0.0);
    let max = p.maxrange.unwrap_or(1.0);
    let span = (max - min).abs().max(1e-6);
    let val =
        cdp_param_f64(node, meta, &p.key, (min + max) * 0.5).clamp(min.min(max), min.max(max));
    let t = ((val - min) / (max - min).copysign(span)).clamp(0.0, 1.0);
    (val, min, max, t)
}

/// `modify_radical_4` (Bit Crush): X = Sample Rate Division (`param2`), Y = Bit Rate (`param1`).
fn render_bitcrush_xy_pad(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let bit_rate = cdp_param_f64(node, meta, "param1", 8.0).clamp(1.0, 16.0);
    let sr_div = cdp_param_f64(node, meta, "param2", 4.0).clamp(1.0, 64.0);
    let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 84.0);
    draw_cdp_backdrop(&painter, rect, meta, telemetry, zoom);

    // Reference smooth sine + quantized staircase sine
    let steps = 64;
    let mid_y = rect.center().y;
    let amp = rect.height() * 0.36;
    let levels = (2.0_f64.powf((bit_rate * 0.45).clamp(1.0, 6.0)))
        .round()
        .max(2.0);
    let hold_every = (sr_div / 4.0).round().clamp(1.0, 16.0) as usize;
    let mut ref_pts = Vec::with_capacity(steps + 1);
    let mut crush_pts = Vec::with_capacity(steps + 1);
    let mut held_y = mid_y;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = rect.min.x + t * rect.width();
        let s = (t as f64 * std::f64::consts::TAU * 2.0).sin();
        ref_pts.push(Pos2::new(x, mid_y - s as f32 * amp));
        if i % hold_every == 0 {
            let q = (s * levels).round() / levels;
            held_y = mid_y - q as f32 * amp;
        }
        crush_pts.push(Pos2::new(x, held_y));
    }
    painter.add(egui::Shape::line(
        ref_pts,
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(120, 140, 170, 90)),
    ));
    painter.add(egui::Shape::line(
        crush_pts,
        Stroke::new(1.8_f32, Color32::from_rgb(245, 158, 11)),
    ));

    let tx = ((sr_div - 1.0) / 63.0).clamp(0.0, 1.0) as f32;
    let ty = ((bit_rate - 1.0) / 15.0).clamp(0.0, 1.0) as f32;
    let dot = Pos2::new(
        rect.min.x + tx * rect.width(),
        rect.max.y - ty * rect.height(),
    );
    painter.circle_filled(dot, 4.6 * zoom.clamp(0.7, 1.4), Color32::WHITE);

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ny = (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        let new_sr = 1.0 + nx * 63.0;
        let new_bits = 1.0 + ny * 15.0;
        response
            .property_edits
            .push(("param2".into(), format!("{new_sr:.1}")));
        response
            .property_edits
            .push(("param1".into(), format!("{new_bits:.1}")));
    }

    resp.on_hover_text(format!(
        "Bit Crush X-Y Pad: {bit_rate:.1} bits, /{sr_div:.1} sample rate\nDrag X = Sample Rate Div, Y = Bit Depth"
    ));
}

/// Interactive waveform generator X-Y pad for `synth_wave_1..4`, `synth_noise`, `synspline_synspline`.
fn render_synth_xy_pad(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let (freq_key, amp_key) = match meta.id.as_str() {
        "synth_noise" => ("param3", "param4"),
        "synspline_synspline" => ("param3", "param4"),
        _ => ("param4", "param5"),
    };
    let freq = cdp_param_f64(node, meta, freq_key, 440.0).clamp(20.0, 20000.0);
    let amp = if meta.id == "synspline_synspline" {
        (cdp_param_f64(node, meta, amp_key, 8.0) / 64.0).clamp(0.05, 1.0)
    } else {
        cdp_param_f64(node, meta, amp_key, 0.8).clamp(0.0, 1.0)
    };

    let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 84.0);
    draw_cdp_backdrop(&painter, rect, meta, telemetry, zoom);

    let cycles = (1.0 + freq_to_log_t(freq) * 5.0) as f64;
    let mid_y = rect.center().y;
    let h_amp = rect.height() * 0.38 * amp as f32;
    let steps = 72;
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let phase = (t * cycles) % 1.0;
        let y_norm = match meta.id.as_str() {
            "synth_wave_2" => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            "synth_wave_3" => 1.0 - 4.0 * (phase - 0.5).abs(),
            "synth_wave_4" => 2.0 * phase - 1.0,
            "synth_noise" => ((i as f64 * 12.9898).sin() * 43758.5453).fract() * 2.0 - 1.0,
            _ => (phase * std::f64::consts::TAU).sin(),
        };
        pts.push(Pos2::new(
            rect.min.x + t as f32 * rect.width(),
            mid_y - y_norm as f32 * h_amp,
        ));
    }
    painter.add(egui::Shape::line(
        pts,
        Stroke::new(1.8_f32, Color32::from_rgb(56, 189, 248)),
    ));

    let dot = Pos2::new(
        rect.min.x + freq_to_log_t(freq) * rect.width(),
        rect.max.y - amp as f32 * rect.height(),
    );
    painter.circle_filled(dot, 4.5 * zoom.clamp(0.7, 1.4), Color32::WHITE);

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ny = (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        if meta.id != "synth_noise" {
            let f = log_t_to_freq(nx);
            response
                .property_edits
                .push((freq_key.into(), format!("{f:.1}")));
        }
        let y_val = if meta.id == "synspline_synspline" {
            1.0 + ny * 63.0
        } else {
            ny
        };
        response
            .property_edits
            .push((amp_key.into(), format!("{y_val:.2}")));
    }

    resp.on_hover_text(format!(
        "{}: Drag X = Frequency/Duration, Y = Amplitude/Splines",
        meta.title
    ));
}

/// Interactive decaying tap/envelope pad for `rmverb` and `bounce_bounce`.
fn render_decay_tap_pad(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let (x_key, x_min, x_max, y_key, y_min, y_max) = if meta.id == "rmverb" {
        ("param4", 0.1, 10.0, "param3", 0.0, 1.0)
    } else {
        ("param2", 0.04, 10.0, "param3", 0.1, 1.0)
    };
    let xv = cdp_param_f64(node, meta, x_key, (x_min + x_max) * 0.5).clamp(x_min, x_max);
    let yv = cdp_param_f64(node, meta, y_key, (y_min + y_max) * 0.5).clamp(y_min, y_max);
    let tx = ((xv - x_min) / (x_max - x_min)).clamp(0.0, 1.0) as f32;
    let ty = ((yv - y_min) / (y_max - y_min)).clamp(0.0, 1.0) as f32;

    let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 84.0);
    draw_cdp_backdrop(&painter, rect, meta, telemetry, zoom);

    let taps = 12;
    for i in 0..taps {
        let frac = i as f32 / (taps - 1) as f32;
        let x = rect.min.x + 6.0 + frac.powf(0.75 + (1.0 - tx) * 0.8) * (rect.width() - 12.0);
        let h = (1.0 - frac * (0.85 - tx * 0.45)).max(0.08)
            * (0.25 + 0.7 * ty)
            * (rect.height() - 10.0);
        painter.line_segment(
            [
                Pos2::new(x, rect.max.y - 4.0),
                Pos2::new(x, rect.max.y - 4.0 - h),
            ],
            Stroke::new(3.0_f32, Color32::from_rgb(120, 210, 190)),
        );
    }

    let dot = Pos2::new(
        rect.min.x + tx * rect.width(),
        rect.max.y - ty * rect.height(),
    );
    painter.circle_filled(dot, 4.5 * zoom.clamp(0.7, 1.4), Color32::WHITE);

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ny = (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        response
            .property_edits
            .push((x_key.into(), format!("{:.2}", x_min + nx * (x_max - x_min))));
        response
            .property_edits
            .push((y_key.into(), format!("{:.2}", y_min + ny * (y_max - y_min))));
    }

    resp.on_hover_text(format!(
        "{}: Drag X = Time/Decay, Y = Mix/Bounce",
        meta.title
    ));
}

/// Interactive Start–End range bar for `sfedit_cut_1`, `sfedit_excise_1`, `spec_cut`.
fn render_range_cut_pad(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let Some(p1) = meta.parameters.iter().find(|p| p.key == "param1") else {
        return;
    };
    let Some(p2) = meta.parameters.iter().find(|p| p.key == "param2") else {
        return;
    };
    let (v1, min1, max1, t1) = param_norm(node, meta, p1);
    let (v2, min2, max2, t2) = param_norm(node, meta, p2);
    let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 84.0);
    draw_cdp_backdrop(&painter, rect, meta, telemetry, zoom);

    let x1 = rect.min.x + t1 as f32 * rect.width();
    let x2 = rect.min.x + t2 as f32 * rect.width();
    let lo = x1.min(x2);
    let hi = x1.max(x2);
    let sel_col = if meta.id == "sfedit_excise_1" {
        Color32::from_rgba_unmultiplied(244, 63, 94, 55)
    } else {
        Color32::from_rgba_unmultiplied(56, 189, 248, 55)
    };
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(lo, rect.min.y + 2.0),
            Pos2::new(hi, rect.max.y - 2.0),
        ),
        2.0,
        sel_col,
    );
    for (x, col) in [
        (x1, Color32::from_rgb(120, 220, 160)),
        (x2, Color32::from_rgb(240, 160, 95)),
    ] {
        painter.line_segment(
            [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
            Stroke::new(2.0_f32, col),
        );
        painter.circle_filled(Pos2::new(x, rect.center().y), 4.2, col);
    }

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        if (pos.x - x1).abs() <= (pos.x - x2).abs() {
            let val = min1 + nx * (max1 - min1);
            response
                .property_edits
                .push(("param1".into(), format!("{val:.2}")));
        } else {
            let val = min2 + nx * (max2 - min2);
            response
                .property_edits
                .push(("param2".into(), format!("{val:.2}")));
        }
    }

    resp.on_hover_text(format!(
        "{}: Start {v1:.2}s — End {v2:.2}s\nDrag green/orange handles",
        meta.title
    ));
}

/// Universal interactive X-Y pad for any CDP node with 2+ `hslider` parameters.
#[allow(clippy::too_many_arguments)]
fn render_cdp_xy_pad(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    px: &little_clip_ops::pipeline::CdpParamMeta,
    py: &little_clip_ops::pipeline::CdpParamMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let (vx, min_x, max_x, tx) = param_norm(node, meta, px);
    let (vy, min_y, max_y, ty) = param_norm(node, meta, py);
    let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 84.0);
    draw_cdp_backdrop(&painter, rect, meta, telemetry, zoom);

    // Abstract parametric harmonic curve driven by (tx, ty)
    let steps = 48;
    let mut pts = Vec::with_capacity(steps + 1);
    let mid_y = rect.center().y;
    let amp = rect.height() * (0.12 + 0.30 * ty as f32);
    let freq_cycles = 1.0 + tx * 4.5;
    for i in 0..=steps {
        let f = i as f64 / steps as f64;
        let y_off = ((f * std::f64::consts::TAU * freq_cycles).sin()
            + 0.35 * (f * std::f64::consts::TAU * (freq_cycles + 1.0)).cos())
            * 0.74;
        pts.push(Pos2::new(
            rect.min.x + f as f32 * rect.width(),
            mid_y - y_off as f32 * amp,
        ));
    }
    painter.add(egui::Shape::line(
        pts,
        Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(130, 195, 250, 210)),
    ));

    let dot = Pos2::new(
        rect.min.x + tx as f32 * rect.width(),
        rect.max.y - ty as f32 * rect.height(),
    );
    painter.line_segment(
        [Pos2::new(dot.x, rect.min.y), Pos2::new(dot.x, rect.max.y)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 35)),
    );
    painter.line_segment(
        [Pos2::new(rect.min.x, dot.y), Pos2::new(rect.max.x, dot.y)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 35)),
    );
    painter.circle_filled(dot, 4.5 * zoom.clamp(0.7, 1.4), Color32::WHITE);

    if zoom >= 0.7 {
        let font = egui::FontId::proportional(8.5 * zoom.clamp(0.7, 1.15));
        painter.text(
            rect.left_top() + Vec2::new(4.0, 2.0),
            egui::Align2::LEFT_TOP,
            format!("{}: {vx:.2}  |  {}: {vy:.2}", px.paramname, py.paramname),
            font,
            Color32::from_rgb(170, 185, 205),
        );
    }

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ny = (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        let new_x = min_x + nx * (max_x - min_x);
        let new_y = min_y + ny * (max_y - min_y);
        response
            .property_edits
            .push((px.key.clone(), format!("{new_x:.2}")));
        response
            .property_edits
            .push((py.key.clone(), format!("{new_y:.2}")));
    }

    resp.on_hover_text(format!(
        "X-Y Pad: X = {} ({vx:.2}), Y = {} ({vy:.2})",
        px.paramname, py.paramname
    ));
}

/// Universal interactive 1D fader pad for any CDP node with 1 `hslider` parameter.
fn render_cdp_1d_pad(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    px: &little_clip_ops::pipeline::CdpParamMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let (vx, min_x, max_x, tx) = param_norm(node, meta, px);
    let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 84.0);
    draw_cdp_backdrop(&painter, rect, meta, telemetry, zoom);

    let fill_w = tx as f32 * rect.width();
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height())),
        2.0,
        Color32::from_rgba_unmultiplied(56, 189, 248, 38),
    );
    let bar_x = rect.min.x + fill_w;
    painter.line_segment(
        [Pos2::new(bar_x, rect.min.y), Pos2::new(bar_x, rect.max.y)],
        Stroke::new(2.0_f32, Color32::from_rgb(56, 189, 248)),
    );
    painter.circle_filled(
        Pos2::new(bar_x, rect.center().y),
        4.4 * zoom.clamp(0.7, 1.4),
        Color32::WHITE,
    );

    if zoom >= 0.7 {
        painter.text(
            rect.left_top() + Vec2::new(4.0, 2.0),
            egui::Align2::LEFT_TOP,
            format!("{}: {vx:.2}", px.paramname),
            egui::FontId::proportional(8.5 * zoom.clamp(0.7, 1.15)),
            Color32::from_rgb(175, 195, 220),
        );
    }

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let new_x = min_x + nx * (max_x - min_x);
        response
            .property_edits
            .push((px.key.clone(), format!("{new_x:.2}")));
    }

    resp.on_hover_text(format!("Drag horizontally: {} ({vx:.2})", px.paramname));
}

/// Interactive vector glyph + bypass/telemetry pad for 0-slider CDP nodes (`pvoc_anal_1`, `pvoc_synth`, `distort_reform_*`, etc.).
fn render_cdp_glyph_pad(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 84.0);
    draw_cdp_backdrop(&painter, rect, meta, telemetry, zoom);

    let bypassed = node
        .props
        .get("bypass")
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "True"));
    let col = if bypassed {
        Color32::from_rgb(120, 128, 142)
    } else {
        Color32::from_rgb(130, 210, 250)
    };

    let mid_y = rect.center().y;
    let amp = rect.height() * 0.30;
    let steps = 40;
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = rect.min.x + 8.0 + t * (rect.width() - 16.0);
        let y_off = match meta.id.as_str() {
            "distort_reform_2" => {
                if (t * 2.0) % 1.0 < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            "distort_reform_4" => 1.0 - 4.0 * ((t * 2.0) % 1.0 - 0.5).abs(),
            "distort_reform_6" => {
                if i == steps / 2 {
                    1.0
                } else {
                    0.0
                }
            }
            "strange_invert_1" => -(t * std::f32::consts::TAU * 2.0).sin(),
            _ => (t * std::f32::consts::TAU * 2.0).sin(),
        };
        pts.push(Pos2::new(x, mid_y - y_off * amp));
    }
    painter.add(egui::Shape::line(pts, Stroke::new(1.6_f32, col)));

    if zoom >= 0.7 {
        let status = if bypassed { "BYPASSED" } else { "ACTIVE" };
        painter.text(
            rect.left_top() + Vec2::new(4.0, 2.0),
            egui::Align2::LEFT_TOP,
            format!("{} [{status}]", meta.title),
            egui::FontId::proportional(8.5 * zoom.clamp(0.7, 1.15)),
            col,
        );
    }

    if resp.clicked() && meta.allowbypass {
        response.property_edits.push((
            "bypass".into(),
            if bypassed { "false" } else { "true" }.into(),
        ));
    }

    resp.on_hover_text(if meta.allowbypass {
        format!("{}: Click to toggle Bypass", meta.title)
    } else {
        meta.title.clone()
    });
}

/// `filter_lohi_1`: `param1` = Rolloff (dB/oct, -96..-1), `param2` = Pass-band (Hz),
/// `param3` = Stop-band (Hz). Pass < Stop => Low-pass; Pass > Stop => High-pass.
fn render_filter_lohi_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let rolloff = cdp_param_f64(node, meta, "param1", -60.0).clamp(-96.0, -1.0);
    let pass_hz = cdp_param_f64(node, meta, "param2", 440.0).clamp(20.0, 20000.0);
    let stop_hz = cdp_param_f64(node, meta, "param3", 10000.0).clamp(20.0, 20000.0);
    let is_lowpass = pass_hz <= stop_hz;

    let width = ui.available_width().max(120.0);
    let height = (84.0 * zoom).clamp(48.0, 140.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let painter = clipped_painter(ui, rect);

    draw_spectrum_grid(&painter, rect, zoom);
    if let Some(tel) = telemetry {
        draw_hold_spectrum_traces(&painter, rect, tel, zoom);
    }

    let steps = 48;
    let mut curve_pts = Vec::with_capacity(steps + 1);
    let mut fill_pts = Vec::with_capacity(steps + 3);
    let zero_y = db_to_y(rect, 0.0);
    let pass_log = pass_hz.log10();
    let stop_log = stop_hz.log10();
    let span_log = (stop_log - pass_log).abs().max(0.02);
    let atten_db = rolloff.clamp(-18.0, -1.0);

    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let f = log_t_to_freq(t as f64);
        let f_log = f.log10();
        let mag_db = if is_lowpass {
            if f <= pass_hz {
                0.0
            } else if f >= stop_hz {
                atten_db
            } else {
                let frac = ((f_log - pass_log) / span_log).clamp(0.0, 1.0);
                atten_db * frac
            }
        } else if f >= pass_hz {
            0.0
        } else if f <= stop_hz {
            atten_db
        } else {
            let frac = ((pass_log - f_log) / span_log).clamp(0.0, 1.0);
            atten_db * frac
        };
        let x = rect.min.x + t * rect.width();
        let y = db_to_y(rect, mag_db as f32);
        curve_pts.push(Pos2::new(x, y));
    }

    if curve_pts.len() >= 2 {
        fill_pts.push(Pos2::new(rect.min.x, zero_y));
        fill_pts.extend_from_slice(&curve_pts);
        fill_pts.push(Pos2::new(rect.max.x, zero_y));
        painter.add(egui::Shape::convex_polygon(
            fill_pts,
            Color32::from_rgba_unmultiplied(120, 165, 220, 28),
            Stroke::NONE,
        ));
        painter.add(egui::Shape::line(
            curve_pts,
            Stroke::new(1.8 * zoom.clamp(0.6, 1.5), Color32::from_rgb(235, 240, 248)),
        ));
    }

    let pass_pos = Pos2::new(rect.min.x + freq_to_log_t(pass_hz) * rect.width(), zero_y);
    let stop_pos = Pos2::new(
        rect.min.x + freq_to_log_t(stop_hz) * rect.width(),
        db_to_y(rect, atten_db as f32),
    );
    let dot_r = 4.6 * zoom.clamp(0.7, 1.4);
    painter.circle_filled(pass_pos, dot_r, Color32::from_rgb(120, 220, 160));
    painter.circle_stroke(
        pass_pos,
        dot_r,
        Stroke::new(1.2_f32, Color32::from_rgb(25, 35, 45)),
    );
    painter.circle_filled(stop_pos, dot_r, Color32::from_rgb(240, 145, 110));
    painter.circle_stroke(
        stop_pos,
        dot_r,
        Stroke::new(1.2_f32, Color32::from_rgb(25, 35, 45)),
    );

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let tx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ty = ((pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        let f_hz = log_t_to_freq(tx);
        if (pos - pass_pos).length() <= (pos - stop_pos).length() {
            response
                .property_edits
                .push(("param2".to_string(), format!("{f_hz:.1}")));
        } else {
            response
                .property_edits
                .push(("param3".to_string(), format!("{f_hz:.1}")));
            let new_rolloff = (-1.0 - ty * 95.0).clamp(-96.0, -1.0);
            response
                .property_edits
                .push(("param1".to_string(), format!("{new_rolloff:.1}")));
        }
    }

    let mode_lbl = if is_lowpass { "Low-Pass" } else { "High-Pass" };
    resp.on_hover_text(format!(
        "{mode_lbl} (Pass: {pass_hz:.0} Hz, Stop: {stop_hz:.0} Hz, Rolloff: {rolloff:.0} dB)\nDrag green dot = Pass-band, orange dot = Stop-band & Rolloff"
    ));
}

/// `filter_bank_1/2/5/6`: `param1` = Q (1..10000), `param2` = Makeup Gain,
/// `param3` = Lowest Band (Hz), `param4` = Highest Band (Hz), `param5` = Scatter / Count / Semitones.
fn render_filter_bank_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let q_val = cdp_param_f64(node, meta, "param1", 400.0).clamp(1.0, 10000.0);
    let lo_hz = cdp_param_f64(node, meta, "param3", 55.0).clamp(20.0, 12000.0);
    let hi_hz = cdp_param_f64(node, meta, "param4", 12000.0).clamp(20.0, 12000.0);
    let p5 = cdp_param_f64(node, meta, "param5", 1.0);

    let lo = lo_hz.min(hi_hz);
    let hi = lo_hz.max(hi_hz).max(lo + 1.0);

    let mut bank_freqs = Vec::new();
    match meta.id.as_str() {
        "filter_bank_1" | "filter_bank_2" => {
            let step = if meta.id == "filter_bank_2" { 2 } else { 1 };
            let mut h = 1usize;
            while bank_freqs.len() < 24 {
                let f = lo * h as f64;
                if f > hi {
                    break;
                }
                bank_freqs.push(f);
                h += step;
            }
        }
        "filter_bank_5" => {
            let count = (p5.round() as usize).clamp(2, 24);
            for i in 0..count {
                let frac = i as f64 / (count - 1) as f64;
                bank_freqs.push(lo + frac * (hi - lo));
            }
        }
        _ => {
            let semitones = p5.clamp(1.0, 24.0);
            let ratio = 2.0_f64.powf(semitones / 12.0);
            let mut f = lo;
            while f <= hi && bank_freqs.len() < 24 {
                bank_freqs.push(f);
                f *= ratio;
            }
        }
    }

    let width = ui.available_width().max(120.0);
    let height = (84.0 * zoom).clamp(48.0, 140.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let painter = clipped_painter(ui, rect);

    draw_spectrum_grid(&painter, rect, zoom);
    if let Some(tel) = telemetry {
        draw_hold_spectrum_traces(&painter, rect, tel, zoom);
    }

    // Q controls peak sharpness and display height (-6 dB..+14 dB)
    let q_norm = (q_val.log10() / 4.0).clamp(0.0, 1.0);
    let peak_y = db_to_y(rect, (-4.0 + q_norm * 18.0) as f32);
    let base_y = db_to_y(rect, -16.0);
    let half_w = (14.0 - q_norm * 11.0).max(2.0) as f32 * zoom.clamp(0.7, 1.3);

    for f in &bank_freqs {
        let x = rect.min.x + freq_to_log_t(*f) * rect.width();
        let pts = vec![
            Pos2::new((x - half_w).max(rect.min.x), base_y),
            Pos2::new(x, peak_y),
            Pos2::new((x + half_w).min(rect.max.x), base_y),
        ];
        painter.add(egui::Shape::line(
            pts,
            Stroke::new(1.3 * zoom.clamp(0.6, 1.3), Color32::from_rgb(160, 210, 250)),
        ));
    }

    let lo_pos = Pos2::new(rect.min.x + freq_to_log_t(lo_hz) * rect.width(), peak_y);
    let hi_pos = Pos2::new(rect.min.x + freq_to_log_t(hi_hz) * rect.width(), peak_y);
    let dot_r = 4.4 * zoom.clamp(0.7, 1.4);
    painter.circle_filled(lo_pos, dot_r, Color32::from_rgb(120, 220, 160));
    painter.circle_filled(hi_pos, dot_r, Color32::from_rgb(240, 185, 95));

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let tx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ty = ((pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        let f_hz = log_t_to_freq(tx).clamp(20.0, 12000.0);
        let new_q = (10.0_f64).powf((1.0 - ty) * 4.0).clamp(1.0, 10000.0);
        if (pos.x - lo_pos.x).abs() <= (pos.x - hi_pos.x).abs() {
            response
                .property_edits
                .push(("param3".to_string(), format!("{f_hz:.1}")));
        } else {
            response
                .property_edits
                .push(("param4".to_string(), format!("{f_hz:.1}")));
        }
        response
            .property_edits
            .push(("param1".to_string(), format!("{new_q:.1}")));
    }

    resp.on_hover_text(format!(
        "Filter Bank ({} bands, Q: {q_val:.0}, Range: {lo_hz:.0}–{hi_hz:.0} Hz)\nDrag green/amber dots: X = Lowest/Highest Band, Y = Q",
        bank_freqs.len()
    ));
}

/// `hilite_trace_1`: `param1` = Amount Kept (0.1..50.0, spectral partial magnitude threshold).
fn render_hilite_trace_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    meta: &CdpProcessMeta,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let kept = cdp_param_f64(node, meta, "param1", 15.0).clamp(0.1, 50.0);

    let width = ui.available_width().max(120.0);
    let height = (84.0 * zoom).clamp(48.0, 140.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let painter = clipped_painter(ui, rect);

    draw_spectrum_grid(&painter, rect, zoom);
    if let Some(tel) = telemetry {
        draw_hold_spectrum_traces(&painter, rect, tel, zoom);
    }

    let kept_frac = ((kept - 0.1) / 49.9).clamp(0.0, 1.0) as f32;
    let thresh_y = rect.min.y + kept_frac * rect.height();
    let keep_rect = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, thresh_y));
    painter.rect_filled(
        keep_rect,
        0.0,
        Color32::from_rgba_unmultiplied(120, 200, 170, 24),
    );
    painter.line_segment(
        [
            Pos2::new(rect.min.x, thresh_y),
            Pos2::new(rect.max.x, thresh_y),
        ],
        Stroke::new(1.6 * zoom.clamp(0.6, 1.4), Color32::from_rgb(130, 225, 180)),
    );
    let handle_pos = Pos2::new(rect.center().x, thresh_y);
    painter.circle_filled(
        handle_pos,
        4.5 * zoom.clamp(0.7, 1.4),
        Color32::from_rgb(130, 225, 180),
    );

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let ty = ((pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        let new_kept = (0.1 + ty * 49.9).clamp(0.1, 50.0);
        response
            .property_edits
            .push(("param1".to_string(), format!("{new_kept:.1}")));
    }

    resp.on_hover_text(format!(
        "Spectral Trace: keeping top {kept:.1} partials\nDrag vertically to adjust Amount Kept"
    ));
}

/// Renders a dark waveform + hold-spectrum preview and custom audio controls for core Audio nodes.
pub(super) fn render_core_audio_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    if let Some(tel) = telemetry
        && (!tel.waveform_peaks.is_empty() || !tel.after_spectrum_db.is_empty())
    {
        render_audio_telemetry_strip(ui, node, tel, zoom, response);
        ui.add_space(4.0);
    } else if matches!(
        node.kind,
        NodeKind::AudioAdjustGain
            | NodeKind::AudioTrimSilence
            | NodeKind::AudioNormalize
            | NodeKind::AudioConcat
    ) {
        // Always show interactive compact bar even before first run
        render_audio_telemetry_strip(ui, node, &NodeTelemetry::default(), zoom, response);
        ui.add_space(4.0);
    }

    match node.kind {
        NodeKind::AudioAdjustGain => {
            let mut db = node
                .props
                .get("gain_db")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(3.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Slider::new(&mut db, -24.0..=24.0).suffix(" dB"))
                    .changed()
                {
                    response
                        .property_edits
                        .push(("gain_db".into(), format!("{db:.1}")));
                }
            });
            ui.horizontal(|ui| {
                for preset in [-6.0_f64, 0.0, 6.0] {
                    if ui.small_button(format!("{preset:+.0}dB")).clicked() {
                        response
                            .property_edits
                            .push(("gain_db".into(), format!("{preset:.1}")));
                    }
                }
            });
        }
        NodeKind::AudioTrimSilence => {
            let mut db = node
                .props
                .get("threshold_db")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(-50.0);
            if ui
                .add(egui::Slider::new(&mut db, -80.0..=-6.0).suffix(" dB"))
                .changed()
            {
                response
                    .property_edits
                    .push(("threshold_db".into(), format!("{db:.1}")));
            }
        }
        NodeKind::AudioNormalize => {
            let mut target = node
                .props
                .get("target_lufs")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(-14.0);
            if ui
                .add(egui::Slider::new(&mut target, -30.0..=-5.0).suffix(" LUFS"))
                .changed()
            {
                response
                    .property_edits
                    .push(("target_lufs".into(), format!("{target:.1}")));
            }
        }
        NodeKind::ImageCrop => {
            let geom = node
                .props
                .get("geometry")
                .map(String::as_str)
                .unwrap_or("1000x1000+0+0");
            let (wh, xy) = geom.split_once('+').unwrap_or((geom, "0+0"));
            let (w_s, h_s) = wh.split_once('x').unwrap_or(("1000", "1000"));
            let (x_s, y_s) = xy.split_once('+').unwrap_or(("0", "0"));
            let x = x_s.trim().parse::<f32>().unwrap_or(0.0);
            let y = y_s.trim().parse::<f32>().unwrap_or(0.0);
            let w = w_s.trim().parse::<f32>().unwrap_or(1000.0).max(1.0);
            let h = h_s.trim().parse::<f32>().unwrap_or(1000.0).max(1.0);
            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 44.0);
            let nx = (x / 1920.0).clamp(0.0, 0.75);
            let ny = (y / 1080.0).clamp(0.0, 0.75);
            let nw = (w / 1920.0).clamp(0.15, 1.0 - nx);
            let nh = (h / 1080.0).clamp(0.15, 1.0 - ny);
            let crop_rect = Rect::from_min_size(
                Pos2::new(
                    rect.min.x + nx * rect.width(),
                    rect.min.y + ny * rect.height(),
                ),
                Vec2::new(nw * rect.width(), nh * rect.height()),
            );
            painter.rect_filled(
                crop_rect,
                2.0,
                Color32::from_rgba_unmultiplied(56, 189, 248, 45),
            );
            painter.rect_stroke(
                crop_rect,
                2.0,
                Stroke::new(1.3_f32, Color32::from_rgb(56, 189, 248)),
            );
            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let tx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 0.85);
                let ty = ((pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 0.85);
                let new_geom = format!("{w:.0}x{h:.0}+{:.0}+{:.0}", tx * 1920.0, ty * 1080.0);
                response.property_edits.push(("geometry".into(), new_geom));
            }
            ui.horizontal(|ui| {
                for (lbl, cw, ch) in [("1:1", 512, 512), ("16:9", 1280, 720), ("4:3", 800, 600)] {
                    if ui.small_button(lbl).clicked() {
                        response
                            .property_edits
                            .push(("geometry".into(), format!("{cw}x{ch}+{x:.0}+{y:.0}")));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        NodeKind::ImagePadFrame => {
            let pad = node
                .props
                .get("border_size")
                .and_then(|s| s.trim().parse::<f32>().ok())
                .unwrap_or(24.0)
                .clamp(0.0, 128.0);
            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 40.0);
            let inset = (pad / 128.0) * (rect.height().min(rect.width()) * 0.36);
            let inner = rect.shrink(inset.max(3.0));
            painter.rect_filled(inner, 2.0, Color32::from_rgb(45, 58, 80));
            painter.rect_stroke(
                inner,
                2.0,
                Stroke::new(1.2_f32, Color32::from_rgb(130, 195, 250)),
            );
            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let tx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
                response
                    .property_edits
                    .push(("border_size".into(), format!("{:.0}", tx * 128.0)));
            }
            ui.horizontal(|ui| {
                for px in [8_u32, 24, 48] {
                    if ui.small_button(format!("{px}px")).clicked() {
                        response
                            .property_edits
                            .push(("border_size".into(), px.to_string()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        NodeKind::ImageStitch => {
            let dir = node
                .props
                .get("direction")
                .map(String::as_str)
                .unwrap_or("horizontal");
            ui.horizontal(|ui| {
                for opt in ["horizontal", "vertical"] {
                    if ui.selectable_label(dir == opt, opt).clicked() {
                        response
                            .property_edits
                            .push(("direction".into(), opt.into()));
                    }
                }
            });
        }
        NodeKind::ImageStripExif => {
            let mut keep_orient = node
                .props
                .get("keep_orientation")
                .is_some_and(|v| v == "true");
            if ui
                .checkbox(&mut keep_orient, "Preserve orientation")
                .changed()
            {
                response.property_edits.push((
                    "keep_orientation".into(),
                    if keep_orient { "true" } else { "false" }.into(),
                ));
            }
        }
        NodeKind::ImageDiff => {
            let mut mode = node
                .props
                .get("highlight")
                .map(String::as_str)
                .unwrap_or("overlay");
            ui.horizontal(|ui| {
                for opt in ["overlay", "heatmap"] {
                    if ui.selectable_label(mode == opt, opt).clicked() {
                        mode = opt;
                        response
                            .property_edits
                            .push(("highlight".into(), opt.into()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        NodeKind::ImageAddTitle => {
            let pos = node
                .props
                .get("position")
                .map(String::as_str)
                .unwrap_or("bottom");
            ui.horizontal(|ui| {
                for opt in ["top", "bottom"] {
                    if ui.selectable_label(pos == opt, opt).clicked() {
                        response
                            .property_edits
                            .push(("position".into(), opt.into()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        _ => {
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
    }
}

/// Renders waveform peaks + optional before/after spectrum overlay for any audio CDP node.
pub(super) fn render_audio_telemetry_strip(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: &NodeTelemetry,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let width = ui.available_width().max(100.0);
    let height = (46.0 * zoom).clamp(30.0, 72.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let painter = clipped_painter(ui, rect);

    painter.rect_filled(rect, 3.0, Color32::from_rgb(12, 15, 21));
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0_f32, Color32::from_rgb(44, 52, 68)),
    );

    let mid_y = rect.center().y;
    painter.line_segment(
        [Pos2::new(rect.min.x, mid_y), Pos2::new(rect.max.x, mid_y)],
        Stroke::new(1.0_f32, Color32::from_rgb(35, 42, 54)),
    );

    if !telemetry.waveform_peaks.is_empty() {
        let count = telemetry.waveform_peaks.len();
        let bar_w = (rect.width() / count as f32).max(1.5);
        for (i, &pk) in telemetry.waveform_peaks.iter().enumerate() {
            let norm = (pk as f32 / 255.0).clamp(0.02, 1.0);
            let half_h = norm * (rect.height() * 0.44);
            let x = rect.min.x + (i as f32 + 0.5) * (rect.width() / count as f32);
            painter.line_segment(
                [Pos2::new(x, mid_y - half_h), Pos2::new(x, mid_y + half_h)],
                Stroke::new((bar_w * 0.7).max(1.2), Color32::from_rgb(168, 85, 247)),
            );
        }
    } else if !telemetry.after_spectrum_db.is_empty() {
        draw_hold_spectrum_traces(&painter, rect, telemetry, zoom);
    }

    if node.kind == NodeKind::AudioTrimSilence {
        let thresh_db = node
            .props
            .get("threshold_db")
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(-50.0);
        let lin = 10.0_f32.powf(thresh_db / 20.0).clamp(0.01, 1.0);
        let dy = lin * (rect.height() * 0.44);
        let line_col = Color32::from_rgb(251, 191, 36);
        painter.line_segment(
            [
                Pos2::new(rect.min.x, mid_y - dy),
                Pos2::new(rect.max.x, mid_y - dy),
            ],
            Stroke::new(1.0_f32, line_col),
        );
        painter.line_segment(
            [
                Pos2::new(rect.min.x, mid_y + dy),
                Pos2::new(rect.max.x, mid_y + dy),
            ],
            Stroke::new(1.0_f32, line_col),
        );
        if (resp.dragged() || resp.clicked())
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let norm_dy = ((pos.y - mid_y).abs() / (rect.height() * 0.44)).clamp(0.001, 1.0);
            let new_db = (20.0 * norm_dy.log10()).clamp(-80.0, -6.0);
            response
                .property_edits
                .push(("threshold_db".into(), format!("{new_db:.1}")));
        }
    }

    if let Some(peak_db) = telemetry.peak_db {
        painter.text(
            rect.right_top() + Vec2::new(-4.0, 2.0),
            egui::Align2::RIGHT_TOP,
            format!("{peak_db:.1} dBFS"),
            egui::FontId::monospace(9.0 * zoom.clamp(0.7, 1.2)),
            Color32::from_rgb(180, 195, 215),
        );
    }
}

/// Interactive 2D breakfile curve editor for `automatable` CDP parameters.
pub(super) fn render_breakfile_editor(
    ui: &mut egui::Ui,
    param_key: &str,
    min_val: f64,
    max_val: f64,
    current_json: &str,
    zoom: f32,
    response: &mut NodeCardResponse,
) {
    let mut points: Vec<[f64; 2]> =
        serde_json::from_str(current_json).unwrap_or_else(|_| vec![[0.0, min_val], [1.0, max_val]]);
    if points.len() < 2 {
        points = vec![[0.0, min_val], [1.0, max_val]];
    }
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));

    let width = ui.available_width().max(100.0);
    let height = (44.0 * zoom).clamp(32.0, 64.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let painter = clipped_painter(ui, rect);

    painter.rect_filled(rect, 3.0, Color32::from_rgb(14, 18, 26));
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0_f32, Color32::from_rgb(56, 72, 96)),
    );

    let span = (max_val - min_val).max(1e-6);
    let screen_pts: Vec<Pos2> = points
        .iter()
        .map(|&[t, v]| {
            let nx = t.clamp(0.0, 1.0) as f32;
            let ny = ((v - min_val) / span).clamp(0.0, 1.0) as f32;
            Pos2::new(
                rect.min.x + nx * rect.width(),
                rect.max.y - ny * rect.height(),
            )
        })
        .collect();

    if screen_pts.len() >= 2 {
        painter.add(egui::Shape::line(
            screen_pts.clone(),
            Stroke::new(1.5_f32, Color32::from_rgb(56, 189, 248)),
        ));
    }
    for &p in &screen_pts {
        painter.circle_filled(p, 3.2, Color32::WHITE);
    }

    if (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let t = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
        let ny = (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
        let v = min_val + ny * span;

        if let Some((idx, _)) = points
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (a[0] - t).abs().total_cmp(&(b[0] - t).abs()))
            && (points[idx][0] - t).abs() < 0.08
        {
            points[idx][1] = v;
        } else if points.len() < 16 {
            points.push([t, v]);
            points.sort_by(|a, b| a[0].total_cmp(&b[0]));
        }
        if let Ok(serialized) = serde_json::to_string(&points) {
            response
                .property_edits
                .push((format!("brk_{param_key}"), serialized));
        }
    }

    resp.on_hover_text("Click or drag to edit breakfile automation curve");
}

/// Renders thumbnail preview + custom controls for Image nodes.
pub(super) fn render_image_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    if let Some(tel) = telemetry
        && !tel.thumbnail_png.is_empty()
    {
        render_thumbnail_preview(ui, node.id, &tel.thumbnail_png, zoom);
        ui.add_space(4.0);
    }

    match node.kind {
        NodeKind::ImageRotate => {
            let mut deg = node
                .props
                .get("degrees")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(90.0);
            ui.horizontal(|ui| {
                for preset in [90_i32, 180, 270] {
                    if ui.small_button(format!("{preset}°")).clicked() {
                        response
                            .property_edits
                            .push(("degrees".into(), preset.to_string()));
                    }
                }
                if ui
                    .add(egui::Slider::new(&mut deg, -360.0..=360.0).suffix("°"))
                    .changed()
                {
                    response
                        .property_edits
                        .push(("degrees".into(), format!("{:.0}", deg.round())));
                }
            });
        }
        NodeKind::ImageConvertWebp => {
            let mut q = node
                .props
                .get("quality")
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(85);
            if ui
                .add(egui::Slider::new(&mut q, 1..=100).text("Quality"))
                .changed()
            {
                response
                    .property_edits
                    .push(("quality".into(), q.to_string()));
            }
        }
        NodeKind::ImageResize => {
            ui.horizontal(|ui| {
                for (label, w, h) in [
                    ("720p", "1280", "720"),
                    ("1080p", "1920", "1080"),
                    ("4K", "3840", "2160"),
                ] {
                    if ui.small_button(label).clicked() {
                        response.property_edits.push(("max_width".into(), w.into()));
                        response
                            .property_edits
                            .push(("max_height".into(), h.into()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        NodeKind::ImageCrop => {
            let geom = node
                .props
                .get("geometry")
                .map(String::as_str)
                .unwrap_or("1000x1000+0+0");
            let (wh, xy) = geom.split_once('+').unwrap_or((geom, "0+0"));
            let (w_s, h_s) = wh.split_once('x').unwrap_or(("1000", "1000"));
            let (x_s, y_s) = xy.split_once('+').unwrap_or(("0", "0"));
            let x = x_s.trim().parse::<f32>().unwrap_or(0.0);
            let y = y_s.trim().parse::<f32>().unwrap_or(0.0);
            let w = w_s.trim().parse::<f32>().unwrap_or(1000.0).max(1.0);
            let h = h_s.trim().parse::<f32>().unwrap_or(1000.0).max(1.0);
            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 44.0);
            let nx = (x / 1920.0).clamp(0.0, 0.75);
            let ny = (y / 1080.0).clamp(0.0, 0.75);
            let nw = (w / 1920.0).clamp(0.15, 1.0 - nx);
            let nh = (h / 1080.0).clamp(0.15, 1.0 - ny);
            let crop_rect = Rect::from_min_size(
                Pos2::new(
                    rect.min.x + nx * rect.width(),
                    rect.min.y + ny * rect.height(),
                ),
                Vec2::new(nw * rect.width(), nh * rect.height()),
            );
            painter.rect_filled(
                crop_rect,
                2.0,
                Color32::from_rgba_unmultiplied(56, 189, 248, 45),
            );
            painter.rect_stroke(
                crop_rect,
                2.0,
                Stroke::new(1.3_f32, Color32::from_rgb(56, 189, 248)),
            );
            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let tx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 0.85);
                let ty = ((pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 0.85);
                let new_geom = format!("{w:.0}x{h:.0}+{:.0}+{:.0}", tx * 1920.0, ty * 1080.0);
                response.property_edits.push(("geometry".into(), new_geom));
            }
            ui.horizontal(|ui| {
                for (lbl, cw, ch) in [("1:1", 512, 512), ("16:9", 1280, 720), ("4:3", 800, 600)] {
                    if ui.small_button(lbl).clicked() {
                        response
                            .property_edits
                            .push(("geometry".into(), format!("{cw}x{ch}+{x:.0}+{y:.0}")));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        NodeKind::ImagePadFrame => {
            let pad = node
                .props
                .get("border_size")
                .and_then(|s| s.trim().parse::<f32>().ok())
                .unwrap_or(24.0)
                .clamp(0.0, 128.0);
            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 40.0);
            let inset = (pad / 128.0) * (rect.height().min(rect.width()) * 0.36);
            let inner = rect.shrink(inset.max(3.0));
            painter.rect_filled(inner, 2.0, Color32::from_rgb(45, 58, 80));
            painter.rect_stroke(
                inner,
                2.0,
                Stroke::new(1.2_f32, Color32::from_rgb(130, 195, 250)),
            );
            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let tx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
                response
                    .property_edits
                    .push(("border_size".into(), format!("{:.0}", tx * 128.0)));
            }
            ui.horizontal(|ui| {
                for px in [8_u32, 24, 48] {
                    if ui.small_button(format!("{px}px")).clicked() {
                        response
                            .property_edits
                            .push(("border_size".into(), px.to_string()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        NodeKind::ImageStitch => {
            let dir = node
                .props
                .get("direction")
                .map(String::as_str)
                .unwrap_or("horizontal");
            ui.horizontal(|ui| {
                for opt in ["horizontal", "vertical"] {
                    if ui.selectable_label(dir == opt, opt).clicked() {
                        response
                            .property_edits
                            .push(("direction".into(), opt.into()));
                    }
                }
            });
        }
        NodeKind::ImageStripExif => {
            let mut keep_orient = node
                .props
                .get("keep_orientation")
                .is_some_and(|v| v == "true");
            if ui
                .checkbox(&mut keep_orient, "Preserve orientation")
                .changed()
            {
                response.property_edits.push((
                    "keep_orientation".into(),
                    if keep_orient { "true" } else { "false" }.into(),
                ));
            }
        }
        NodeKind::ImageDiff => {
            let mut mode = node
                .props
                .get("highlight")
                .map(String::as_str)
                .unwrap_or("overlay");
            ui.horizontal(|ui| {
                for opt in ["overlay", "heatmap"] {
                    if ui.selectable_label(mode == opt, opt).clicked() {
                        mode = opt;
                        response
                            .property_edits
                            .push(("highlight".into(), opt.into()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        NodeKind::ImageAddTitle => {
            let pos = node
                .props
                .get("position")
                .map(String::as_str)
                .unwrap_or("bottom");
            ui.horizontal(|ui| {
                for opt in ["top", "bottom"] {
                    if ui.selectable_label(pos == opt, opt).clicked() {
                        response
                            .property_edits
                            .push(("position".into(), opt.into()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        _ => {
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
    }
}

/// Renders custom controls (e.g. Case / Base64 dropdown) + last-run output snippet for Text nodes.
pub(super) fn render_text_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    match node.kind {
        NodeKind::TextCase => {
            let current = node
                .props
                .get("mode")
                .map(String::as_str)
                .unwrap_or("snake_case");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("mode")
                        .color(hex_to_c32(&palette.colors.text_muted))
                        .small(),
                );
                egui::ComboBox::from_id_source((node.id, "text_case_mode"))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for mode in ["snake_case", "camelCase", "UPPER", "lower", "Title Case"] {
                            if ui.selectable_label(current == mode, mode).clicked() {
                                response.property_edits.push(("mode".into(), mode.into()));
                            }
                        }
                    });
            });
        }
        NodeKind::TextBase64 => {
            let current = node
                .props
                .get("mode")
                .map(String::as_str)
                .unwrap_or("encode");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("mode")
                        .color(hex_to_c32(&palette.colors.text_muted))
                        .small(),
                );
                egui::ComboBox::from_id_source((node.id, "text_b64_mode"))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for mode in ["encode", "decode"] {
                            if ui.selectable_label(current == mode, mode).clicked() {
                                response.property_edits.push(("mode".into(), mode.into()));
                            }
                        }
                    });
            });
        }
        NodeKind::TextTrim => {
            let mode = node.props.get("mode").map(String::as_str).unwrap_or("both");
            ui.horizontal(|ui| {
                for opt in ["both", "start", "end"] {
                    if ui.selectable_label(mode == opt, opt).clicked() {
                        response.property_edits.push(("mode".into(), opt.into()));
                    }
                }
            });
        }
        NodeKind::TextSlugify => {
            let sep = node
                .props
                .get("separator")
                .map(String::as_str)
                .unwrap_or("-");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("sep:")
                        .color(hex_to_c32(&palette.colors.text_muted))
                        .small(),
                );
                for opt in ["-", "_"] {
                    if ui.selectable_label(sep == opt, opt).clicked() {
                        response
                            .property_edits
                            .push(("separator".into(), opt.into()));
                    }
                }
            });
        }
        NodeKind::TextSplitLines | NodeKind::TextJoin => {
            let key = if node.kind == NodeKind::TextJoin {
                "separator"
            } else {
                "delimiter"
            };
            ui.horizontal(|ui| {
                for (lbl, val) in [("nl", "\\n"), ("comma", ", "), ("pipe", " | ")] {
                    if ui.small_button(lbl).clicked() {
                        response.property_edits.push((key.into(), val.into()));
                    }
                }
            });
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
        _ => {
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
    }

    render_text_telemetry_badge(ui, telemetry);
}

fn render_text_telemetry_badge(ui: &mut egui::Ui, telemetry: Option<&NodeTelemetry>) {
    if let Some(tel) = telemetry
        && let Some(ref snippet) = tel.text_preview
    {
        ui.add_space(3.0);
        egui::Frame::none()
            .fill(Color32::from_rgb(14, 17, 24))
            .rounding(3.0)
            .inner_margin(egui::Margin::symmetric(6.0, 3.0))
            .show(ui, |ui| {
                let short: String = snippet.chars().take(48).collect();
                ui.label(
                    egui::RichText::new(short)
                        .monospace()
                        .size(10.0)
                        .color(Color32::from_rgb(190, 210, 235)),
                );
            });
    }
}

/// Interactive controls + telemetry for Source nodes (`LeftHandSource`, `RightHandSource`, `SlotSource`, `Constant*`, `PromptOrHandText`).
pub(super) fn render_source_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    if let Some(tel) = telemetry
        && !tel.thumbnail_png.is_empty()
    {
        render_thumbnail_preview(ui, node.id, &tel.thumbnail_png, zoom);
        ui.add_space(3.0);
    }

    match &node.kind {
        NodeKind::ConstantNumber => {
            let mut val = node
                .props
                .get("value")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.0);
            if ui.add(egui::DragValue::new(&mut val).speed(0.5)).changed() {
                response
                    .property_edits
                    .push(("value".into(), format!("{val:.2}")));
            }
            ui.horizontal(|ui| {
                for (lbl, delta) in [("-1", -1.0), ("0", 0.0), ("+1", 1.0)] {
                    if ui.small_button(lbl).clicked() {
                        let next = if delta == 0.0 { 0.0 } else { val + delta };
                        response
                            .property_edits
                            .push(("value".into(), format!("{next:.2}")));
                    }
                }
            });
        }
        NodeKind::ConstantText | NodeKind::PromptOrHandText => {
            super::drawing::render_generic_properties(ui, node, palette, response);
            let key = if node.kind == NodeKind::ConstantText {
                "text"
            } else {
                "default_text"
            };
            let len = node.props.get(key).map(|s| s.len()).unwrap_or(0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{len} chars"))
                        .color(hex_to_c32(&palette.colors.text_muted))
                        .small(),
                );
                if ui.small_button("Clear").clicked() {
                    response.property_edits.push((key.into(), String::new()));
                }
            });
        }
        NodeKind::SlotSource(slot) => {
            let default_slot = match slot {
                little_clip_core::SlotId::LeftHand => "LeftHand",
                little_clip_core::SlotId::RightHand => "RightHand",
                little_clip_core::SlotId::Undo => "Undo",
                _ => "LeftPrior1",
            };
            let active = node
                .props
                .get("slot")
                .map(String::as_str)
                .unwrap_or(default_slot);
            ui.horizontal(|ui| {
                for (s, lbl) in [
                    ("LeftHand", "Left"),
                    ("RightHand", "Right"),
                    ("Undo", "Undo"),
                ] {
                    if ui.selectable_label(active == s, lbl).clicked() {
                        response.property_edits.push(("slot".into(), s.into()));
                    }
                }
            });
        }
        NodeKind::LeftHandSource | NodeKind::RightHandSource => {
            let hand_lbl = if matches!(node.kind, NodeKind::LeftHandSource) {
                "Left Hand Input"
            } else {
                "Right Hand Input"
            };
            let mut auto = node.props.get("auto_detect").is_none_or(|v| v != "false");
            if ui.checkbox(&mut auto, hand_lbl).changed() {
                response.property_edits.push((
                    "auto_detect".into(),
                    if auto { "true" } else { "false" }.into(),
                ));
            }
        }
        _ => {}
    }
    render_text_telemetry_badge(ui, telemetry);
}

/// Interactive controls + telemetry for File transform nodes (`FileSha256`, `FileZip`, `FileExtractName`).
pub(super) fn render_file_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    match node.kind {
        NodeKind::FileSha256 => {
            let upper = node.props.get("uppercase").is_some_and(|v| v == "true");
            ui.horizontal(|ui| {
                if ui.selectable_label(!upper, "lower hex").clicked() {
                    response
                        .property_edits
                        .push(("uppercase".into(), "false".into()));
                }
                if ui.selectable_label(upper, "UPPER HEX").clicked() {
                    response
                        .property_edits
                        .push(("uppercase".into(), "true".into()));
                }
            });
        }
        NodeKind::FileExtractName => {
            let mode = node
                .props
                .get("part")
                .map(String::as_str)
                .unwrap_or("filename");
            ui.horizontal(|ui| {
                for opt in ["filename", "stem", "ext"] {
                    if ui.selectable_label(mode == opt, opt).clicked() {
                        response.property_edits.push(("part".into(), opt.into()));
                    }
                }
            });
        }
        NodeKind::FileZip => {
            super::drawing::render_generic_properties(ui, node, palette, response);
            let mut level = node
                .props
                .get("level")
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(6);
            if ui
                .add(egui::Slider::new(&mut level, 1..=9).text("Level"))
                .changed()
            {
                response
                    .property_edits
                    .push(("level".into(), level.to_string()));
            }
        }
        _ => {
            super::drawing::render_generic_properties(ui, node, palette, response);
        }
    }
    render_text_telemetry_badge(ui, telemetry);
}

/// Interactive controls + telemetry for Sink nodes (`SetLeftHand`, `SetRightHand`, `SaveToDisk`, `PasteToApp`).
pub(super) fn render_sink_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    if let Some(tel) = telemetry
        && !tel.thumbnail_png.is_empty()
    {
        render_thumbnail_preview(ui, node.id, &tel.thumbnail_png, zoom);
        ui.add_space(3.0);
    }

    match node.kind {
        NodeKind::SaveToDisk => {
            super::drawing::render_generic_properties(ui, node, palette, response);
            let mut overwrite = node.props.get("overwrite").is_some_and(|v| v == "true");
            if ui.checkbox(&mut overwrite, "Overwrite existing").changed() {
                response.property_edits.push((
                    "overwrite".into(),
                    if overwrite { "true" } else { "false" }.into(),
                ));
            }
        }
        NodeKind::SetLeftHand | NodeKind::SetRightHand => {
            let mut notify = node.props.get("notify").is_none_or(|v| v != "false");
            let label = if matches!(node.kind, NodeKind::SetLeftHand) {
                "Update Left Hand"
            } else {
                "Update Right Hand"
            };
            if ui.checkbox(&mut notify, label).changed() {
                response.property_edits.push((
                    "notify".into(),
                    if notify { "true" } else { "false" }.into(),
                ));
            }
        }
        NodeKind::PasteToApp => {
            let mut focus_back = node.props.get("restore_focus").is_none_or(|v| v != "false");
            if ui
                .checkbox(&mut focus_back, "Auto-paste to active app")
                .changed()
            {
                response.property_edits.push((
                    "restore_focus".into(),
                    if focus_back { "true" } else { "false" }.into(),
                ));
            }
        }
        _ => {}
    }
    render_text_telemetry_badge(ui, telemetry);
}

/// Interactive quick-action footer + telemetry for `PythonScript` and `BashCommand` nodes.
pub(super) fn render_script_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    let lines = node
        .props
        .get("script")
        .map(|s| s.lines().count())
        .unwrap_or(0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{lines} lines"))
                .color(hex_to_c32(&palette.colors.text_muted))
                .small(),
        );
        let mut enabled = node.props.get("bypass").is_none_or(|v| v != "true");
        if ui.checkbox(&mut enabled, "Active").changed() {
            response.property_edits.push((
                "bypass".into(),
                if enabled { "false" } else { "true" }.into(),
            ));
        }
    });
    render_text_telemetry_badge(ui, telemetry);
}

/// Interactive UI for generic Automation nodes (`AutomationLfo`, `AutomationEnvelope`, `AutomationDraw`, `AutomationSteps`).
pub(super) fn render_automation_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    zoom: f32,
    _palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    let teal = Color32::from_rgb(45, 212, 191);
    match node.kind {
        NodeKind::AutomationLfo => {
            let shape = node
                .props
                .get("shape")
                .map(String::as_str)
                .unwrap_or("sine");
            let rate = node
                .props
                .get("rate_hz")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(2.0)
                .clamp(0.1, 20.0);
            let depth = node
                .props
                .get("depth")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            let offset = node
                .props
                .get("offset")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.5)
                .clamp(0.0, 1.0);

            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 66.0);
            let steps = 56;
            let mut pts = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let t = i as f64 / steps as f64;
                let phase = (t * rate) % 1.0;
                let raw = match shape {
                    "triangle" => 1.0 - 4.0 * (phase - 0.5).abs(),
                    "square" => {
                        if phase < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    "saw" => 2.0 * phase - 1.0,
                    _ => (phase * std::f64::consts::TAU).sin(),
                };
                let v = (offset + 0.5 * depth * raw).clamp(0.0, 1.0) as f32;
                pts.push(Pos2::new(
                    rect.min.x + t as f32 * rect.width(),
                    rect.max.y - v * rect.height(),
                ));
            }
            painter.add(egui::Shape::line(pts, Stroke::new(1.7_f32, teal)));
            let dot = Pos2::new(
                rect.min.x + ((rate - 0.1) / 19.9).clamp(0.0, 1.0) as f32 * rect.width(),
                rect.max.y - depth as f32 * rect.height(),
            );
            painter.circle_filled(dot, 4.2 * zoom.clamp(0.7, 1.3), Color32::WHITE);

            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
                let ny =
                    (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
                response
                    .property_edits
                    .push(("rate_hz".into(), format!("{:.2}", 0.1 + nx * 19.9)));
                response
                    .property_edits
                    .push(("depth".into(), format!("{ny:.2}")));
            }
            resp.on_hover_text(format!(
                "LFO ({shape}): Rate {rate:.1} Hz, Depth {depth:.2}\nDrag X = Rate, Y = Depth"
            ));

            ui.horizontal(|ui| {
                for (opt, lbl) in [
                    ("sine", "Sin"),
                    ("triangle", "Tri"),
                    ("square", "Sq"),
                    ("saw", "Saw"),
                ] {
                    if ui.selectable_label(shape == opt, lbl).clicked() {
                        response.property_edits.push(("shape".into(), opt.into()));
                    }
                }
            });
        }
        NodeKind::AutomationEnvelope => {
            let a = node
                .props
                .get("attack")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.15)
                .clamp(0.01, 0.9);
            let d = node
                .props
                .get("decay")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.25)
                .clamp(0.01, 0.9);
            let s = node
                .props
                .get("sustain")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.65)
                .clamp(0.0, 1.0);
            let r = node
                .props
                .get("release")
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.25)
                .clamp(0.01, 0.9);

            let total = (a + d + r).max(1.0);
            let t_a = (a / total).clamp(0.02, 0.8);
            let t_d = ((a + d) / total).clamp(t_a + 0.02, 0.9);
            let t_r = (1.0 - r / total).clamp(t_d, 0.98);
            let pts_norm = [[0.0, 0.0], [t_a, 1.0], [t_d, s], [t_r, s], [1.0, 0.0]];

            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 66.0);
            let screen_pts: Vec<Pos2> = pts_norm
                .iter()
                .map(|&[t, v]| {
                    Pos2::new(
                        rect.min.x + t as f32 * rect.width(),
                        rect.max.y - v as f32 * rect.height(),
                    )
                })
                .collect();
            painter.add(egui::Shape::line(
                screen_pts.clone(),
                Stroke::new(1.7_f32, teal),
            ));
            for &p in &screen_pts[1..4] {
                painter.circle_filled(p, 3.8 * zoom.clamp(0.7, 1.3), Color32::WHITE);
            }

            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.02, 0.98) as f64;
                let ny =
                    (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
                if (nx - t_a).abs() <= (nx - t_d).abs() && (nx - t_a).abs() <= (nx - t_r).abs() {
                    response
                        .property_edits
                        .push(("attack".into(), format!("{nx:.2}")));
                } else if (nx - t_d).abs() <= (nx - t_r).abs() {
                    let new_d = (nx - t_a).clamp(0.02, 0.85);
                    response
                        .property_edits
                        .push(("decay".into(), format!("{new_d:.2}")));
                    response
                        .property_edits
                        .push(("sustain".into(), format!("{ny:.2}")));
                } else {
                    let new_r = (1.0 - nx).clamp(0.02, 0.85);
                    response
                        .property_edits
                        .push(("release".into(), format!("{new_r:.2}")));
                    response
                        .property_edits
                        .push(("sustain".into(), format!("{ny:.2}")));
                }
            }
            resp.on_hover_text("ADSR Envelope: Drag Attack, Decay/Sustain, or Release handles");

            ui.horizontal(|ui| {
                for (lbl, pa, pd, ps, pr) in [
                    ("Pluck", "0.02", "0.25", "0.10", "0.20"),
                    ("Pad", "0.35", "0.30", "0.75", "0.40"),
                    ("Gate", "0.02", "0.05", "1.00", "0.05"),
                ] {
                    if ui.small_button(lbl).clicked() {
                        response.property_edits.push(("attack".into(), pa.into()));
                        response.property_edits.push(("decay".into(), pd.into()));
                        response.property_edits.push(("sustain".into(), ps.into()));
                        response.property_edits.push(("release".into(), pr.into()));
                    }
                }
            });
        }
        NodeKind::AutomationDraw => {
            let current = node
                .props
                .get("points")
                .map(String::as_str)
                .unwrap_or("[[0.0,0.2],[0.35,0.85],[0.7,0.4],[1.0,0.9]]");
            let mut pts: Vec<[f64; 2]> = serde_json::from_str(current)
                .unwrap_or_else(|_| vec![[0.0, 0.2], [0.5, 0.8], [1.0, 0.4]]);
            if pts.len() < 2 {
                pts = vec![[0.0, 0.2], [1.0, 0.8]];
            }
            pts.sort_by(|a, b| a[0].total_cmp(&b[0]));

            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 66.0);
            let screen_pts: Vec<Pos2> = pts
                .iter()
                .map(|&[t, v]| {
                    Pos2::new(
                        rect.min.x + t.clamp(0.0, 1.0) as f32 * rect.width(),
                        rect.max.y - v.clamp(0.0, 1.0) as f32 * rect.height(),
                    )
                })
                .collect();
            painter.add(egui::Shape::line(
                screen_pts.clone(),
                Stroke::new(1.7_f32, teal),
            ));
            for p in &screen_pts {
                painter.circle_filled(*p, 3.2, Color32::WHITE);
            }
            if (resp.clicked() || resp.dragged())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let t = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0) as f64;
                let v =
                    (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
                if let Some((idx, _)) = pts
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| (a[0] - t).abs().total_cmp(&(b[0] - t).abs()))
                    && (pts[idx][0] - t).abs() < 0.08
                {
                    pts[idx][1] = v;
                } else if pts.len() < 16 {
                    pts.push([t, v]);
                    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                }
                if let Ok(json) = serde_json::to_string(&pts) {
                    response.property_edits.push(("points".into(), json));
                }
            }
            resp.on_hover_text("Click or drag to draw automation breakpoints");

            ui.horizontal(|ui| {
                for (lbl, preset) in [
                    ("Up", "[[0.0,0.0],[1.0,1.0]]"),
                    ("Down", "[[0.0,1.0],[1.0,0.0]]"),
                    ("Arch", "[[0.0,0.0],[0.5,1.0],[1.0,0.0]]"),
                ] {
                    if ui.small_button(lbl).clicked() {
                        response
                            .property_edits
                            .push(("points".into(), preset.into()));
                    }
                }
            });
        }
        NodeKind::AutomationSteps => {
            let current = node
                .props
                .get("steps")
                .map(String::as_str)
                .unwrap_or("[0.2,0.8,0.4,0.9,0.1,0.6,0.5,0.7]");
            let mut vals: Vec<f64> = serde_json::from_str(current)
                .unwrap_or_else(|_| vec![0.2, 0.8, 0.4, 0.9, 0.1, 0.6, 0.5, 0.7]);
            if vals.is_empty() {
                vals = vec![0.5; 8];
            }

            let (rect, resp, painter) = allocate_dark_pad(ui, zoom, 66.0);
            let n = vals.len();
            let bar_w = rect.width() / n as f32;
            for (i, &v) in vals.iter().enumerate() {
                let h = v.clamp(0.02, 1.0) as f32 * (rect.height() - 4.0);
                let x0 = rect.min.x + i as f32 * bar_w + 1.5;
                let x1 = rect.min.x + (i + 1) as f32 * bar_w - 1.5;
                let bar = Rect::from_min_max(
                    Pos2::new(x0, rect.max.y - 2.0 - h),
                    Pos2::new(x1.max(x0 + 2.0), rect.max.y - 2.0),
                );
                painter.rect_filled(bar, 2.0, teal);
            }
            if (resp.clicked() || resp.dragged())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let nx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 0.999);
                let idx = ((nx * n as f32).floor() as usize).min(n - 1);
                let v =
                    (1.0 - (pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0) as f64;
                vals[idx] = v;
                if let Ok(json) = serde_json::to_string(&vals) {
                    response.property_edits.push(("steps".into(), json));
                }
            }
            resp.on_hover_text("Click or drag across bars to edit step sequence");

            ui.horizontal(|ui| {
                for (lbl, preset) in [
                    ("Ramp", "[0.12,0.25,0.37,0.5,0.62,0.75,0.87,1.0]"),
                    ("Alt", "[0.9,0.2,0.9,0.2,0.9,0.2,0.9,0.2]"),
                    ("Mid", "[0.5,0.5,0.5,0.5,0.5,0.5,0.5,0.5]"),
                ] {
                    if ui.small_button(lbl).clicked() {
                        response
                            .property_edits
                            .push(("steps".into(), preset.into()));
                    }
                }
            });
        }
        _ => {}
    }
}

/// Interactive summary + telemetry for fused chain nodes (`ImageMagickChain`, `AudioFilterChain`).
pub(super) fn render_chain_custom_ui(
    ui: &mut egui::Ui,
    node: &NodeInstance,
    telemetry: Option<&NodeTelemetry>,
    zoom: f32,
    palette: &AppPalette,
    response: &mut NodeCardResponse,
) {
    if let Some(tel) = telemetry
        && !tel.thumbnail_png.is_empty()
    {
        render_thumbnail_preview(ui, node.id, &tel.thumbnail_png, zoom);
        ui.add_space(3.0);
    }
    let mut bypass = node.props.get("bypass").is_some_and(|v| v == "true");
    if ui.checkbox(&mut bypass, "Bypass fused chain").changed() {
        response.property_edits.push((
            "bypass".into(),
            if bypass { "true" } else { "false" }.into(),
        ));
    }
    super::drawing::render_generic_properties(ui, node, palette, response);
    render_text_telemetry_badge(ui, telemetry);
}

fn render_thumbnail_preview(ui: &mut egui::Ui, node_id: Uuid, png_bytes: &[u8], zoom: f32) {
    let cache_id = egui::Id::new(("node_thumb_tex", node_id));
    let fingerprint = (
        png_bytes.len(),
        png_bytes.first().copied().unwrap_or(0),
        png_bytes.last().copied().unwrap_or(0),
    );
    let cached: Option<((usize, u8, u8), egui::TextureHandle)> =
        ui.ctx().data(|d| d.get_temp(cache_id));

    let tex = if let Some((fp, handle)) = cached
        && fp == fingerprint
    {
        Some(handle)
    } else if let Ok(img) = image::load_from_memory(png_bytes) {
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color_img = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        let handle = ui.ctx().load_texture(
            format!("node_thumb_{node_id}"),
            color_img,
            egui::TextureOptions::LINEAR,
        );
        ui.ctx()
            .data_mut(|d| d.insert_temp(cache_id, (fingerprint, handle.clone())));
        Some(handle)
    } else {
        None
    };

    if let Some(handle) = tex {
        let max_w = ui.available_width().min(150.0 * zoom).max(48.0);
        let max_h = (68.0 * zoom).clamp(32.0, 96.0);
        let sz = handle.size_vec2();
        let scale = (max_w / sz.x.max(1.0)).min(max_h / sz.y.max(1.0)).min(1.5);
        ui.centered_and_justified(|ui| {
            ui.image(egui::load::SizedTexture::new(handle.id(), sz * scale));
        });
    }
}

fn db_to_y(rect: Rect, db: f32) -> f32 {
    let norm = ((18.0 - db.clamp(-18.0, 18.0)) / 36.0).clamp(0.0, 1.0);
    rect.min.y + norm * rect.height()
}

fn draw_spectrum_grid(painter: &egui::Painter, rect: Rect, zoom: f32) {
    painter.rect_filled(rect, 4.0, Color32::from_rgb(10, 12, 16));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, Color32::from_rgb(48, 56, 72)),
    );

    let grid_col = Color32::from_rgba_unmultiplied(255, 255, 255, 24);
    let zero_col = Color32::from_rgba_unmultiplied(255, 255, 255, 58);

    // Log-frequency vertical gridlines
    for freq in [50.0_f32, 100.0, 500.0, 1000.0, 5000.0, 10000.0] {
        let t = ((freq / 20.0).log10() / 3.0).clamp(0.0, 1.0);
        let x = rect.min.x + t * rect.width();
        painter.line_segment(
            [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
            Stroke::new(1.0_f32, grid_col),
        );
    }

    // Horizontal dB gridlines
    for db in [-12.0_f32, -6.0, 0.0, 6.0, 12.0] {
        let y = db_to_y(rect, db);
        let col = if db == 0.0 { zero_col } else { grid_col };
        painter.line_segment(
            [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
            Stroke::new(1.0_f32, col),
        );
    }

    if zoom >= 0.75 {
        let font = egui::FontId::proportional(8.0 * zoom.clamp(0.75, 1.2));
        let lbl_col = Color32::from_rgb(130, 145, 165);
        for (freq, lbl) in [(100.0_f32, "100"), (1000.0, "1k"), (10000.0, "10k")] {
            let t = ((freq / 20.0).log10() / 3.0).clamp(0.0, 1.0);
            painter.text(
                Pos2::new(rect.min.x + t * rect.width(), rect.max.y - 2.0),
                egui::Align2::CENTER_BOTTOM,
                lbl,
                font.clone(),
                lbl_col,
            );
        }
    }
}

fn draw_hold_spectrum_traces(
    painter: &egui::Painter,
    rect: Rect,
    telemetry: &NodeTelemetry,
    zoom: f32,
) {
    let draw_trace = |bins: &[i8], color: Color32, width: f32| {
        if bins.len() < 2 {
            return;
        }
        let pts: Vec<Pos2> = bins
            .iter()
            .enumerate()
            .map(|(i, &db)| {
                let x = rect.min.x + (i as f32 / (bins.len() - 1) as f32) * rect.width();
                // Map -72..+18 dB across the graph height
                let norm = ((18.0 - (db as f32).clamp(-72.0, 18.0)) / 90.0).clamp(0.0, 1.0);
                let y = rect.min.y + norm * rect.height();
                Pos2::new(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(pts, Stroke::new(width, color)));
    };

    draw_trace(
        &telemetry.before_spectrum_db,
        Color32::from_rgba_unmultiplied(120, 140, 170, 140),
        1.1 * zoom.clamp(0.6, 1.3),
    );
    draw_trace(
        &telemetry.after_spectrum_db,
        Color32::from_rgba_unmultiplied(56, 189, 248, 210),
        1.4 * zoom.clamp(0.6, 1.3),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cdp_process_and_core_template_has_interactive_custom_ui() {
        for tpl in little_clip_ops::pipeline::NODE_CATALOG
            .iter()
            .chain(little_clip_ops::pipeline::all_cdp_templates().iter())
        {
            let mut node = NodeInstance {
                id: Uuid::new_v4(),
                kind: tpl.kind.clone(),
                title: tpl.name.to_string(),
                pos: [0.0, 0.0],
                props: Default::default(),
                inputs: vec![],
                outputs: vec![],
            };
            for (k, v) in tpl.default_props {
                node.props.insert((*k).to_string(), (*v).to_string());
            }
            assert!(
                extra_custom_ui_height(&node, None) > 0.0,
                "NodeKind {:?} ({}) should reserve interactive custom UI height",
                tpl.kind,
                tpl.name
            );
        }
    }
}
