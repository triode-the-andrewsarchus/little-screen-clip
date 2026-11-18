//! SVG and RGBA raster rendering for VCV Rack patch diagrams, strips, and miniature module panels.

use super::analyze::{Band, SignalType};
use super::catalog::{ModuleFunction, Role};
use super::layout::{
    ARROW_LENGTH, DiagramEdge, DiagramLayout, DiagramNode, ModulePanelSpec, PORT_TAB_HEIGHT,
};
use super::panels::{ResolvedPanel, resolve_panel};
use super::rack::{RACK_HP_WIDTH, RACK_ROW_HEIGHT, RackRailLayout, StripLineup};
use resvg::usvg::{TreeParsing, TreeTextToPath, TreeWriting};
use std::fmt::Write as _;
use std::sync::LazyLock;

/// Rendered RGBA pixel buffer returned to desktop/workbench callers.
#[derive(Clone, Debug)]
pub struct RenderedDiagram {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Theme colors for diagram rendering.
#[derive(Clone, Copy, Debug)]
pub struct DiagramTheme {
    pub dark: bool,
    pub bg: &'static str,
    pub panel: &'static str,
    pub panel_border: &'static str,
    pub band: &'static str,
    pub band_voice: &'static str,
    pub ink: &'static str,
    pub muted: &'static str,
    pub audio: &'static str,
    pub pitch: &'static str,
    pub cv: &'static str,
    pub gate: &'static str,
}

impl DiagramTheme {
    pub fn for_mode(dark: bool) -> Self {
        if dark {
            Self {
                dark: true,
                bg: "#16171a",
                panel: "#22242a",
                panel_border: "#3a3c44",
                band: "#1d1f24",
                band_voice: "#241f1f",
                ink: "#ececee",
                muted: "#9a9aa2",
                audio: "#ff6b6b",
                pitch: "#ffd452",
                cv: "#52e08a",
                gate: "#5cb8ff",
            }
        } else {
            Self {
                dark: false,
                bg: "#eef0f2",
                panel: "#ffffff",
                panel_border: "#cfd3d8",
                band: "#e3e6ea",
                band_voice: "#f1e9e6",
                ink: "#1b1d21",
                muted: "#60656d",
                audio: "#d64545",
                pitch: "#b97d00",
                cv: "#2c9457",
                gate: "#2f74c4",
            }
        }
    }

    pub fn signal_color(self, sig: SignalType) -> &'static str {
        match sig {
            SignalType::Audio => self.audio,
            SignalType::Pitch => self.pitch,
            SignalType::Cv => self.cv,
            SignalType::Gate => self.gate,
        }
    }

    pub fn role_color(self, role: Role) -> &'static str {
        if self.dark {
            match role {
                Role::Time => "#5cb8ff",
                Role::Pitch => "#ffd452",
                Role::Controller => "#52e08a",
                Role::Source => "#ff6b6b",
                Role::Modifier => "#ff9a52",
                Role::Mixer => "#b48cff",
                Role::Effect => "#ff7ad1",
                Role::Io => "#9a9aa2",
                Role::Performance => "#4fd1c5",
                Role::Monitor => "#6f6c66",
                Role::Router => "#52e08a",
            }
        } else {
            match role {
                Role::Time => "#2f74c4",
                Role::Pitch => "#b97d00",
                Role::Controller => "#2c9457",
                Role::Source => "#d64545",
                Role::Modifier => "#c2611f",
                Role::Mixer => "#7a52b3",
                Role::Effect => "#b0479a",
                Role::Io => "#6b6b70",
                Role::Performance => "#138a8a",
                Role::Monitor => "#9a958a",
                Role::Router => "#2c9457",
            }
        }
    }
}

/// Renders a diagram layout into a tightly bounded RGBA buffer at the requested scale.
pub fn render_layout_rgba(
    layout: &DiagramLayout,
    scale: f32,
    dark: bool,
) -> Result<RenderedDiagram, String> {
    let svg = render_layout_svg(layout, dark);
    rasterize_svg_rgba(&svg, scale)
}

/// Renders a diagram layout into text-outlined SVG paths for retained `femtovg` drawing.
pub fn render_layout_outlined_svg(layout: &DiagramLayout, dark: bool) -> Result<String, String> {
    let svg = render_layout_svg(layout, dark);
    outline_svg(&svg)
}

/// Renders the ranked module lineup strip (no boxes or cables) into RGBA pixels.
pub fn render_ranked_strip_rgba(
    lineup: &StripLineup,
    scale: f32,
    dark: bool,
) -> Result<RenderedDiagram, String> {
    let svg = render_ranked_strip_svg(lineup, dark);
    rasterize_svg_rgba(&svg, scale)
}

/// Renders the ranked module lineup strip into standalone SVG markup.
pub fn render_ranked_strip_svg(lineup: &StripLineup, dark: bool) -> String {
    let w = lineup.width.max(16.0);
    let h = lineup.height.max(16.0);
    let rail = if dark { "#262930" } else { "#8d94a0" };
    let mut svg = String::with_capacity(4096);
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.1}" height="{h:.1}" viewBox="0 0 {w:.1} {h:.1}" font-family="Arimo, sans-serif">"#
    );
    let _ = writeln!(
        svg,
        r#"<rect x="0" y="0" width="{w:.1}" height="4.5" fill="{rail}"/>"#
    );
    let _ = writeln!(
        svg,
        r#"<rect x="0" y="{:.1}" width="{w:.1}" height="4.5" fill="{rail}"/>"#,
        h - 4.5
    );

    for m in &lineup.modules {
        let resolved = resolve_panel(&m.plugin, &m.model);
        let spec = ModulePanelSpec {
            plugin: m.plugin.clone(),
            model: m.model.clone(),
            catalog_key: format!("{}/{}", m.plugin, m.model),
            title: m.title.clone(),
            role: m.role,
            function: m.function,
            hp_estimate: m.hp.min(255) as u8,
            knob_positions: vec![0.5],
            input_count: 2,
            output_count: 2,
            count: m.count,
        };
        append_single_panel_svg(
            &mut svg,
            &spec,
            resolved.as_ref(),
            m.x,
            m.y,
            m.width,
            m.height,
        );
        if m.count > 1 {
            let badge = format!("×{}", m.count);
            let bw = (badge.chars().count() as f64 * 11.0 + 10.0).max(26.0);
            let bh = 20.0;
            let bx = (m.x + m.width - bw - 2.0).max(m.x + 1.0);
            let by = (m.y + m.height - bh - 2.0).max(m.y + 1.0);
            let _ = writeln!(
                svg,
                r##"<rect x="{bx:.1}" y="{by:.1}" width="{bw:.1}" height="{bh:.1}" rx="4" fill="#181a20" fill-opacity="0.92" stroke="#ffd452" stroke-width="1.2"/>"##
            );
            let _ = writeln!(
                svg,
                r##"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="13" font-weight="bold" fill="#ffd452">{badge}</text>"##,
                bx + bw / 2.0,
                by + 14.5
            );
        }
    }

    svg.push_str("</svg>");
    svg
}

/// Renders the physical VCV Rack rail layout into RGBA pixels.
pub fn render_rack_rails_rgba(
    rack: &RackRailLayout,
    scale: f32,
    dark: bool,
) -> Result<RenderedDiagram, String> {
    let svg = render_rack_rails_svg(rack, dark);
    rasterize_svg_rgba(&svg, scale)
}

/// Renders all patch modules on brushed Eurorack rails at their actual patch positions.
pub fn render_rack_rails_svg(rack: &RackRailLayout, dark: bool) -> String {
    let w = rack.width.max(64.0);
    let h = rack.height.max(64.0);
    let case_bg = if dark { "#111216" } else { "#1f2228" };
    let recess_bg = "#16181e";
    let rail_fill = "#383c46";
    let rail_edge = "#525765";

    let mut svg = String::with_capacity(8192);
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.1}" height="{h:.1}" viewBox="0 0 {w:.1} {h:.1}" font-family="Arimo, sans-serif">"#
    );
    let _ = writeln!(
        svg,
        r#"<rect x="0" y="0" width="{w:.1}" height="{h:.1}" rx="6" fill="{case_bg}"/>"#
    );

    let rail_x = 12.0;
    let rail_w = (w - 24.0).max(20.0);
    for &(_, ry) in &rack.rows {
        let _ = writeln!(
            svg,
            r#"<rect x="{rail_x:.1}" y="{ry:.1}" width="{rail_w:.1}" height="{RACK_ROW_HEIGHT:.1}" fill="{recess_bg}"/>"#
        );
        // Top & bottom aluminum rails with subtle mounting screw tracks
        let _ = writeln!(
            svg,
            r#"<rect x="{rail_x:.1}" y="{ry:.1}" width="{rail_w:.1}" height="15" fill="{rail_fill}" stroke="{rail_edge}" stroke-width="0.8"/>"#
        );
        let bot_y = ry + RACK_ROW_HEIGHT - 15.0;
        let _ = writeln!(
            svg,
            r#"<rect x="{rail_x:.1}" y="{bot_y:.1}" width="{rail_w:.1}" height="15" fill="{rail_fill}" stroke="{rail_edge}" stroke-width="0.8"/>"#
        );
        let mut sx = rail_x + RACK_HP_WIDTH * 0.5;
        while sx < rail_x + rail_w - 6.0 {
            let _ = writeln!(
                svg,
                r##"<circle cx="{sx:.1}" cy="{:.1}" r="2.1" fill="#181a1f"/>"##,
                ry + 7.5
            );
            let _ = writeln!(
                svg,
                r##"<circle cx="{sx:.1}" cy="{:.1}" r="2.1" fill="#181a1f"/>"##,
                bot_y + 7.5
            );
            sx += RACK_HP_WIDTH * 2.0;
        }
    }

    for m in &rack.modules {
        let resolved = resolve_panel(&m.plugin, &m.model);
        let spec = ModulePanelSpec {
            plugin: m.plugin.clone(),
            model: m.model.clone(),
            catalog_key: format!("{}/{}", m.plugin, m.model),
            title: m.title.clone(),
            role: m.role,
            function: m.function,
            hp_estimate: m.hp.clamp(2, 255) as u8,
            knob_positions: vec![0.5],
            input_count: 2,
            output_count: 2,
            count: 1,
        };
        append_single_panel_svg(
            &mut svg,
            &spec,
            resolved.as_ref(),
            m.x,
            m.y,
            m.width,
            m.height,
        );
    }

    svg.push_str("</svg>");
    svg
}

fn outline_svg(svg: &str) -> Result<String, String> {
    let options = resvg::usvg::Options {
        font_family: "Arimo".into(),
        ..Default::default()
    };
    let mut tree = resvg::usvg::Tree::from_str(svg, &options)
        .map_err(|e| format!("VCV diagram SVG parse failed: {e}"))?;
    tree.convert_text(diagram_fonts());
    Ok(tree.to_string(&resvg::usvg::XmlOptions::default()))
}

/// Renders a `DiagramLayout` into standalone vector SVG markup.
pub fn render_layout_svg(layout: &DiagramLayout, dark: bool) -> String {
    let theme = DiagramTheme::for_mode(dark);
    let legend_h = if layout.is_strip { 0.0 } else { 24.0 };
    let total_h = (layout.height + legend_h).max(16.0);
    let total_w = layout.width.max(16.0);

    let mut svg = String::with_capacity(8192);
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{total_w:.1}" height="{total_h:.1}" viewBox="0 0 {total_w:.1} {total_h:.1}" font-family="Arimo, sans-serif">"#
    );

    if !layout.is_strip {
        let _ = writeln!(
            svg,
            r#"<rect x="0" y="0" width="{total_w:.1}" height="{total_h:.1}" rx="6" fill="{}"/>"#,
            theme.bg
        );
    }

    // Horizontal/vertical band backgrounds
    for band in &layout.bands {
        let fill = if band.band == Band::Voice {
            theme.band_voice
        } else {
            theme.band
        };
        let bx = band.x.unwrap_or(8.0);
        let bw = band.width.unwrap_or((total_w - 16.0).max(20.0));
        let stroke = if band.group.is_some() {
            format!(
                r#"stroke="{}" stroke-width="1" stroke-dasharray="5 4""#,
                theme.panel_border
            )
        } else {
            r#"stroke="none""#.to_string()
        };
        let _ = writeln!(
            svg,
            r#"<rect x="{bx:.1}" y="{:.1}" width="{bw:.1}" height="{:.1}" rx="6" fill="{fill}" {stroke}/>"#,
            band.y, band.height
        );
        let title_upper = escape_xml(&band.title.to_ascii_uppercase());
        let _ = writeln!(
            svg,
            r#"<text x="{:.1}" y="{:.1}" font-size="10.5" font-weight="bold" fill="{}" letter-spacing="0.5">{title_upper}</text>"#,
            bx + 10.0,
            band.y + 17.0,
            theme.muted
        );
    }

    // Orthogonal signal cables & arrowheads (drawn under nodes or neatly meeting port edges)
    for edge in &layout.edges {
        append_edge_svg(&mut svg, edge, theme, layout.is_strip);
    }

    // Nodes (boxes + miniature Eurorack panels + port tabs + eye watcher badge)
    for node in &layout.nodes {
        append_node_svg(&mut svg, node, theme, layout.is_strip);
    }

    // Cable intent labels (hover diagram only, where spacing allows)
    if !layout.is_strip {
        for edge in &layout.edges {
            if edge.intent.is_empty() || edge.is_feedback {
                continue;
            }
            let label = edge.label();
            let lw = (label.chars().count() as f64 * 5.6) + 8.0;
            let lx = edge.label_x;
            let ly = edge.label_y - 4.0;
            let _ = writeln!(
                svg,
                r#"<rect x="{:.1}" y="{:.1}" width="{lw:.1}" height="13" rx="3" fill="{}" fill-opacity="0.88"/>"#,
                lx - lw / 2.0,
                ly - 10.0,
                theme.bg
            );
            let _ = writeln!(
                svg,
                r#"<text x="{lx:.1}" y="{ly:.1}" text-anchor="middle" font-size="10" fill="{}">{}</text>"#,
                theme.signal_color(edge.signal),
                escape_xml(&label)
            );
        }

        // Bottom signal color legend
        let ly = layout.height + 14.0;
        let items = [
            (SignalType::Audio, "Audio"),
            (SignalType::Pitch, "Pitch"),
            (SignalType::Cv, "CV / Mod"),
            (SignalType::Gate, "Gate / Clock"),
        ];
        let mut lx = 14.0;
        for (sig, text) in items {
            let color = theme.signal_color(sig);
            let _ = writeln!(
                svg,
                r#"<line x1="{lx:.1}" y1="{ly:.1}" x2="{:.1}" y2="{ly:.1}" stroke="{color}" stroke-width="2.4" stroke-linecap="round"/>"#,
                lx + 16.0
            );
            let _ = writeln!(
                svg,
                r#"<text x="{:.1}" y="{:.1}" font-size="10.5" fill="{}">{text}</text>"#,
                lx + 21.0,
                ly + 3.5,
                theme.muted
            );
            lx += 82.0 + (text.len() as f64 * 3.0);
        }
        let _ = writeln!(
            svg,
            r#"<text x="{:.1}" y="{:.1}" text-anchor="end" font-size="10" fill="{}">Scopes: badge</text>"#,
            total_w - 14.0,
            ly + 3.5,
            theme.muted
        );
    }

    svg.push_str("</svg>");
    svg
}

fn append_edge_svg(svg: &mut String, edge: &DiagramEdge, theme: DiagramTheme, is_strip: bool) {
    let color = theme.signal_color(edge.signal);
    let base_width = if is_strip {
        if edge.cable_ids.len() > 1 { 2.4 } else { 1.6 }
    } else if edge.cable_ids.len() > 1 {
        3.2
    } else {
        2.0
    };
    let dash = if edge.is_monitor {
        r#" stroke-dasharray="4 3""#
    } else if edge.is_feedback {
        r#" stroke-dasharray="6 3 2 3""#
    } else {
        ""
    };
    let opacity = if is_strip && edge.is_feedback {
        "0.75"
    } else {
        "0.95"
    };
    let _ = writeln!(
        svg,
        r#"<path d="{}" fill="none" stroke="{color}" stroke-width="{base_width}" stroke-linecap="round" stroke-linejoin="round" opacity="{opacity}"{dash}/>"#,
        edge.path
    );

    // Arrowhead
    if edge.end_dir.abs() > 0.1 {
        let len = if is_strip { 5.5 } else { ARROW_LENGTH * 0.72 };
        let half_w = if is_strip { 2.8 } else { 3.6 };
        let tip_x = edge.end_x + edge.end_dir * len;
        let tip_y = edge.end_y;
        let bx = edge.end_x;
        let _ = writeln!(
            svg,
            r#"<polygon points="{tip_x:.1},{tip_y:.1} {bx:.1},{:.1} {bx:.1},{:.1}" fill="{color}" opacity="{opacity}"/>"#,
            tip_y - half_w,
            tip_y + half_w
        );
    }
}

fn append_node_svg(svg: &mut String, node: &DiagramNode, theme: DiagramTheme, is_strip: bool) {
    let role_color = theme.role_color(node.role);

    // Folded stack shadow underneath the main box
    if node.is_folded {
        let _ = writeln!(
            svg,
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="5" fill="{}" stroke="{}" stroke-width="1"/>"#,
            node.x + 3.0,
            node.y + 3.0,
            node.width,
            node.height,
            theme.panel,
            theme.panel_border
        );
    }

    // Main box
    let border_color = if node.is_folded {
        role_color
    } else {
        theme.panel_border
    };
    let border_width = if node.is_folded { "1.4" } else { "1" };
    let dash = if node.is_insert {
        r#" stroke-dasharray="4 3""#
    } else {
        ""
    };
    let _ = writeln!(
        svg,
        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="5" fill="{}" stroke="{border_color}" stroke-width="{border_width}"{dash}/>"#,
        node.x, node.y, node.width, node.height, theme.panel
    );

    // Role accent bar on the left edge
    let _ = writeln!(
        svg,
        r#"<rect x="{:.1}" y="{:.1}" width="4" height="{:.1}" rx="2" fill="{role_color}"/>"#,
        node.x + 1.0,
        node.y + 1.0,
        (node.height - 2.0).max(4.0)
    );

    // Port tabs on edges (in hover layout)
    if !is_strip {
        for tab in &node.tabs {
            let color = theme.signal_color(tab.signal);
            let tx = if tab.output {
                node.x + node.width
            } else {
                node.x - tab.width
            };
            let ty = tab.y - PORT_TAB_HEIGHT / 2.0;
            let _ = writeln!(
                svg,
                r#"<rect x="{tx:.1}" y="{ty:.1}" width="{:.1}" height="{PORT_TAB_HEIGHT:.1}" rx="2.5" fill="{color}"/>"#,
                tab.width
            );
            let _ = writeln!(
                svg,
                r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="8.5" font-weight="bold" fill="{}">{}</text>"#,
                tx + tab.width / 2.0,
                ty + 9.2,
                theme.panel,
                escape_xml(&tab.text)
            );
        }
    }

    // Miniature Eurorack panel preview tray inside the left side of the box
    let tray_left = node.x + 8.0;
    let tray_top = node.y + 5.0;
    let tray_h = (node.height - 10.0).clamp(24.0, 42.0);
    let max_tray_w = if is_strip { 64.0 } else { 76.0 };
    let tray_used_w = append_panel_tray_svg(
        svg,
        &node.panels,
        tray_left,
        tray_top,
        tray_h,
        max_tray_w,
        theme.dark,
    );

    // Title & subtitle beside the panel preview tray
    let text_x = tray_left + tray_used_w + 7.0;
    let has_watch = !node.watchers.is_empty();
    let badge_reserve = if has_watch { 24.0 } else { 6.0 };
    let avail_text_w = (node.x + node.width - badge_reserve - text_x).max(32.0);

    let title_font = if is_strip { 11.2 } else { 12.0 };
    let max_title_chars = ((avail_text_w / (title_font * 0.56)).floor() as usize).max(6);
    let title_str = fit_chars(&node.title, max_title_chars);
    let title_y = node.y + (node.height * 0.40).min(21.0);
    let _ = writeln!(
        svg,
        r#"<text x="{text_x:.1}" y="{title_y:.1}" font-size="{title_font:.1}" font-weight="bold" fill="{}">{}</text>"#,
        theme.ink,
        escape_xml(&title_str)
    );

    let sub_font = if is_strip { 9.2 } else { 9.8 };
    let max_sub_chars = ((avail_text_w / (sub_font * 0.54)).floor() as usize).max(8);
    let sub_str = fit_chars(&node.subtitle, max_sub_chars);
    let sub_y = title_y + if is_strip { 13.0 } else { 14.0 };
    if sub_y + 4.0 < node.y + node.height {
        let _ = writeln!(
            svg,
            r#"<text x="{text_x:.1}" y="{sub_y:.1}" font-size="{sub_font:.1}" fill="{}">{}</text>"#,
            theme.muted,
            escape_xml(&sub_str)
        );
    }

    // Scope / display watcher badge ("MonitorView::Badge")
    if has_watch {
        append_watch_badge_svg(
            svg,
            node.x + node.width - 14.0,
            node.y + 11.0,
            node.watchers.len(),
            theme,
        );
    }
}

/// Draws 1..=4 miniature Eurorack module faceplates inside a node box's panel tray.
fn append_panel_tray_svg(
    svg: &mut String,
    panels: &[ModulePanelSpec],
    x: f64,
    y: f64,
    height: f64,
    max_width: f64,
    _dark: bool,
) -> f64 {
    if panels.is_empty() {
        return 0.0;
    }
    let max_count = if panels.len() > 1 { 3 } else { 1 };
    let shown: Vec<(&ModulePanelSpec, Option<ResolvedPanel>)> = panels
        .iter()
        .take(max_count)
        .map(|p| (p, resolve_panel(&p.plugin, &p.model)))
        .collect();
    let gap = 3.0;
    let total_gap = gap * (shown.len().saturating_sub(1)) as f64;

    let raw_widths: Vec<f64> = shown
        .iter()
        .map(|(panel, resolved)| {
            if let Some(real) = resolved {
                (height * real.aspect).clamp(11.0, max_width.min(48.0))
            } else if shown.len() == 1 {
                ((height * 0.56) * (panel.hp_estimate as f64 / 10.0).clamp(0.75, 1.35))
                    .clamp(16.0, max_width.min(28.0))
            } else {
                (height * 0.45).clamp(13.0, 21.0)
            }
        })
        .collect();
    let sum_w: f64 = raw_widths.iter().sum();
    let avail = (max_width - total_gap).max(12.0);
    let shrink = if sum_w > avail { avail / sum_w } else { 1.0 };

    let mut cur_x = x;
    for ((panel, resolved), rw) in shown.iter().zip(raw_widths) {
        let w = rw * shrink;
        append_single_panel_svg(svg, panel, resolved.as_ref(), cur_x, y, w, height);
        cur_x += w + gap;
    }
    if panels.len() > max_count {
        let rem = panels.len() - max_count;
        let _ = writeln!(
            svg,
            r##"<text x="{:.1}" y="{:.1}" font-size="8" font-weight="bold" fill="#9a9aa2">+{rem}</text>"##,
            cur_x + 1.0,
            y + height - 3.0
        );
        return (cur_x - x + 12.0).min(max_width + 12.0);
    }
    (cur_x - gap) - x
}

#[derive(Clone, Copy)]
struct PanelPalette {
    faceplate: &'static str,
    border: &'static str,
    header: &'static str,
    text: &'static str,
    accent: &'static str,
    dark_face: bool,
}

fn plugin_panel_palette(plugin: &str) -> PanelPalette {
    match plugin {
        "Core" | "Fundamental" => PanelPalette {
            faceplate: "#232730",
            border: "#4b5160",
            header: "#161920",
            text: "#e2e6ed",
            accent: "#f59e0b",
            dark_face: true,
        },
        "AudibleInstruments" => PanelPalette {
            faceplate: "#d6dbe2",
            border: "#8d95a0",
            header: "#c4cad2",
            text: "#22262d",
            accent: "#0d9488",
            dark_face: false,
        },
        "Befaco" => PanelPalette {
            faceplate: "#202126",
            border: "#4f4f58",
            header: "#131418",
            text: "#f0f0f2",
            accent: "#ef4444",
            dark_face: true,
        },
        "Bogaudio" => PanelPalette {
            faceplate: "#292c34",
            border: "#535866",
            header: "#1e2027",
            text: "#e5e9f0",
            accent: "#38bdf8",
            dark_face: true,
        },
        "MindMeldModular" => PanelPalette {
            faceplate: "#1c222e",
            border: "#445066",
            header: "#121720",
            text: "#dce3ee",
            accent: "#60a5fa",
            dark_face: true,
        },
        "SurgeXTRack" => PanelPalette {
            faceplate: "#1d222c",
            border: "#485164",
            header: "#e9741e",
            text: "#f8fafc",
            accent: "#fb923c",
            dark_face: true,
        },
        "Venom" => PanelPalette {
            faceplate: "#17181c",
            border: "#3f424a",
            header: "#101114",
            text: "#e4e4e7",
            accent: "#4ade80",
            dark_face: true,
        },
        "Cardinal" => PanelPalette {
            faceplate: "#1d1d22",
            border: "#5a323a",
            header: "#881337",
            text: "#f4f4f5",
            accent: "#fb7185",
            dark_face: true,
        },
        "Valley" => PanelPalette {
            faceplate: "#1e2028",
            border: "#484c5c",
            header: "#14161c",
            text: "#e6e8ef",
            accent: "#38bdf8",
            dark_face: true,
        },
        _ => {
            let hash: u32 = plugin
                .bytes()
                .fold(2166136261u32, |h, b| h.wrapping_mul(16777619) ^ b as u32);
            if hash.is_multiple_of(3) {
                PanelPalette {
                    faceplate: "#cfd4dc",
                    border: "#89909c",
                    header: "#333944",
                    text: "#f4f4f6",
                    accent: "#d97706",
                    dark_face: false,
                }
            } else {
                const ACCENTS: [&str; 4] = ["#38bdf8", "#a78bfa", "#34d399", "#fb7185"];
                PanelPalette {
                    faceplate: "#23252c",
                    border: "#494d58",
                    header: "#17191f",
                    text: "#e4e6eb",
                    accent: ACCENTS[(hash as usize) % ACCENTS.len()],
                    dark_face: true,
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn append_single_panel_svg(
    svg: &mut String,
    panel: &ModulePanelSpec,
    resolved: Option<&ResolvedPanel>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) {
    let pal = plugin_panel_palette(&panel.plugin);
    if let Some(real) = resolved {
        let _ = writeln!(
            svg,
            r#"<image x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" href="{}" preserveAspectRatio="xMidYMid meet"/>"#,
            real.data_uri
        );
        let _ = writeln!(
            svg,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="1" fill="none" stroke="{}" stroke-width="0.6"/>"#,
            pal.border
        );
        if panel.count > 1 {
            let _ = writeln!(
                svg,
                r#"<rect x="{:.1}" y="{:.1}" width="11" height="6.5" rx="1.5" fill="{}"/>"#,
                x + w - 10.5,
                y + h - 7.0,
                pal.accent
            );
            let _ = writeln!(
                svg,
                r##"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="5" font-weight="bold" fill="#111318">x{}</text>"##,
                x + w - 5.0,
                y + h - 2.0,
                panel.count
            );
        }
        return;
    }
    // Faceplate body
    let _ = writeln!(
        svg,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="1.5" fill="{}" stroke="{}" stroke-width="0.7"/>"#,
        pal.faceplate, pal.border
    );

    // Header band + top/bottom mounting screws
    let header_h = (h * 0.19).clamp(6.0, 9.0);
    let _ = writeln!(
        svg,
        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{header_h:.1}" rx="1" fill="{}"/>"#,
        x + 0.5,
        y + 0.5,
        w - 1.0,
        pal.header
    );
    let screw_fill = if pal.dark_face { "#8c939e" } else { "#767c87" };
    for &(sx, sy) in &[
        (x + 2.2, y + 2.2),
        (x + w - 2.2, y + 2.2),
        (x + 2.2, y + h - 2.2),
        (x + w - 2.2, y + h - 2.2),
    ] {
        let _ = writeln!(
            svg,
            r#"<circle cx="{sx:.1}" cy="{sy:.1}" r="1.0" fill="{screw_fill}"/>"#
        );
    }

    // Micro model label
    let abbr: String = panel
        .model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(if w >= 20.0 { 4 } else { 3 })
        .collect::<String>()
        .to_ascii_uppercase();
    if !abbr.is_empty() {
        let _ = writeln!(
            svg,
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="4.8" font-weight="bold" fill="{}">{}</text>"#,
            x + w / 2.0,
            y + header_h - 1.5,
            pal.text,
            escape_xml(&abbr)
        );
    }

    // Middle control region tailored by module function + real saved param positions
    let mid_y = y + header_h + 2.0;
    let mid_h = (h - header_h - 10.5).max(8.0);
    let cx = x + w / 2.0;
    let cy = mid_y + mid_h * 0.45;
    let knob_fill = "#181a20";
    let knob_ring = if pal.dark_face { "#9ca3af" } else { "#4b5563" };
    let val0 = panel.knob_positions.first().copied().unwrap_or(0.5) as f64;
    let val1 = panel.knob_positions.get(1).copied().unwrap_or(0.35) as f64;

    match panel.function {
        ModuleFunction::Oscillator | ModuleFunction::SoundSource => {
            // Big tuning knob + small secondary wave knob
            draw_knob_svg(
                svg,
                cx,
                cy - 1.0,
                (w * 0.25).clamp(3.0, 5.2),
                val0,
                pal.accent,
                knob_fill,
                knob_ring,
            );
            if mid_h >= 14.0 {
                draw_knob_svg(
                    svg,
                    cx - w * 0.2,
                    cy + mid_h * 0.32,
                    2.2,
                    val1,
                    pal.accent,
                    knob_fill,
                    knob_ring,
                );
                draw_knob_svg(
                    svg,
                    cx + w * 0.2,
                    cy + mid_h * 0.32,
                    2.2,
                    0.7,
                    pal.accent,
                    knob_fill,
                    knob_ring,
                );
            }
        }
        ModuleFunction::Filter | ModuleFunction::Waveshaper => {
            // Large cutoff pot + resonance pot + subtle LED dot
            draw_knob_svg(
                svg,
                cx,
                cy - 1.5,
                (w * 0.24).clamp(3.0, 4.8),
                val0,
                pal.accent,
                knob_fill,
                knob_ring,
            );
            if mid_h >= 13.0 {
                draw_knob_svg(
                    svg,
                    cx - w * 0.2,
                    cy + mid_h * 0.32,
                    2.1,
                    val1,
                    "#ffffff",
                    knob_fill,
                    knob_ring,
                );
                let _ = writeln!(
                    svg,
                    r#"<circle cx="{:.1}" cy="{:.1}" r="1.4" fill="{}"/>"#,
                    cx + w * 0.22,
                    cy + mid_h * 0.32,
                    pal.accent
                );
            }
        }
        ModuleFunction::Envelope => {
            // 4 vertical fader tracks with caps shaped like an ADSR envelope
            let count = 4;
            let span = (w - 5.0).max(6.0);
            for i in 0..count {
                let fx = x + 2.8 + (span * (i as f64 + 0.5) / count as f64);
                let val = panel.knob_positions.get(i).copied().unwrap_or(match i {
                    0 => 0.15,
                    1 => 0.6,
                    2 => 0.75,
                    _ => 0.4,
                }) as f64;
                let top_t = mid_y + 1.0;
                let bot_t = mid_y + mid_h - 1.5;
                let cap_y = bot_t - val * (bot_t - top_t);
                let _ = writeln!(
                    svg,
                    r##"<line x1="{fx:.1}" y1="{top_t:.1}" x2="{fx:.1}" y2="{bot_t:.1}" stroke="#525866" stroke-width="1"/>"##
                );
                let _ = writeln!(
                    svg,
                    r#"<rect x="{:.1}" y="{:.1}" width="2.4" height="1.8" rx="0.5" fill="{}"/>"#,
                    fx - 1.2,
                    cap_y - 0.9,
                    pal.accent
                );
            }
        }
        ModuleFunction::Mixer => {
            // Vertical mixer channel faders + tiny LED meter strip
            let channels = 3;
            let span = (w - 6.0).max(6.0);
            for i in 0..channels {
                let fx = x + 3.0 + (span * (i as f64 + 0.5) / channels as f64);
                let val = panel.knob_positions.get(i).copied().unwrap_or(0.65) as f64;
                let top_t = mid_y + 1.5;
                let bot_t = mid_y + mid_h - 1.5;
                let cap_y = bot_t - val * (bot_t - top_t);
                let _ = writeln!(
                    svg,
                    r##"<line x1="{fx:.1}" y1="{top_t:.1}" x2="{fx:.1}" y2="{bot_t:.1}" stroke="#525866" stroke-width="1"/>"##
                );
                let _ = writeln!(
                    svg,
                    r##"<rect x="{:.1}" y="{:.1}" width="2.8" height="2.0" fill="#e5e7eb"/>"##,
                    fx - 1.4,
                    cap_y - 1.0
                );
            }
            // LED meter bar
            let mx = x + w - 3.2;
            let _ = writeln!(
                svg,
                r##"<rect x="{mx:.1}" y="{:.1}" width="1.6" height="{:.1}" fill="#22c55e"/>"##,
                mid_y + 4.0,
                (mid_h - 6.0).max(3.0)
            );
        }
        ModuleFunction::Sequencer | ModuleFunction::Clock | ModuleFunction::Logic => {
            // 2x4 glowing step/clock LED matrix
            let cols = 4;
            let rows = 2;
            let cell_w = ((w - 5.0) / cols as f64).max(2.0);
            let cell_h = (mid_h / rows as f64).min(5.0);
            for r in 0..rows {
                for c in 0..cols {
                    let lx = x + 2.5 + (c as f64 + 0.5) * cell_w;
                    let ly = mid_y + 2.5 + (r as f64 + 0.5) * cell_h;
                    let on = (r * cols + c) % 3 == 0;
                    let fill = if on { pal.accent } else { "#3f4451" };
                    let _ = writeln!(
                        svg,
                        r#"<rect x="{:.1}" y="{:.1}" width="2.2" height="2.2" rx="0.5" fill="{fill}"/>"#,
                        lx - 1.1,
                        ly - 1.1
                    );
                }
            }
        }
        ModuleFunction::Scope => {
            // Phosphor oscilloscope screen + glowing waveform trace
            let sw = (w - 4.0).max(6.0);
            let sh = (mid_h - 2.0).max(6.0);
            let sx = x + 2.0;
            let sy = mid_y + 1.0;
            let _ = writeln!(
                svg,
                r##"<rect x="{sx:.1}" y="{sy:.1}" width="{sw:.1}" height="{sh:.1}" rx="1" fill="#091114" stroke="#334155" stroke-width="0.6"/>"##
            );
            let y_mid = sy + sh * 0.5;
            let _ = writeln!(
                svg,
                r##"<path d="M {:.1} {:.1} Q {:.1} {:.1} {:.1} {:.1} T {:.1} {:.1}" fill="none" stroke="#22d3ee" stroke-width="1.0"/>"##,
                sx + 1.0,
                y_mid,
                sx + sw * 0.28,
                sy + 1.5,
                sx + sw * 0.5,
                y_mid,
                sx + sw - 1.0,
                y_mid
            );
        }
        _ => {
            // Balanced 2-3 pot control grid
            let r = (w * 0.18).clamp(2.2, 3.8);
            draw_knob_svg(
                svg,
                cx - w * 0.2,
                mid_y + mid_h * 0.35,
                r,
                val0,
                pal.accent,
                knob_fill,
                knob_ring,
            );
            draw_knob_svg(
                svg,
                cx + w * 0.2,
                mid_y + mid_h * 0.35,
                r,
                val1,
                "#e5e7eb",
                knob_fill,
                knob_ring,
            );
            if mid_h >= 14.0 {
                draw_knob_svg(
                    svg,
                    cx,
                    mid_y + mid_h * 0.72,
                    r * 0.9,
                    0.5,
                    pal.accent,
                    knob_fill,
                    knob_ring,
                );
            }
        }
    }

    // Bottom jack patchbay (inputs = silver rings, outputs = black/green-ringed jacks)
    let jack_top = y + h - 9.0;
    let total_jacks = (panel.input_count + panel.output_count).clamp(2, 6);
    let in_jacks = panel.input_count.min(3);
    let out_jacks = panel
        .output_count
        .min(3)
        .max(usize::from(total_jacks > in_jacks));
    let jacks: Vec<bool> = std::iter::repeat_n(false, in_jacks)
        .chain(std::iter::repeat_n(true, out_jacks))
        .take(6)
        .collect();
    let cols = if jacks.len() > 3 {
        3
    } else {
        jacks.len().max(1)
    };
    let jack_span = (w - 4.0).max(5.0);
    for (idx, &is_out) in jacks.iter().enumerate() {
        let row = idx / cols;
        let col = idx % cols;
        let jx = x + 2.0 + (jack_span * (col as f64 + 0.5) / cols as f64);
        let jy = jack_top + row as f64 * 3.6;
        let ring = if is_out { pal.accent } else { "#cbd5e1" };
        let _ = writeln!(
            svg,
            r##"<circle cx="{jx:.1}" cy="{jy:.1}" r="1.4" fill="#090a0d" stroke="{ring}" stroke-width="0.7"/>"##
        );
    }

    // Repeat badge on folded identical voices (e.g. "x3")
    if panel.count > 1 {
        let _ = writeln!(
            svg,
            r#"<rect x="{:.1}" y="{:.1}" width="11" height="6.5" rx="1.5" fill="{}"/>"#,
            x + w - 10.5,
            y + h - 7.0,
            pal.accent
        );
        let _ = writeln!(
            svg,
            r##"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="5" font-weight="bold" fill="#111318">x{}</text>"##,
            x + w - 5.0,
            y + h - 2.0,
            panel.count
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_knob_svg(
    svg: &mut String,
    cx: f64,
    cy: f64,
    r: f64,
    value_01: f64,
    pointer_color: &str,
    fill: &str,
    ring: &str,
) {
    // Rack knobs sweep roughly -145° to +145° (from 7 o'clock to 5 o'clock)
    let angle = (-145.0 + value_01.clamp(0.0, 1.0) * 290.0 - 90.0).to_radians();
    let px = cx + r * 0.85 * angle.cos();
    let py = cy + r * 0.85 * angle.sin();
    let _ = writeln!(
        svg,
        r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{fill}" stroke="{ring}" stroke-width="0.6"/>"#
    );
    let _ = writeln!(
        svg,
        r#"<line x1="{cx:.1}" y1="{cy:.1}" x2="{px:.1}" y2="{py:.1}" stroke="{pointer_color}" stroke-width="0.9" stroke-linecap="round"/>"#
    );
}

fn append_watch_badge_svg(svg: &mut String, cx: f64, cy: f64, count: usize, theme: DiagramTheme) {
    // Crisp eye glyph pill for MonitorView::Badge
    let pill_w = if count > 1 { 22.0 } else { 15.0 };
    let px = cx - pill_w / 2.0;
    let py = cy - 6.0;
    let _ = writeln!(
        svg,
        r#"<rect x="{px:.1}" y="{py:.1}" width="{pill_w:.1}" height="12" rx="6" fill="{}" stroke="{}" stroke-width="0.8"/>"#,
        theme.bg, theme.panel_border
    );
    let eye_x = if count > 1 { px + 6.5 } else { cx };
    let eye_y = cy;
    let _ = writeln!(
        svg,
        r#"<path d="M {:.1} {eye_y:.1} Q {eye_x:.1} {:.1} {:.1} {eye_y:.1} Q {eye_x:.1} {:.1} {:.1} {eye_y:.1} Z" fill="none" stroke="{}" stroke-width="1.1"/>"#,
        eye_x - 4.2,
        eye_y - 4.0,
        eye_x + 4.2,
        eye_y + 4.0,
        eye_x - 4.2,
        theme.ink
    );
    let _ = writeln!(
        svg,
        r#"<circle cx="{eye_x:.1}" cy="{eye_y:.1}" r="1.4" fill="{}"/>"#,
        theme.pitch
    );
    if count > 1 {
        let _ = writeln!(
            svg,
            r#"<text x="{:.1}" y="{:.1}" font-size="8" font-weight="bold" fill="{}">{count}</text>"#,
            eye_x + 5.2,
            cy + 2.8,
            theme.ink
        );
    }
}

fn fit_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else if max_chars <= 2 {
        "…".to_string()
    } else {
        let prefix: String = text.chars().take(max_chars - 1).collect();
        format!("{}…", prefix.trim_end())
    }
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn rasterize_svg_rgba(svg: &str, scale: f32) -> Result<RenderedDiagram, String> {
    let options = resvg::usvg::Options {
        font_family: "Arimo".into(),
        ..Default::default()
    };
    let mut tree = resvg::usvg::Tree::from_str(svg, &options)
        .map_err(|e| format!("VCV diagram SVG parse failed: {e}"))?;
    tree.convert_text(diagram_fonts());
    let scale = scale.clamp(0.25, 4.0);
    let fit_scale = scale
        .min(4096.0 / tree.size.width().max(1.0))
        .min(4096.0 / tree.size.height().max(1.0));
    let width = (tree.size.width() * fit_scale).ceil().clamp(1.0, 4096.0) as u32;
    let height = (tree.size.height() * fit_scale).ceil().clamp(1.0, 4096.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "VCV diagram raster allocation failed".to_string())?;
    resvg::Tree::from_usvg(&tree).render(
        resvg::tiny_skia::Transform::from_scale(fit_scale, fit_scale),
        &mut pixmap.as_mut(),
    );
    Ok(RenderedDiagram {
        width,
        height,
        rgba: pixmap.take(),
    })
}

fn diagram_fonts() -> &'static resvg::usvg::fontdb::Database {
    static FONTS: LazyLock<resvg::usvg::fontdb::Database> = LazyLock::new(|| {
        let mut fonts = resvg::usvg::fontdb::Database::new();
        for bytes in crate::font_assets::GRAPHVIZ_FONT_BYTES {
            fonts.load_font_data(bytes.to_vec());
        }
        fonts.load_font_data(crate::font_assets::EMOJI_FONT_BYTES.to_vec());
        fonts.set_sans_serif_family("Arimo");
        fonts.set_serif_family("Tinos");
        fonts.set_monospace_family("Cousine");
        fonts
    });
    &FONTS
}
