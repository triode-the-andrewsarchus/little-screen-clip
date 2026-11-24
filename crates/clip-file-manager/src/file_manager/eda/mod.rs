//! EDA / PCB fabrication, Excellon drill, centroid, HPGL, and KiCad strips and viewers.

mod kicad;
mod spice;

use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    ui_appearance::WorkbenchColors,
};
use clip_ui::buttons::ButtonRole;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, TextureHandle, Vec2, pos2, vec2};
use little_clip_core::ImageThumbnail;
use little_clip_core::eda::{
    Document, EdaFormat, StripSummary,
    gerber::GerberDocument,
    hpgl::{HpglDocument, pen_rgb},
    kicad::PcbLayerBucket,
    pick_place::{PickAndPlaceDocument, PlacementSide},
    read_document,
};
use little_clip_core::palette::AppPalette;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};

pub(super) fn is_gerber_gbr_file(path: &Path) -> bool {
    little_clip_core::eda::is_gerber_gbr_path(path)
}

pub(super) fn is_spice_raw_file(path: &Path) -> bool {
    little_clip_core::eda::is_spice_raw_path(path)
}

pub(super) fn icon_for_format(format: EdaFormat) -> Icon {
    match format {
        EdaFormat::Gerber
        | EdaFormat::Excellon
        | EdaFormat::KicadPcb
        | EdaFormat::KicadFootprint
        | EdaFormat::GerberJob => Icon::Layers,
        EdaFormat::PickAndPlace => Icon::Grid,
        EdaFormat::Hpgl => Icon::Pencil,
        EdaFormat::KicadSchematic | EdaFormat::KicadSymbolLib => Icon::NodeTree,
        EdaFormat::KicadProject | EdaFormat::SpiceNetlist => Icon::Code,
        EdaFormat::SpiceRaw => Icon::Waves,
        EdaFormat::LtspicePlot => Icon::Zap,
    }
}

pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "EDA / PCB".into(), Icon::Layers);
    match read_document(path, token) {
        Ok(document) => {
            let summary = document.strip_summary();
            data.icon = icon_for_format(summary.format);
            data.detail = summary.detail.clone();
            if let Ok(image) = document.render_thumbnail([240, 96], token) {
                data.image = Some(image);
            }
            data.eda = Some(Arc::new(summary));
        }
        Err(error) => data.detail = error,
    }
    data
}

pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: Rect,
    entry: &Entry,
    columns: Option<super::ui::ListColumns>,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) {
    let fallback = EdaFormat::extension_hint(&entry.path).map_or("EDA / PCB", |f| f.label());
    let detail = ready.map_or(fallback, |r| r.data.detail.as_str());
    let rect = super::ui::information_strip(ui, row, entry, detail, columns, colors);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.recessed);

    let summary = ready.and_then(|r| r.data.eda.as_ref());
    let texture = ready.and_then(|r| r.texture.as_ref());
    paint_strip_contents(
        &painter,
        ui,
        rect,
        summary.map(Arc::as_ref),
        texture,
        colors,
    );
}

fn paint_strip_contents(
    painter: &egui::Painter,
    ui: &egui::Ui,
    rect: Rect,
    summary: Option<&StripSummary>,
    texture: Option<&TextureHandle>,
    colors: WorkbenchColors,
) {
    let thumb_w = (rect.height() * 1.65).clamp(48.0, (rect.width() * 0.42).max(48.0));
    let thumb_rect = Rect::from_min_size(
        rect.min + vec2(4.0, 4.0),
        vec2((thumb_w - 8.0).max(16.0), (rect.height() - 8.0).max(16.0)),
    );
    painter.rect_filled(thumb_rect, 2.0, Color32::from_rgb(18, 22, 26));
    painter.rect_stroke(
        thumb_rect,
        2.0,
        Stroke::new(1.0, colors.border),
        egui::StrokeKind::Inside,
    );

    if let Some(tex) = texture {
        let tex_size = tex.size_vec2();
        if tex_size.x > 0.0 && tex_size.y > 0.0 {
            let scale = (thumb_rect.width() / tex_size.x).min(thumb_rect.height() / tex_size.y);
            let drawn = Rect::from_center_size(thumb_rect.center(), tex_size * scale);
            painter.image(
                tex.id(),
                drawn,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    let text_left = thumb_rect.right() + 10.0;
    if let Some(summary) = summary
        && rect.right() - text_left > 60.0
    {
        let text_w = (rect.right() - text_left - 8.0).max(20.0);
        let headline_font = egui::TextStyle::Body.resolve(ui.style());
        let sub_font = egui::TextStyle::Small.resolve(ui.style());
        let headline_galley = painter.layout(
            summary.headline.clone(),
            headline_font,
            colors.text_primary,
            text_w,
        );
        painter.galley(
            pos2(text_left, rect.top() + 8.0),
            headline_galley,
            colors.text_primary,
        );
        let specs_galley = painter.layout(
            summary.specs.clone(),
            sub_font,
            colors.text_secondary,
            text_w,
        );
        painter.galley(
            pos2(text_left, rect.top() + 30.0),
            specs_galley,
            colors.text_secondary,
        );
    }
}

#[derive(Default)]
struct ViewerState {
    raster: Option<TextureHandle>,
    source_lines: Vec<std::ops::Range<usize>>,
    selected_line: Option<usize>,
    source_scroll: bool,
    selected_aperture: Option<i32>,
    selected_tool: Option<u32>,
    selected_placement: Option<usize>,
    side_filter: Option<PlacementSide>,
    hidden_pens: BTreeSet<u16>,
    show_rapid_moves: bool,
    show_vector_overlay: bool,
    pcb_layers: BTreeSet<PcbLayerBucket>,
    filter_query: String,
    selected_symbol: usize,
    selected_unit: i64,
    selected_plot: usize,
    hidden_traces: BTreeSet<usize>,
    show_phase: bool,
    zoom: f32,
    pan: Vec2,
}

impl ViewerState {
    fn new(source_lines: Vec<std::ops::Range<usize>>) -> Self {
        Self {
            source_lines,
            show_rapid_moves: true,
            pcb_layers: PcbLayerBucket::ALL.into_iter().collect(),
            selected_unit: 1,
            zoom: 1.0,
            ..Self::default()
        }
    }

    fn select_line(&mut self, line: usize) {
        self.selected_line = Some(line);
        self.source_scroll = true;
    }
}

pub(super) struct Viewer {
    doc: Document,
    state: ViewerState,
}

impl Viewer {
    pub fn load(path: &Path, cancelled: &AtomicBool) -> Result<Self, String> {
        let doc = read_document(path, cancelled)?;
        let source_lines = index_source_lines(source_text_for(&doc));
        let mut state = ViewerState::new(source_lines);
        if let Document::KicadSymbolLib(lib) = &doc
            && let Some(symbol) = lib.symbols.first()
        {
            state.selected_unit = symbol.units[0];
        }
        Ok(Self { doc, state })
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        let format = self.doc.format();
        let colors = WorkbenchColors::from_palette(palette);
        let mut reset_view = false;
        StudioShell::new(format.label(), icon_for_format(format), palette, close).render_header(
            ui,
            |ui| {
                if ui
                    .add(ButtonRole::Secondary.button("Fit view", palette))
                    .clicked()
                {
                    reset_view = true;
                }
            },
        );
        if reset_view {
            self.state.zoom = 1.0;
            self.state.pan = Vec2::ZERO;
        }

        let layout = StudioPanelLayout::default().with_left().allocate(ui);
        if let Some(left) = layout.left {
            StudioPanelLayout::show_region(ui, left, |ui| {
                render_left_panel(ui, &self.doc, &mut self.state, palette);
            });
        }

        StudioPanelLayout::show_region(ui, layout.center, |ui| {
            render_center_panel(ui, &self.doc, &mut self.state, colors);
        });
    }
}

fn render_left_panel(
    ui: &mut egui::Ui,
    doc: &Document,
    state: &mut ViewerState,
    palette: &AppPalette,
) {
    egui::ScrollArea::vertical()
        .id_salt("eda-details")
        .auto_shrink([false, false])
        .show(ui, |ui| match doc {
            Document::Gerber(gerber) => render_gerber_details(ui, gerber, state),
            Document::PickAndPlace(pos_doc) => render_pick_place_details(ui, pos_doc, state),
            Document::Hpgl(hpgl) => render_hpgl_details(ui, hpgl, state, palette),
            Document::KicadPcb(pcb) => kicad::render_pcb_details(ui, pcb, state),
            Document::KicadSchematic(sch) => kicad::render_schematic_details(ui, sch, state),
            Document::KicadFootprint(fp) => kicad::render_footprint_details(ui, fp),
            Document::KicadSymbolLib(lib) => kicad::render_symbol_lib_details(ui, lib, state),
            Document::GerberJob(job) => kicad::render_gbrjob_details(ui, job),
            Document::KicadProject(proj) => kicad::render_project_details(ui, proj),
            Document::SpiceNetlist(net) => spice::render_netlist_details(ui, net, state),
            Document::SpiceRaw(raw) => spice::render_raw_details(ui, raw, state),
            Document::LtspicePlot(plt) => spice::render_plt_details(ui, plt),
        });
}

fn text_only_heading(doc: &Document) -> Option<&'static str> {
    match doc {
        Document::KicadProject(_) => Some("Configuration JSON"),
        Document::SpiceNetlist(_) => Some("SPICE Netlist Deck"),
        _ => None,
    }
}

fn paint_canvas_document(
    ui: &mut egui::Ui,
    rect: Rect,
    response: &egui::Response,
    doc: &Document,
    state: &mut ViewerState,
    colors: WorkbenchColors,
) -> String {
    let mut raw_readout = None;
    let cursor_mm = match doc {
        Document::Gerber(gerber) => {
            if state.raster.is_none() {
                ensure_texture_from_result(
                    ui.ctx(),
                    "eda-gerber",
                    gerber.render_thumbnail([1024, 1024], &AtomicBool::new(false)),
                    state,
                );
            }
            paint_gerber_canvas(ui, rect, gerber, state, colors)
        }
        Document::PickAndPlace(pos_doc) => {
            paint_pick_place_canvas(ui, rect, response, pos_doc, state, colors)
        }
        Document::Hpgl(hpgl) => paint_hpgl_canvas(ui, rect, response, hpgl, state, colors),
        Document::KicadPcb(pcb) => kicad::paint_pcb_center(ui, rect, pcb, state, colors),
        Document::KicadSchematic(sch) => {
            kicad::paint_schematic_center(ui, rect, sch, state, colors)
        }
        Document::KicadFootprint(fp) => kicad::paint_footprint_center(ui, rect, fp, state, colors),
        Document::KicadSymbolLib(lib) => {
            kicad::paint_symbol_lib_center(ui, rect, lib, state, colors);
            None
        }
        Document::GerberJob(job) => {
            kicad::paint_gbrjob_center(ui, rect, job, state, colors);
            None
        }
        Document::SpiceRaw(raw) => {
            raw_readout = Some(spice::paint_raw_center(ui, rect, raw, state, colors));
            None
        }
        Document::LtspicePlot(plt) => {
            spice::paint_plt_center(ui, rect, plt, state, colors);
            None
        }
        Document::KicadProject(_) | Document::SpiceNetlist(_) => None,
    };

    let zoom_pct = (state.zoom * 100.0).round() as i32;
    raw_readout.unwrap_or_else(|| {
        cursor_mm
            .map(|[x, y]| {
                format!(
                    "Cursor {x:.3}, {y:.3} mm · {zoom_pct}% · drag to pan · scroll to zoom · double-click to fit"
                )
            })
            .unwrap_or_else(|| {
                format!("{zoom_pct}% · drag to pan · scroll to zoom · double-click to fit")
            })
    })
}

const EDA_MIN_ZOOM: f32 = 0.1;
const EDA_MAX_ZOOM: f32 = 64.0;

fn eda_pan_zoom_canvas(
    ui: &egui::Ui,
    rect: Rect,
    response: &egui::Response,
    zoom: &mut f32,
    pan: &mut Vec2,
) {
    let max_pan = rect.size() * (*zoom).max(1.0) * 8.0;
    if response.dragged() {
        *pan = (*pan + ui.input(|i| i.pointer.delta())).clamp(-max_pan, max_pan);
    }
    if response.double_clicked() {
        *zoom = 1.0;
        *pan = Vec2::ZERO;
        return;
    }
    if !response.hovered() {
        return;
    }

    let wheel_y = crate::ui_ext::wheel_delta_including_start(ui.ctx()).y;
    let pinch = ui.input(|i| i.zoom_delta());
    let scroll_y = if wheel_y != 0.0 || (pinch - 1.0).abs() > f32::EPSILON {
        wheel_y
    } else {
        ui.input(|i| i.smooth_scroll_delta.y)
    };
    let factor = (scroll_y * 0.004).exp() * pinch;
    if (factor - 1.0).abs() <= f32::EPSILON {
        return;
    }

    let prev_zoom = *zoom;
    let next_zoom = (prev_zoom * factor).clamp(EDA_MIN_ZOOM, EDA_MAX_ZOOM);
    let ratio = next_zoom / prev_zoom.max(f32::EPSILON);
    let anchor = ui
        .input(|i| i.pointer.hover_pos())
        .unwrap_or_else(|| rect.center())
        - rect.center();
    let clamped_pan = rect.size() * next_zoom.max(1.0) * 8.0;
    *pan = (anchor + (*pan - anchor) * ratio).clamp(-clamped_pan, clamped_pan);
    *zoom = next_zoom;
}

fn eda_fit_rect(rect: Rect) -> Rect {
    if rect.width() > 48.0 && rect.height() > 48.0 {
        rect.shrink(12.0)
    } else {
        rect
    }
}

fn render_center_panel(
    ui: &mut egui::Ui,
    doc: &Document,
    state: &mut ViewerState,
    colors: WorkbenchColors,
) {
    if let Some(heading) = text_only_heading(doc) {
        ui.strong(heading);
        ui.separator();
        render_source_rows(ui, source_text_for(doc), state);
        return;
    }

    let has_source = matches!(doc, Document::Hpgl(_) | Document::LtspicePlot(_))
        && !state.source_lines.is_empty();
    let canvas_h = if has_source {
        (ui.available_height() - 160.0).max(120.0)
    } else {
        (ui.available_height() - 26.0).max(120.0)
    };
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), canvas_h),
        egui::Sense::click_and_drag(),
    );
    eda_pan_zoom_canvas(ui, rect, &response, &mut state.zoom, &mut state.pan);

    let coord_text = paint_canvas_document(ui, rect, &response, doc, state, colors);
    ui.label(coord_text);

    if has_source {
        render_source_rows(ui, source_text_for(doc), state);
    }
}

fn ensure_texture_from_result(
    ctx: &egui::Context,
    name: &str,
    rendered: Result<ImageThumbnail, String>,
    state: &mut ViewerState,
) {
    if let Ok(image) = rendered {
        let color_image = egui::ColorImage::from_rgba_premultiplied(
            [image.width as usize, image.height as usize],
            &image.premultiplied_rgba,
        );
        state.raster = Some(ctx.load_texture(name, color_image, egui::TextureOptions::LINEAR));
    }
}

fn paint_raster_canvas(
    ui: &egui::Ui,
    rect: Rect,
    bounds: [f64; 4],
    y_down: bool,
    state: &ViewerState,
    _colors: WorkbenchColors,
) -> Option<[f64; 2]> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, Color32::from_rgb(18, 22, 26));
    if let Some(tex) = &state.raster {
        let tex_size = tex.size_vec2();
        if tex_size.x > 0.0 && tex_size.y > 0.0 {
            let scale = ((rect.width() - 32.0).max(1.0) / tex_size.x)
                .min((rect.height() - 32.0).max(1.0) / tex_size.y)
                * state.zoom;
            let drawn = Rect::from_center_size(rect.center() + state.pan, tex_size * scale);
            painter.image(
                tex.id(),
                drawn,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    let cursor_rect = if rect.width() > 40.0 && rect.height() > 40.0 {
        rect.shrink(4.0)
    } else {
        rect
    };
    cursor_coords(ui, cursor_rect, bounds, state.zoom, state.pan, y_down)
}

fn gerber_texture_draw_rect(board_rect: Rect, bounds: [f64; 4], tex_size: Vec2) -> Rect {
    let span_x = (bounds[2] - bounds[0]).abs();
    let span_y = (bounds[3] - bounds[1]).abs();
    let (span_x, span_y) = if span_x < 1e-6 && span_y < 1e-6 {
        (10.0, 10.0)
    } else {
        (span_x.max(0.01), span_y.max(0.01))
    };
    let pw = f64::from(tex_size.x).max(1.0);
    let ph = f64::from(tex_size.y).max(1.0);
    let pad = (pw.min(ph) * 0.08).clamp(3.0, 18.0);
    let tex_scale = ((pw - 2.0 * pad).max(1.0) / span_x).min((ph - 2.0 * pad).max(1.0) / span_y);
    let board_tex_w = (span_x * tex_scale).max(1.0);
    let board_tex_h = (span_y * tex_scale).max(1.0);
    let draw_w = board_rect.width().max(1.0) * (pw / board_tex_w) as f32;
    let draw_h = board_rect.height().max(1.0) * (ph / board_tex_h) as f32;
    Rect::from_center_size(board_rect.center(), vec2(draw_w, draw_h))
}

fn paint_gerber_canvas(
    ui: &mut egui::Ui,
    rect: Rect,
    gerber: &GerberDocument,
    state: &ViewerState,
    colors: WorkbenchColors,
) -> Option<[f64; 2]> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, Color32::from_rgb(18, 22, 26));

    let fit_rect = eda_fit_rect(rect);
    let bounds = gerber.bounds_mm;
    let lo = crate::ui_ext::fit_cartesian_point(
        fit_rect,
        bounds,
        [bounds[0], bounds[3]],
        state.zoom,
        state.pan,
    );
    let hi = crate::ui_ext::fit_cartesian_point(
        fit_rect,
        bounds,
        [bounds[2], bounds[1]],
        state.zoom,
        state.pan,
    );

    if let Some(tex) = &state.raster {
        let board_rect = Rect::from_min_max(lo, hi);
        let draw_rect = gerber_texture_draw_rect(board_rect, bounds, tex.size_vec2());
        painter.image(
            tex.id(),
            draw_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    if state.show_vector_overlay
        || state.selected_aperture.is_some()
        || state.selected_tool.is_some()
    {
        paint_gerber_overlays(&painter, fit_rect, gerber, state, colors);
    }

    paint_origin_crosshair(&painter, fit_rect, rect, bounds, state.zoom, state.pan);
    cursor_coords(ui, fit_rect, bounds, state.zoom, state.pan, false)
}

fn is_poly_selected(
    state: &ViewerState,
    poly: &little_clip_core::eda::gerber::VectorPolyline,
) -> bool {
    match (state.selected_tool, state.selected_aperture, poly.aperture) {
        (Some(t), _, Some(ap)) => i32::try_from(t).ok() == Some(ap),
        (_, Some(sel_d), Some(ap)) => sel_d == ap,
        _ => false,
    }
}

fn paint_gerber_overlays(
    painter: &egui::Painter,
    rect: Rect,
    gerber: &GerberDocument,
    state: &ViewerState,
    colors: WorkbenchColors,
) {
    let bounds = gerber.bounds_mm;
    for poly in &gerber.polylines {
        let selected_match = is_poly_selected(state, poly);
        if !state.show_vector_overlay && !selected_match {
            continue;
        }
        let color = if selected_match {
            colors.focus
        } else if poly.clear {
            Color32::from_rgba_unmultiplied(240, 92, 92, 160)
        } else {
            Color32::from_rgba_unmultiplied(220, 230, 240, 95)
        };
        let stroke = Stroke::new(if selected_match { 2.2 } else { 1.0 }, color);
        let screen_pts: Vec<Pos2> = poly
            .points
            .iter()
            .map(|p| crate::ui_ext::fit_cartesian_point(rect, bounds, *p, state.zoom, state.pan))
            .collect();
        for seg in screen_pts.windows(2) {
            painter.line_segment([seg[0], seg[1]], stroke);
        }
        if poly.closed && screen_pts.len() >= 3 {
            painter.line_segment([*screen_pts.last().unwrap(), screen_pts[0]], stroke);
        }
    }
}

fn paint_pick_place_canvas(
    ui: &mut egui::Ui,
    rect: Rect,
    response: &egui::Response,
    pos_doc: &PickAndPlaceDocument,
    state: &mut ViewerState,
    colors: WorkbenchColors,
) -> Option<[f64; 2]> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, Color32::from_rgb(18, 22, 26));
    let fit_rect = eda_fit_rect(rect);
    let bounds = pos_doc.bounds_mm;
    paint_origin_crosshair(&painter, fit_rect, rect, bounds, state.zoom, state.pan);

    let hover = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let mut best_hit: Option<(usize, f32)> = None;
    let show_labels = state.zoom >= 2.2 || pos_doc.placements.len() <= 36;

    for (idx, item) in pos_doc.placements.iter().enumerate() {
        if state.side_filter.is_some_and(|side| item.side != side) {
            continue;
        }
        let center = crate::ui_ext::fit_cartesian_point(
            fit_rect,
            bounds,
            [item.x_mm, item.y_mm],
            state.zoom,
            state.pan,
        );
        let [r, g, b] = item.side.rgb();
        let is_selected = state.selected_placement == Some(idx);
        let fill = Color32::from_rgba_unmultiplied(r, g, b, if is_selected { 245 } else { 190 });
        let size = (5.0 * state.zoom.sqrt()).clamp(4.0, 18.0);
        let rad = (-item.rotation_deg).to_radians() as f32;
        let (s, c) = rad.sin_cos();
        let hw = size * 0.7;
        let hh = size * 0.45;
        let poly: Vec<Pos2> = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]
            .into_iter()
            .map(|[lx, ly]| pos2(center.x + lx * c - ly * s, center.y + lx * s + ly * c))
            .collect();
        painter.add(egui::Shape::convex_polygon(
            poly,
            fill,
            Stroke::new(
                if is_selected { 2.0 } else { 1.0 },
                if is_selected {
                    colors.focus
                } else {
                    Color32::from_rgb(235, 240, 245)
                },
            ),
        ));
        let pin_tip = pos2(center.x + hw * 1.1 * c, center.y + hw * 1.1 * s);
        painter.line_segment([center, pin_tip], Stroke::new(1.5, Color32::WHITE));
        if show_labels && rect.contains(center) {
            painter.text(
                center + vec2(0.0, -hh - 3.0),
                Align2::CENTER_BOTTOM,
                &item.ref_des,
                FontId::proportional(10.5),
                colors.text_primary,
            );
        }
        if let Some(pointer) = hover {
            let d2 = pointer.distance_sq(center);
            if d2 < 144.0 && best_hit.is_none_or(|(_, best)| d2 < best) {
                best_hit = Some((idx, d2));
            }
        }
    }
    if response.clicked()
        && let Some((idx, _)) = best_hit
    {
        state.selected_placement = Some(idx);
    }
    cursor_coords(ui, fit_rect, bounds, state.zoom, state.pan, false)
}

fn paint_hpgl_canvas(
    ui: &mut egui::Ui,
    rect: Rect,
    response: &egui::Response,
    hpgl: &HpglDocument,
    state: &mut ViewerState,
    colors: WorkbenchColors,
) -> Option<[f64; 2]> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, Color32::from_rgb(20, 23, 28));
    let fit_rect = eda_fit_rect(rect);
    let bounds = hpgl.bounds_mm;
    paint_origin_crosshair(&painter, fit_rect, rect, bounds, state.zoom, state.pan);

    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let hit_line = paint_hpgl_segments(&painter, fit_rect, hpgl, state, pointer, colors);
    if state.zoom >= 1.4 {
        paint_hpgl_labels(&painter, fit_rect, hpgl, state);
    }
    if response.clicked()
        && let Some(line) = hit_line
    {
        state.select_line(line);
    }
    cursor_coords(ui, fit_rect, bounds, state.zoom, state.pan, false)
}

fn paint_hpgl_segments(
    painter: &egui::Painter,
    rect: Rect,
    hpgl: &HpglDocument,
    state: &ViewerState,
    pointer: Option<Pos2>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let bounds = hpgl.bounds_mm;
    let step = hpgl.segments.len().div_ceil(30_000).max(1);
    let mut nearest = 64.0_f32;
    let mut hit_line = None;

    for seg in hpgl.segments.iter().step_by(step) {
        let visible = if seg.pen_down {
            !state.hidden_pens.contains(&seg.pen)
        } else {
            state.show_rapid_moves
        };
        if !visible {
            continue;
        }
        let from =
            crate::ui_ext::fit_cartesian_point(rect, bounds, seg.from, state.zoom, state.pan);
        let to = crate::ui_ext::fit_cartesian_point(rect, bounds, seg.to, state.zoom, state.pan);
        let is_selected = state.selected_line == Some(seg.line);
        let color = if is_selected {
            colors.focus
        } else if !seg.pen_down {
            Color32::from_rgba_unmultiplied(128, 136, 148, 85)
        } else {
            let [r, g, b] = pen_rgb(seg.pen);
            Color32::from_rgb(r, g, b)
        };
        let width = if is_selected {
            2.6
        } else if seg.pen_down {
            1.6
        } else {
            0.8
        };
        painter.line_segment([from, to], Stroke::new(width, color));
        if let Some(pointer) = pointer {
            let (_, d2) = crate::ui_ext::segment_hit(pointer, from, to);
            if d2 < nearest {
                nearest = d2;
                hit_line = Some(seg.line);
            }
        }
    }
    hit_line
}

fn paint_hpgl_labels(
    painter: &egui::Painter,
    rect: Rect,
    hpgl: &HpglDocument,
    state: &ViewerState,
) {
    let bounds = hpgl.bounds_mm;
    for label in &hpgl.labels {
        if state.hidden_pens.contains(&label.pen) {
            continue;
        }
        let p = crate::ui_ext::fit_cartesian_point(rect, bounds, label.pos, state.zoom, state.pan);
        let [r, g, b] = pen_rgb(label.pen);
        painter.text(
            p,
            Align2::LEFT_BOTTOM,
            &label.text,
            FontId::proportional(11.0),
            Color32::from_rgb(r, g, b),
        );
    }
}

fn render_gerber_details(ui: &mut egui::Ui, gerber: &GerberDocument, state: &mut ViewerState) {
    let w = (gerber.bounds_mm[2] - gerber.bounds_mm[0]).max(0.0);
    let h = (gerber.bounds_mm[3] - gerber.bounds_mm[1]).max(0.0);
    ui.strong(&gerber.layer_type);
    ui.label(format!("Size: {w:.2} × {h:.2} mm"));
    ui.label(format!("Units: {}", gerber.units));
    ui.label(format!("Format: {}", gerber.coordinate_format));
    ui.label(format!("Polarity: {}", gerber.polarity));
    if let Some(cu) = &gerber.copper_thickness {
        ui.label(format!("Copper thickness: {cu}"));
    }
    ui.checkbox(&mut state.show_vector_overlay, "Show vector boundaries");

    ui.separator();
    if gerber.is_excellon {
        ui.strong(format!(
            "{} tools · {} holes · {} slots",
            gerber.tools.len(),
            gerber.hole_count,
            gerber.slot_count
        ));
        for tool in &gerber.tools {
            let selected = state.selected_tool == Some(tool.number);
            let label = format!(
                "T{}  Ø {:.3} mm  ({} hits{})",
                tool.number,
                tool.diameter_mm,
                tool.hit_count,
                if tool.slot_count > 0 {
                    format!(", {} slots", tool.slot_count)
                } else {
                    String::new()
                }
            );
            if ui.selectable_label(selected, label).clicked() {
                state.selected_tool = (!selected).then_some(tool.number);
            }
        }
    } else {
        ui.label(format!(
            "Flashes: {} · Draws: {} · Regions: {}",
            gerber.flash_count, gerber.stroke_count, gerber.region_count
        ));
        if !gerber.attributes.is_empty() {
            ui.separator();
            ui.strong("Gerber X2 Attributes");
            for (k, v) in &gerber.attributes {
                ui.label(format!("{k}: {v}"));
            }
        }
        if !gerber.apertures.is_empty() {
            ui.separator();
            ui.strong(format!("Apertures ({})", gerber.apertures.len()));
            for ap in &gerber.apertures {
                let selected = state.selected_aperture == Some(ap.d_code);
                let label = format!(
                    "D{} · {} ({}) · {}x",
                    ap.d_code,
                    ap.kind,
                    ap.dimensions,
                    ap.flash_count + ap.stroke_count
                );
                if ui.selectable_label(selected, label).clicked() {
                    state.selected_aperture = (!selected).then_some(ap.d_code);
                }
            }
        }
    }
    if !gerber.issues.is_empty() {
        ui.separator();
        ui.strong(format!("Notes ({})", gerber.issues.len()));
        for note in gerber.issues.iter().take(12) {
            ui.add(egui::Label::new(note).wrap());
        }
    }
}

fn render_pick_place_details(
    ui: &mut egui::Ui,
    pos_doc: &PickAndPlaceDocument,
    state: &mut ViewerState,
) {
    let w = (pos_doc.bounds_mm[2] - pos_doc.bounds_mm[0]).max(0.0);
    let h = (pos_doc.bounds_mm[3] - pos_doc.bounds_mm[1]).max(0.0);
    ui.strong(format!("{} Components", pos_doc.placements.len()));
    ui.label(format!("Size: {w:.2} × {h:.2} mm"));
    ui.label(format!("Units: {}", pos_doc.units));
    ui.label(format!(
        "Top: {} · Bottom: {} · {} packages",
        pos_doc.top_count, pos_doc.bottom_count, pos_doc.unique_packages
    ));
    ui.horizontal(|ui| {
        for (label, filter) in [
            ("All", None),
            ("Top", Some(PlacementSide::Top)),
            ("Bottom", Some(PlacementSide::Bottom)),
        ] {
            if ui
                .selectable_label(state.side_filter == filter, label)
                .clicked()
            {
                state.side_filter = filter;
            }
        }
    });
    if let Some(idx) = state.selected_placement
        && let Some(item) = pos_doc.placements.get(idx)
    {
        ui.separator();
        ui.strong(format!("Selected: {} ({})", item.ref_des, item.side_raw));
        ui.label(format!("Package: {}", item.package));
        ui.label(format!("Value: {}", item.value));
        ui.label(format!(
            "X {:.3} mm · Y {:.3} mm · {:.1}°",
            item.x_mm, item.y_mm, item.rotation_deg
        ));
    }
    ui.separator();
    for (idx, item) in pos_doc.placements.iter().enumerate() {
        if state.side_filter.is_some_and(|side| item.side != side) {
            continue;
        }
        let selected = state.selected_placement == Some(idx);
        let row = format!(
            "{} · {} · {} ({:.1}, {:.1}) {:.0}°",
            item.ref_des,
            item.side.label(),
            if item.package.is_empty() {
                &item.value
            } else {
                &item.package
            },
            item.x_mm,
            item.y_mm,
            item.rotation_deg
        );
        if ui.selectable_label(selected, row).clicked() {
            state.selected_placement = Some(idx);
        }
    }
}

fn render_hpgl_details(
    ui: &mut egui::Ui,
    hpgl: &HpglDocument,
    state: &mut ViewerState,
    _palette: &AppPalette,
) {
    let w = (hpgl.bounds_mm[2] - hpgl.bounds_mm[0]).max(0.0);
    let h = (hpgl.bounds_mm[3] - hpgl.bounds_mm[1]).max(0.0);
    ui.strong("HPGL Vector Plot");
    ui.label(format!("Size: {w:.2} × {h:.2} mm"));
    ui.label(format!(
        "Draws: {} · Moves: {}",
        hpgl.draw_count, hpgl.move_count
    ));
    ui.label(format!("Draw length: {:.1} mm", hpgl.draw_length_mm));
    ui.checkbox(&mut state.show_rapid_moves, "Show pen-up travel moves");

    if !hpgl.pens_used.is_empty() {
        ui.separator();
        ui.strong("Pens");
        for &pen in &hpgl.pens_used {
            let mut visible = !state.hidden_pens.contains(&pen);
            let [r, g, b] = pen_rgb(pen);
            ui.horizontal(|ui| {
                let (swatch, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(swatch, 2.0, Color32::from_rgb(r, g, b));
                if ui.checkbox(&mut visible, format!("Pen {pen}")).changed() {
                    if visible {
                        state.hidden_pens.remove(&pen);
                    } else {
                        state.hidden_pens.insert(pen);
                    }
                }
            });
        }
    }
    if !hpgl.labels.is_empty() {
        ui.separator();
        ui.strong(format!("Labels ({})", hpgl.labels.len()));
        for label in hpgl.labels.iter().take(64) {
            if ui
                .selectable_label(
                    state.selected_line == Some(label.line),
                    format!("L{}: {}", label.line, label.text),
                )
                .clicked()
            {
                state.select_line(label.line);
            }
        }
    }
}

fn render_source_rows(ui: &mut egui::Ui, source: &str, state: &mut ViewerState) {
    let row_height = crate::ui_ext::selectable_text_row_height(ui, 22.0);
    let selected = state.selected_line;
    let mut area = egui::ScrollArea::vertical();
    if std::mem::take(&mut state.source_scroll) {
        area = area.vertical_scroll_offset(
            selected.unwrap_or(1).saturating_sub(1) as f32
                * (row_height + ui.spacing().item_spacing.y),
        );
    }
    area.id_salt("eda-source")
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .show_rows(ui, row_height, state.source_lines.len(), |ui, rows| {
            for index in rows {
                let range = state.source_lines[index].clone();
                let line = &source[range];
                let capped = &line[..line.floor_char_boundary(220.min(line.len()))];
                if crate::ui_ext::selectable_text_row(
                    ui,
                    format!("{}  {capped}", index + 1),
                    selected == Some(index + 1),
                    row_height,
                )
                .clicked()
                {
                    state.select_line(index + 1);
                }
            }
        });
}

fn source_text_for(doc: &Document) -> &str {
    match doc {
        Document::Gerber(g) => &g.source_excerpt,
        Document::PickAndPlace(p) => &p.source_excerpt,
        Document::Hpgl(h) => &h.source_excerpt,
        Document::KicadPcb(p) => &p.source_excerpt,
        Document::KicadSchematic(s) => &s.source_excerpt,
        Document::KicadProject(p) => &p.source_excerpt,
        Document::KicadFootprint(f) => &f.source_excerpt,
        Document::KicadSymbolLib(l) => &l.source_excerpt,
        Document::GerberJob(j) => &j.source_excerpt,
        Document::SpiceNetlist(n) => &n.source_excerpt,
        Document::SpiceRaw(_) => "",
        Document::LtspicePlot(p) => &p.source_excerpt,
    }
}

fn index_source_lines(source: &str) -> Vec<std::ops::Range<usize>> {
    let mut offset = 0;
    source
        .split_inclusive('\n')
        .map(|line| {
            let range = offset..offset + line.trim_end_matches(['\r', '\n']).len();
            offset += line.len();
            range
        })
        .collect()
}

fn paint_origin_crosshair(
    painter: &egui::Painter,
    fit_rect: Rect,
    clip_rect: Rect,
    bounds: [f64; 4],
    zoom: f32,
    pan: Vec2,
) {
    let origin = crate::ui_ext::fit_cartesian_point(fit_rect, bounds, [0.0, 0.0], zoom, pan);
    if clip_rect.contains(origin) {
        let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 145, 175, 110));
        painter.line_segment([origin - vec2(8.0, 0.0), origin + vec2(8.0, 0.0)], stroke);
        painter.line_segment([origin - vec2(0.0, 8.0), origin + vec2(0.0, 8.0)], stroke);
    }
}

fn cursor_coords(
    ui: &egui::Ui,
    rect: Rect,
    bounds: [f64; 4],
    zoom: f32,
    pan: Vec2,
    y_down: bool,
) -> Option<[f64; 2]> {
    let pos = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten()?;
    let width = (bounds[2] - bounds[0]).max(0.00001);
    let height = (bounds[3] - bounds[1]).max(0.00001);
    let scale = ((rect.width() - 24.0).max(1.0) as f64 / width)
        .min((rect.height() - 24.0).max(1.0) as f64 / height)
        * f64::from(zoom);
    let delta = pos - rect.center() - pan;
    let x = (bounds[0] + bounds[2]) * 0.5 + f64::from(delta.x) / scale;
    let y_sign = if y_down { 1.0 } else { -1.0 };
    let y = (bounds[1] + bounds[3]) * 0.5 + y_sign * f64::from(delta.y) / scale;
    Some([x, y])
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::config::WorkbenchAppearance;

    fn render_single_frame(viewer: &mut Viewer, palette: &AppPalette) {
        let ctx = egui::Context::default();
        let mut close = false;
        let _ = clip_ui::test_support::run_frame(&ctx, egui::RawInput::default(), |ctx| {
            ctx.root_ui(|ui| {
                egui::CentralPanel::default().show(ui, |ui| viewer.ui(ui, &mut close, palette));
            });
        });
        assert!(!close);
    }

    #[test]
    fn eda_formats_load_strips_viewers_and_disambiguate_gbr_from_brushes() {
        let dir = tempfile::tempdir().unwrap();
        let gbr_path = dir.path().join("top-F_Cu.gbr");
        std::fs::write(
            &gbr_path,
            "%TF.FileFunction,Copper,L1,Top*%\n%MOMM*%\n%FSLAX46Y46*%\n%ADD10C,0.400000*%\nD10*\nX0Y0D02*\nX5000000Y5000000D01*\nM02*\n",
        )
        .unwrap();
        let drl_path = dir.path().join("drill.xln");
        std::fs::write(
            &drl_path,
            "M48\nMETRIC,TZ\nT1C0.800\n%\nT1\nX5.0Y5.0\nM30\n",
        )
        .unwrap();
        let pos_path = dir.path().join("parts.pos");
        std::fs::write(
            &pos_path,
            "# Ref Val Package PosX PosY Rot Side\nC1 100nF C_0603 5.0 5.0 90.0 top\n",
        )
        .unwrap();
        let hpgl_path = dir.path().join("cut.hpgl");
        std::fs::write(&hpgl_path, "IN;SP1;PU0,0;PD1000,0,1000,1000;SP0;").unwrap();

        let mut gimp = Vec::new();
        gimp.extend_from_slice(&34_u32.to_be_bytes());
        gimp.extend_from_slice(&2_u32.to_be_bytes());
        gimp.extend_from_slice(&4_u32.to_be_bytes());
        gimp.extend_from_slice(&4_u32.to_be_bytes());
        gimp.extend_from_slice(&1_u32.to_be_bytes());
        gimp.extend_from_slice(b"GIMP");
        gimp.extend_from_slice(&20_u32.to_be_bytes());
        gimp.extend_from_slice(b"Tip\0\0\0");
        gimp.extend_from_slice(&[240_u8; 16]);
        let brush_gbr = dir.path().join("brush.gbr");
        std::fs::write(&brush_gbr, gimp).unwrap();

        let cancel = AtomicBool::new(false);
        let gerber_strip = super::super::brush_strip::load(&gbr_path, &cancel);
        assert!(gerber_strip.eda.is_some());
        assert!(gerber_strip.image.is_some());
        let brush_strip = super::super::brush_strip::load(&brush_gbr, &cancel);
        assert!(brush_strip.brush.is_some());
        assert!(brush_strip.eda.is_none());

        let palette = crate::ui_appearance::palette(WorkbenchAppearance::Dark);
        for path in [&gbr_path, &drl_path, &pos_path, &hpgl_path] {
            let mut viewer = Viewer::load(path, &cancel).unwrap();
            render_single_frame(&mut viewer, &palette);
        }

        let proc_link = dir.path().join("bad.drl");
        std::os::unix::fs::symlink("/proc/uptime", &proc_link).unwrap();
        assert!(Viewer::load(&proc_link, &cancel).is_err());
        assert!(Viewer::load(&gbr_path, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn kicad_formats_load_strips_and_render_interactive_viewers() {
        let dir = tempfile::tempdir().unwrap();
        let pcb = dir.path().join("board.kicad_pcb");
        std::fs::write(
            &pcb,
            "(kicad_pcb (version 20240108) (layers (0 \"F.Cu\" signal) (44 \"Edge.Cuts\" user)) (net 1 \"VCC\") (gr_rect (start 0 0) (end 20 10) (layer \"Edge.Cuts\")) (segment (start 2 2) (end 12 2) (width 0.3) (layer \"F.Cu\") (net 1)))",
        )
        .unwrap();
        let sch = dir.path().join("sheet.kicad_sch");
        std::fs::write(
            &sch,
            "(kicad_sch (version 20231120) (paper \"A4\") (title_block (title \"Demo\")) (wire (pts (xy 10 10) (xy 30 10))) (label \"NET1\" (at 20 10 0)))",
        )
        .unwrap();
        let pro = dir.path().join("proj.kicad_pro");
        std::fs::write(
            &pro,
            r#"{"meta":{"filename":"proj.kicad_pro","version":1},"board":{"design_settings":{"rules":{"min_track_width":0.2}}},"net_settings":{"classes":[{"name":"Default","clearance":0.2,"track_width":0.25}]}}"#,
        )
        .unwrap();
        let fp = dir.path().join("part.kicad_mod");
        std::fs::write(
            &fp,
            "(footprint \"Test:Pad\" (layer \"F.Cu\") (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\")))",
        )
        .unwrap();
        let sym = dir.path().join("lib.kicad_sym");
        std::fs::write(
            &sym,
            "(kicad_symbol_lib (version 20231120) (symbol \"Res\" (property \"Reference\" \"R\") (symbol \"Res_0_1\" (rectangle (start -1 -2) (end 1 2)))))",
        )
        .unwrap();
        let gbrjob = dir.path().join("fab.gbrjob");
        std::fs::write(
            &gbrjob,
            r#"{"GeneralSpecs":{"Size":{"X":50,"Y":30},"LayerNumber":2,"Finish":"ENIG"},"MaterialStackup":[{"Type":"Copper","Name":"F.Cu","Thickness":0.035},{"Type":"Dielectric","Name":"Core","Material":"FR4","Thickness":1.5}]}"#,
        )
        .unwrap();

        let cancel = AtomicBool::new(false);
        let palette = crate::ui_appearance::palette(WorkbenchAppearance::Dark);
        for path in [&pcb, &sch, &pro, &fp, &sym, &gbrjob] {
            let strip = load(path, &cancel);
            assert!(
                strip.eda.is_some(),
                "missing summary for {}",
                path.display()
            );
            let mut viewer = Viewer::load(path, &cancel).unwrap();
            render_single_frame(&mut viewer, &palette);
        }
    }

    #[test]
    fn spice_ltspice_viewers_and_raw_plt_disambiguation() {
        let dir = tempfile::tempdir().unwrap();
        let cir = dir.path().join("filter.cir");
        std::fs::write(
            &cir,
            "* LP\nV1 in 0 1\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 2m\n.end\n",
        )
        .unwrap();
        let raw = dir.path().join("sim.raw");
        std::fs::write(
            &raw,
            "Title: Demo\nPlotname: Transient Analysis\nFlags: real\nNo. Variables: 2\nNo. Points: 2\nVariables:\n\t0\ttime\ttime\n\t1\tV(out)\tvoltage\nValues:\n 0\t0.0\n\t0.0\n 1\t1.0e-3\n\t3.3\n",
        )
        .unwrap();
        let lt_plt = dir.path().join("sim.plt");
        std::fs::write(
            &lt_plt,
            "[Transient Analysis]\n{\n   Npanes: 1\n   {\n      traces: 1 { (524290,0,\"V(out)\") }\n   }\n}\n",
        )
        .unwrap();
        let hpgl_plt = dir.path().join("plotter.plt");
        std::fs::write(&hpgl_plt, "IN;SP1;PU0,0;PD500,500;SP0;").unwrap();

        let camera_raw = dir.path().join("photo.raw");
        std::fs::write(&camera_raw, b"II*\0\x08\0\0\0fake tiff").unwrap();

        let cancel = AtomicBool::new(false);
        let listing = super::super::filesystem::read_directory(dir.path(), &cancel).unwrap();
        let revision_for = |target: &Path| {
            let canon = target.canonicalize().unwrap();
            listing
                .entries
                .iter()
                .find(|e| e.path == canon)
                .unwrap()
                .revision
        };
        let raw_strip = super::super::image_strip::load(&raw, revision_for(&raw), &cancel);
        assert!(raw_strip.eda.is_some());
        let cam_strip =
            super::super::image_strip::load(&camera_raw, revision_for(&camera_raw), &cancel);
        assert!(cam_strip.eda.is_none());
        assert!(is_spice_raw_file(&raw));
        assert!(!is_spice_raw_file(&camera_raw));

        let palette = crate::ui_appearance::palette(WorkbenchAppearance::Dark);
        for path in [&cir, &raw, &lt_plt, &hpgl_plt] {
            let strip = load(path, &cancel);
            assert!(strip.eda.is_some());
            let mut viewer = Viewer::load(path, &cancel).unwrap();
            render_single_frame(&mut viewer, &palette);
        }
    }

    #[test]
    fn narrow_canvas_fits_gerber_and_wheel_zooms_below_100_percent() {
        // Reproduce mobo-Edge_Cuts.gbr (149x95 mm) in a narrow half-screen canvas.
        let canvas = Rect::from_min_size(pos2(260.0, 60.0), vec2(480.0, 920.0));
        let fit = eda_fit_rect(canvas);
        let bounds = [66.6, -145.0, 215.6, -50.0];
        let lo = crate::ui_ext::fit_cartesian_point(
            fit,
            bounds,
            [bounds[0], bounds[3]],
            1.0,
            Vec2::ZERO,
        );
        let hi = crate::ui_ext::fit_cartesian_point(
            fit,
            bounds,
            [bounds[2], bounds[1]],
            1.0,
            Vec2::ZERO,
        );
        let board_rect = Rect::from_min_max(lo, hi);
        let draw_rect = gerber_texture_draw_rect(board_rect, bounds, vec2(1024.0, 653.0));
        assert!(canvas.contains_rect(board_rect));
        assert!(canvas.contains_rect(draw_rect));

        // Scroll out below 100% and back in above 100% in an interactive viewer frame.
        let dir = tempfile::tempdir().unwrap();
        let gbr = dir.path().join("wide-Edge_Cuts.gbr");
        std::fs::write(
            &gbr,
            "%MOMM*%\n%FSLAX46Y46*%\n%ADD10C,0.050000*%\nD10*\nX66600000Y-145000000D02*\nX215600000Y-50000000D01*\nM02*\n",
        )
        .unwrap();
        let mut viewer = Viewer::load(&gbr, &AtomicBool::new(false)).unwrap();
        let palette = crate::ui_appearance::palette(WorkbenchAppearance::Dark);
        let ctx = egui::Context::default();
        let pointer = pos2(520.0, 480.0);
        let run_events = |viewer: &mut Viewer, events: Vec<egui::Event>| {
            let mut close = false;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 1000.0))),
                events,
                ..Default::default()
            };
            let _ = clip_ui::test_support::run_frame(&ctx, raw, |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default()
                        .show(ui, |ui| viewer.ui(ui, &mut close, &palette));
                });
            });
            assert!(!close);
        };

        run_events(&mut viewer, vec![egui::Event::PointerMoved(pointer)]);
        run_events(
            &mut viewer,
            vec![
                egui::Event::PointerMoved(pointer),
                clip_ui::test_support::scroll(vec2(0.0, -180.0)),
            ],
        );
        assert!(
            viewer.state.zoom < 1.0 && viewer.state.zoom >= EDA_MIN_ZOOM,
            "expected zoom < 1.0 after zoom-out scroll, got {}",
            viewer.state.zoom
        );

        for _ in 0..3 {
            run_events(
                &mut viewer,
                vec![
                    egui::Event::PointerMoved(pointer),
                    clip_ui::test_support::scroll(vec2(0.0, 200.0)),
                ],
            );
        }
        assert!(
            viewer.state.zoom > 1.0 && viewer.state.zoom <= EDA_MAX_ZOOM,
            "expected zoom > 1.0 after zoom-in scroll, got {}",
            viewer.state.zoom
        );
    }
}
