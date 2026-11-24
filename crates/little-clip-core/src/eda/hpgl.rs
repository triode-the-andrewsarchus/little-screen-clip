//! HPGL / HP-GL2 (`.plt`, `.hpgl`) bounded parser, path geometry builder, and rasterizer.

use super::{
    allocate_canvas, bounded_source_text, check_cancelled, pixmap_to_thumbnail, update_bounds,
};
use crate::ImageThumbnail;
use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;
use tiny_skia::{Color, LineCap, LineJoin, Paint, PathBuilder, Stroke};

const MM_PER_PLOTTER_UNIT: f64 = 0.025;
const MAX_COMMANDS: usize = 400_000;
const MAX_SEGMENTS: usize = 300_000;
const BG_COLOR: [u8; 4] = [20, 23, 28, 255];

pub const PEN_PALETTE: [[u8; 3]; 8] = [
    [90, 204, 240],
    [244, 174, 68],
    [136, 224, 118],
    [232, 116, 172],
    [152, 166, 246],
    [238, 124, 96],
    [200, 148, 240],
    [228, 232, 238],
];

pub fn pen_rgb(pen: u16) -> [u8; 3] {
    let idx = pen.saturating_sub(1) as usize % PEN_PALETTE.len();
    PEN_PALETTE[idx]
}

#[derive(Clone, Copy, Debug)]
pub struct HpglSegment {
    pub line: usize,
    pub pen: u16,
    pub pen_down: bool,
    pub from: [f64; 2],
    pub to: [f64; 2],
}

#[derive(Clone, Debug)]
pub struct HpglLabel {
    pub line: usize,
    pub pen: u16,
    pub pos: [f64; 2],
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct HpglDocument {
    pub segments: Vec<HpglSegment>,
    pub labels: Vec<HpglLabel>,
    pub pens_used: Vec<u16>,
    pub command_count: usize,
    pub draw_count: usize,
    pub move_count: usize,
    pub draw_length_mm: f64,
    pub bounds_mm: [f64; 4],
    pub source_excerpt: String,
}

impl HpglDocument {
    /// Parses HPGL / HP-GL2 bytes into millimeter stroke segments and labels.
    pub fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancelled, "HPGL parse cancelled")?;
        if bytes.contains(&0) {
            return Err("HPGL file contains binary NUL bytes".into());
        }
        let text = String::from_utf8_lossy(bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

        let mut parser = Parser::new(text);
        parser.run(cancelled)?;
        if parser.segments.is_empty() && parser.labels.is_empty() {
            return Err("No plotter vector commands found in HPGL file".into());
        }

        let mut bounds = None;
        let mut draw_count = 0_usize;
        let mut move_count = 0_usize;
        let mut draw_length_mm = 0.0_f64;
        for seg in &parser.segments {
            if seg.pen_down {
                draw_count += 1;
                draw_length_mm +=
                    ((seg.to[0] - seg.from[0]).powi(2) + (seg.to[1] - seg.from[1]).powi(2)).sqrt();
                update_bounds(&mut bounds, seg.from[0], seg.from[1]);
                update_bounds(&mut bounds, seg.to[0], seg.to[1]);
            } else {
                move_count += 1;
            }
        }
        if bounds.is_none() {
            for seg in &parser.segments {
                update_bounds(&mut bounds, seg.from[0], seg.from[1]);
                update_bounds(&mut bounds, seg.to[0], seg.to[1]);
            }
            for label in &parser.labels {
                update_bounds(&mut bounds, label.pos[0], label.pos[1]);
            }
        }

        Ok(Self {
            segments: parser.segments,
            labels: parser.labels,
            pens_used: parser.pens_used.into_iter().collect(),
            command_count: parser.command_count,
            draw_count,
            move_count,
            draw_length_mm,
            bounds_mm: bounds.unwrap_or([0.0, 0.0, 10.0, 10.0]),
            source_excerpt: bounded_source_text(bytes),
        })
    }

    pub fn summary_line(&self) -> String {
        let w = (self.bounds_mm[2] - self.bounds_mm[0]).max(0.0);
        let h = (self.bounds_mm[3] - self.bounds_mm[1]).max(0.0);
        format!(
            "HPGL · {} pens · {} draws ({:.1} mm) · {:.1}×{:.1} mm",
            self.pens_used.len().max(1),
            self.draw_count,
            self.draw_length_mm,
            w,
            h
        )
    }

    pub fn render_thumbnail(
        &self,
        max_size: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled, "HPGL render cancelled")?;
        let bg = Color::from_rgba8(BG_COLOR[0], BG_COLOR[1], BG_COLOR[2], BG_COLOR[3]);
        let (mut image, transform, scale) = allocate_canvas(self.bounds_mm, max_size, bg)
            .ok_or_else(|| "Could not allocate HPGL preview".to_string())?;
        let stroke = Stroke {
            width: (1.25 / scale.max(0.01)).max(0.05),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };

        let mut current_pen = None;
        let mut pb = PathBuilder::new();
        let mut last_to: Option<[f64; 2]> = None;

        for (idx, seg) in self.segments.iter().enumerate() {
            if idx.is_multiple_of(1024) {
                check_cancelled(cancelled, "HPGL render cancelled")?;
            }
            if !seg.pen_down || seg.pen == 0 {
                continue;
            }
            if current_pen != Some(seg.pen) {
                flush_pen_path(&mut image, &mut pb, current_pen, &stroke, transform);
                current_pen = Some(seg.pen);
                last_to = None;
            }
            if last_to != Some(seg.from) {
                pb.move_to(seg.from[0] as f32, seg.from[1] as f32);
            }
            pb.line_to(seg.to[0] as f32, seg.to[1] as f32);
            last_to = Some(seg.to);
        }
        flush_pen_path(&mut image, &mut pb, current_pen, &stroke, transform);

        Ok(pixmap_to_thumbnail(image))
    }
}

fn flush_pen_path(
    image: &mut tiny_skia::Pixmap,
    pb: &mut PathBuilder,
    pen: Option<u16>,
    stroke: &Stroke,
    transform: tiny_skia::Transform,
) {
    let builder = std::mem::take(pb);
    let Some(pen) = pen else {
        return;
    };
    let Some(path) = builder.finish() else {
        return;
    };
    let [r, g, b] = pen_rgb(pen);
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 240);
    paint.anti_alias = true;
    image.stroke_path(&path, &paint, stroke, transform, None);
}

#[derive(Clone, Copy)]
struct Scaling {
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
    pen: u16,
    pen_down: bool,
    relative: bool,
    label_term: u8,
    cursor_u: [f64; 2],
    p1_u: [f64; 2],
    p2_u: [f64; 2],
    scaling: Option<Scaling>,
    polygon_mode: bool,
    segments: Vec<HpglSegment>,
    labels: Vec<HpglLabel>,
    pens_used: BTreeSet<u16>,
    command_count: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            src: text.as_bytes(),
            pos: 0,
            line: 1,
            pen: 1,
            pen_down: false,
            relative: false,
            label_term: 0x03,
            cursor_u: [0.0, 0.0],
            p1_u: [0.0, 0.0],
            p2_u: [10_000.0, 10_000.0],
            scaling: None,
            polygon_mode: false,
            segments: Vec::new(),
            labels: Vec::new(),
            pens_used: BTreeSet::new(),
            command_count: 0,
        }
    }

    fn run(&mut self, cancelled: &AtomicBool) -> Result<(), String> {
        while self.pos < self.src.len() {
            if self.command_count >= MAX_COMMANDS || self.segments.len() >= MAX_SEGMENTS {
                break;
            }
            if self.command_count.is_multiple_of(512) {
                check_cancelled(cancelled, "HPGL parse cancelled")?;
            }
            self.skip_separators();
            if self.pos >= self.src.len() {
                break;
            }
            // Skip escape sequences (e.g. `\x1b.(`).
            if self.src[self.pos] == 0x1b {
                self.advance_byte();
                while self.pos < self.src.len()
                    && !matches!(self.src[self.pos], b':' | b';' | b'\n')
                {
                    self.advance_byte();
                }
                continue;
            }
            let Some(mnemonic) = self.read_mnemonic() else {
                self.advance_byte();
                continue;
            };
            self.command_count += 1;
            let cmd_line = self.line;
            self.dispatch(mnemonic, cmd_line);
        }
        Ok(())
    }

    fn dispatch(&mut self, mnemonic: [u8; 2], line: usize) {
        if self.dispatch_state(mnemonic, line) || self.dispatch_motion_or_draw(mnemonic, line) {
            return;
        }
        self.skip_to_terminator();
    }

    fn dispatch_state(&mut self, mnemonic: [u8; 2], line: usize) -> bool {
        match &mnemonic {
            b"IN" | b"DF" => self.reset(mnemonic == *b"IN"),
            b"CO" => self.skip_comment(),
            b"DT" => self.read_label_terminator(),
            b"LB" => self.read_label(line),
            b"SP" => self.select_pen(),
            b"IP" => self.apply_ip(),
            b"SC" => self.apply_sc(),
            b"PM" => {
                let args = self.read_numbers();
                let mode = args.first().copied().unwrap_or(0.0).round() as i32;
                self.polygon_mode = mode != 2;
            }
            b"EP" | b"FP" => {
                self.polygon_mode = false;
                let _ = self.read_numbers();
            }
            _ => return false,
        }
        true
    }

    fn dispatch_motion_or_draw(&mut self, mnemonic: [u8; 2], line: usize) -> bool {
        match &mnemonic {
            b"PU" | b"PD" => {
                self.pen_down = mnemonic == *b"PD";
                let args = self.read_numbers();
                self.apply_points(&args, line);
            }
            b"PA" | b"PR" => {
                self.relative = mnemonic == *b"PR";
                let args = self.read_numbers();
                self.apply_points(&args, line);
            }
            b"AA" | b"AR" => {
                let args = self.read_numbers();
                self.apply_arc(&args, mnemonic == *b"AR", line);
            }
            b"CI" => {
                let args = self.read_numbers();
                self.apply_circle(&args, line);
            }
            b"EA" | b"ER" | b"RA" | b"RR" => {
                let rel = matches!(&mnemonic, b"ER" | b"RR");
                let args = self.read_numbers();
                self.apply_rect(&args, rel, line);
            }
            b"EW" | b"WG" => {
                let args = self.read_numbers();
                self.apply_wedge(&args, line);
            }
            _ => return false,
        }
        true
    }

    fn select_pen(&mut self) {
        let args = self.read_numbers();
        let next_pen = args
            .first()
            .map(|v| (*v).round().clamp(0.0, 255.0) as u16)
            .unwrap_or(0);
        self.pen = next_pen;
        if next_pen > 0 {
            self.pens_used.insert(next_pen);
        }
    }

    fn reset(&mut self, reset_scaling: bool) {
        self.pen = 1;
        self.pen_down = false;
        self.relative = false;
        self.label_term = 0x03;
        self.polygon_mode = false;
        if reset_scaling {
            self.scaling = None;
            self.cursor_u = [0.0, 0.0];
        }
        let _ = self.read_numbers();
    }

    fn apply_points(&mut self, args: &[f64], line: usize) {
        for pair in args.as_chunks::<2>().0 {
            if self.segments.len() >= MAX_SEGMENTS {
                break;
            }
            let target_u = if self.relative {
                [self.cursor_u[0] + pair[0], self.cursor_u[1] + pair[1]]
            } else {
                [pair[0], pair[1]]
            };
            self.emit_segment(self.cursor_u, target_u, self.pen_down, line);
            self.cursor_u = target_u;
        }
    }

    fn apply_arc(&mut self, args: &[f64], rel_center: bool, line: usize) {
        if args.len() < 3 {
            return;
        }
        let cx = if rel_center {
            self.cursor_u[0] + args[0]
        } else {
            args[0]
        };
        let cy = if rel_center {
            self.cursor_u[1] + args[1]
        } else {
            args[1]
        };
        let sweep_deg = args[2].clamp(-3600.0, 3600.0);
        let chord_deg = args.get(3).copied().unwrap_or(5.0).abs().clamp(1.0, 45.0);
        let dx = self.cursor_u[0] - cx;
        let dy = self.cursor_u[1] - cy;
        let radius = (dx * dx + dy * dy).sqrt();
        if radius < 1e-9 || !sweep_deg.is_finite() {
            return;
        }
        let start_rad = dy.atan2(dx);
        let sweep_rad = sweep_deg * PI / 180.0;
        let steps = ((sweep_deg.abs() / chord_deg).ceil() as usize).clamp(4, 360);
        let mut prev = self.cursor_u;
        for step in 1..=steps {
            if self.segments.len() >= MAX_SEGMENTS {
                break;
            }
            let theta = start_rad + sweep_rad * (step as f64 / steps as f64);
            let next = [cx + radius * theta.cos(), cy + radius * theta.sin()];
            self.emit_segment(prev, next, self.pen_down, line);
            prev = next;
        }
        self.cursor_u = prev;
    }

    fn apply_circle(&mut self, args: &[f64], line: usize) {
        let Some(&radius) = args.first() else {
            return;
        };
        let radius = radius.abs();
        if radius < 1e-9 || !radius.is_finite() {
            return;
        }
        let chord_deg = args.get(1).copied().unwrap_or(5.0).abs().clamp(1.0, 45.0);
        let steps = ((360.0 / chord_deg).ceil() as usize).clamp(12, 360);
        let [cx, cy] = self.cursor_u;
        let mut prev = [cx + radius, cy];
        for step in 1..=steps {
            if self.segments.len() >= MAX_SEGMENTS {
                break;
            }
            let theta = (2.0 * PI) * (step as f64 / steps as f64);
            let next = [cx + radius * theta.cos(), cy + radius * theta.sin()];
            self.emit_segment(prev, next, true, line);
            prev = next;
        }
    }

    fn apply_rect(&mut self, args: &[f64], rel: bool, line: usize) {
        if args.len() < 2 {
            return;
        }
        let [x0, y0] = self.cursor_u;
        let x1 = if rel { x0 + args[0] } else { args[0] };
        let y1 = if rel { y0 + args[1] } else { args[1] };
        let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
        for edge in corners.windows(2) {
            self.emit_segment(edge[0], edge[1], true, line);
        }
    }

    fn apply_wedge(&mut self, args: &[f64], line: usize) {
        if args.len() < 3 {
            return;
        }
        let radius = args[0].abs();
        if radius < 1e-9 || !radius.is_finite() {
            return;
        }
        let start_deg = args[1];
        let sweep_deg = args[2].clamp(-360.0, 360.0);
        let chord_deg = args.get(3).copied().unwrap_or(5.0).abs().clamp(1.0, 45.0);
        let steps = ((sweep_deg.abs() / chord_deg).ceil() as usize).clamp(4, 180);
        let [cx, cy] = self.cursor_u;
        let mut prev = [cx, cy];
        for step in 0..=steps {
            let theta = (start_deg + sweep_deg * (step as f64 / steps as f64)) * PI / 180.0;
            let next = [cx + radius * theta.cos(), cy + radius * theta.sin()];
            self.emit_segment(prev, next, true, line);
            prev = next;
        }
        self.emit_segment(prev, [cx, cy], true, line);
    }

    fn apply_ip(&mut self) {
        let args = self.read_numbers();
        let position = self.user_to_mm(self.cursor_u);
        if args.len() >= 4 {
            self.p1_u = [args[0], args[1]];
            self.p2_u = [args[2], args[3]];
        } else if args.len() >= 2 {
            let dx = self.p2_u[0] - self.p1_u[0];
            let dy = self.p2_u[1] - self.p1_u[1];
            self.p1_u = [args[0], args[1]];
            self.p2_u = [args[0] + dx, args[1] + dy];
        }
        self.cursor_u = self.mm_to_user(position);
    }

    fn apply_sc(&mut self) {
        let args = self.read_numbers();
        let position = self.user_to_mm(self.cursor_u);
        if args.len() >= 4 && (args[1] - args[0]).abs() > 1e-9 && (args[3] - args[2]).abs() > 1e-9 {
            self.scaling = Some(Scaling {
                xmin: args[0],
                xmax: args[1],
                ymin: args[2],
                ymax: args[3],
            });
        } else if args.is_empty() {
            self.scaling = None;
        }
        self.cursor_u = self.mm_to_user(position);
    }

    fn user_to_mm(&self, pt: [f64; 2]) -> [f64; 2] {
        let plotter = match self.scaling {
            Some(sc) => {
                let ux = (pt[0] - sc.xmin) / (sc.xmax - sc.xmin);
                let uy = (pt[1] - sc.ymin) / (sc.ymax - sc.ymin);
                [
                    self.p1_u[0] + ux * (self.p2_u[0] - self.p1_u[0]),
                    self.p1_u[1] + uy * (self.p2_u[1] - self.p1_u[1]),
                ]
            }
            None => pt,
        };
        [
            plotter[0] * MM_PER_PLOTTER_UNIT,
            plotter[1] * MM_PER_PLOTTER_UNIT,
        ]
    }

    fn mm_to_user(&self, position: [f64; 2]) -> [f64; 2] {
        let plotter = position.map(|coordinate| coordinate / MM_PER_PLOTTER_UNIT);
        let Some(sc) = self.scaling else {
            return plotter;
        };
        let [px, py] = self.p1_u;
        let [qx, qy] = self.p2_u;
        let interpolate = |p, q, v, low, high| {
            if p == q {
                low
            } else {
                low + (v - p) / (q - p) * (high - low)
            }
        };
        [
            interpolate(px, qx, plotter[0], sc.xmin, sc.xmax),
            interpolate(py, qy, plotter[1], sc.ymin, sc.ymax),
        ]
    }

    fn emit_segment(&mut self, from_u: [f64; 2], to_u: [f64; 2], pen_down: bool, line: usize) {
        let pen_active = pen_down && self.pen > 0;
        if pen_active {
            self.pens_used.insert(self.pen);
        }
        self.segments.push(HpglSegment {
            line,
            pen: self.pen,
            pen_down: pen_active,
            from: self.user_to_mm(from_u),
            to: self.user_to_mm(to_u),
        });
    }

    fn advance_byte(&mut self) -> Option<u8> {
        let b = *self.src.get(self.pos)?;
        if b == b'\n' {
            self.line += 1;
        }
        self.pos += 1;
        Some(b)
    }

    fn skip_separators(&mut self) {
        while let Some(&b) = self.src.get(self.pos) {
            if b == b';' || b.is_ascii_whitespace() {
                self.advance_byte();
            } else {
                break;
            }
        }
    }

    fn read_mnemonic(&mut self) -> Option<[u8; 2]> {
        let a = *self.src.get(self.pos)?;
        let b = *self.src.get(self.pos + 1)?;
        if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() {
            self.advance_byte();
            self.advance_byte();
            Some([a.to_ascii_uppercase(), b.to_ascii_uppercase()])
        } else {
            None
        }
    }

    fn skip_number_separators(&mut self) {
        while let Some(&b) = self.src.get(self.pos) {
            if b == b',' || b.is_ascii_whitespace() {
                self.advance_byte();
            } else {
                break;
            }
        }
    }

    fn read_number_token(&mut self) -> Option<f64> {
        let start = self.pos;
        while let Some(&c) = self.src.get(self.pos) {
            if c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.') {
                self.advance_byte();
            } else {
                break;
            }
        }
        if self.pos == start {
            self.advance_byte();
            return None;
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .ok()
            .and_then(|token| token.parse::<f64>().ok())
            .filter(|val| val.is_finite())
    }

    fn read_numbers(&mut self) -> Vec<f64> {
        let mut out = Vec::new();
        while self.pos < self.src.len() {
            self.skip_number_separators();
            let Some(&b) = self.src.get(self.pos) else {
                break;
            };
            if b == b';' || b.is_ascii_alphabetic() || b == 0x1b {
                if b == b';' {
                    self.advance_byte();
                }
                break;
            }
            if let Some(val) = self.read_number_token() {
                out.push(val);
            }
        }
        out
    }

    fn skip_comment(&mut self) {
        while let Some(&b) = self.src.get(self.pos) {
            if b == b';' || b.is_ascii_alphabetic() {
                break;
            }
            if b == b'"' {
                self.advance_byte();
                while let Some(c) = self.advance_byte() {
                    if c == b'"' {
                        break;
                    }
                }
                break;
            }
            self.advance_byte();
        }
        self.skip_to_terminator();
    }

    fn read_label_terminator(&mut self) {
        while let Some(&b) = self.src.get(self.pos) {
            if b == b';' {
                self.advance_byte();
                return;
            }
            if !b.is_ascii_whitespace() {
                self.label_term = b;
                self.advance_byte();
                break;
            }
            self.advance_byte();
        }
        self.skip_to_terminator();
    }

    fn read_label(&mut self, line: usize) {
        let start = self.pos;
        let mut end = start;
        while self.pos < self.src.len() {
            let b = self.src[self.pos];
            if b == self.label_term || b == 0x03 {
                end = self.pos;
                self.advance_byte();
                break;
            }
            if self.label_term != b'\n'
                && b == b';'
                && self
                    .src
                    .get(self.pos + 1)
                    .is_some_and(|c| c.is_ascii_alphabetic())
            {
                end = self.pos;
                break;
            }
            self.advance_byte();
            end = self.pos;
        }
        let text = String::from_utf8_lossy(&self.src[start..end])
            .trim()
            .to_string();
        if !text.is_empty() && self.labels.len() < 8192 {
            self.labels.push(HpglLabel {
                line,
                pen: self.pen,
                pos: self.user_to_mm(self.cursor_u),
                text,
            });
        }
    }

    fn skip_to_terminator(&mut self) {
        while let Some(&b) = self.src.get(self.pos) {
            if b == b';' {
                self.advance_byte();
                break;
            }
            if b.is_ascii_alphabetic() {
                break;
            }
            self.advance_byte();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_changes_preserve_the_physical_pen_position() {
        let bytes =
            b"IN;PU400,400;IP0,0,2000,2000;SC0,10,0,10;PD2,2;IP0,0,4000,4000;PR1,1;SC;PR400,400;";
        let doc = HpglDocument::parse(bytes, &AtomicBool::new(false)).unwrap();
        let drawn: Vec<_> = doc.segments.iter().filter(|seg| seg.pen_down).collect();
        assert_eq!(drawn.len(), 3);
        assert_eq!((drawn[0].from, drawn[0].to), ([10.0, 10.0], [10.0, 10.0]));
        assert_eq!((drawn[1].from, drawn[1].to), ([10.0, 10.0], [20.0, 20.0]));
        assert_eq!((drawn[2].from, drawn[2].to), ([20.0, 20.0], [30.0, 30.0]));
    }

    #[test]
    fn hpgl_pens_polylines_arcs_circles_and_labels_render() {
        let plt =
            "IN;SP1;PU0,0;PD1000,0,1000,1000;SP2;CI200;PU500,500;PD;AA500,700,90;LBTest\x03;SP0;";
        let cancel = AtomicBool::new(false);
        let doc = HpglDocument::parse(plt.as_bytes(), &cancel).unwrap();
        assert_eq!(doc.pens_used, vec![1, 2]);
        assert_eq!(doc.labels.len(), 1);
        assert_eq!(doc.labels[0].text, "Test");
        assert!(doc.draw_count > 10);
        assert!(doc.draw_length_mm > 50.0);

        let thumb = doc.render_thumbnail([160, 100], &cancel).unwrap();
        assert!(
            thumb
                .premultiplied_rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [BG_COLOR[0], BG_COLOR[1], BG_COLOR[2]])
        );
    }
}
