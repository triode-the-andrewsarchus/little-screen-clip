//! Musical interaction is separate from format parsing and source admission.
use super::{
    Region,
    surface::{self, Strike},
};
use crate::{
    ui_appearance::WorkbenchColors,
    ui_tabs::{Tab, TabBar},
};
use egui::{Align2, FontId, Rect, Stroke, pos2, vec2};
use little_clip_core::palette::AppPalette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Harmony,
    Pads,
    Mapping,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Pitched,
    Drums,
    Hybrid,
    Unknown,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pitched => "Pitched · mapped key ranges",
            Self::Drums => "Pads · percussion / unpitched hint",
            Self::Hybrid => "Hybrid · mixed pitch tracking",
            Self::Unknown => "Unknown · inspect the mapping",
        }
    }
}
pub(super) fn classify(regions: &[Region]) -> Kind {
    if regions.is_empty() {
        return Kind::Unknown;
    }
    let percussion = |r: &Region| {
        r.properties.iter().any(|(k, v)| {
            (k == "Preset" && v.starts_with("Bank 128 ·"))
                || (k == "pitch_keytrack" && v.parse::<f32>() == Ok(0.0))
        })
    };
    let unpitched = regions.iter().filter(|r| percussion(r)).count();
    match unpitched {
        n if n == regions.len() => Kind::Drums,
        n if n > 0 => Kind::Hybrid,
        _ if regions
            .iter()
            .any(|r| r.key[1].saturating_sub(r.key[0]) >= 3) =>
        {
            Kind::Pitched
        }
        _ => Kind::Unknown,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Quality {
    Major,
    Minor,
    Diminished,
    Augmented,
    HalfDiminished,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Voicing {
    Root,
    First,
    Second,
    Open,
}

pub(super) struct Explorer {
    pub mode: Mode,
    pub kind: Kind,
    pub center: u8,
    pub quality: Quality,
    pub voicing: Voicing,
    zoom: f32,
    pan: egui::Vec2,
    pub seventh: bool,
    pub lower: bool,
    pub upper: bool,
}
impl Explorer {
    pub fn new(regions: &[Region]) -> Self {
        let kind = classify(regions);
        let range = surface::bounds(regions);
        Self {
            mode: Mode::Mapping,
            kind,
            center: 60_u8.clamp(range[0], range[1]),
            quality: Quality::Major,
            voicing: Voicing::Root,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            seventh: false,
            lower: false,
            upper: false,
        }
    }
    pub fn tabs(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> bool {
        let tabs = [
            Tab::new(Mode::Harmony, "Harmony"),
            Tab::new(Mode::Pads, "Pads"),
            Tab::new(Mode::Mapping, "Mapping"),
        ];
        if let Some(mode) = TabBar::new("sampler-view", self.mode, &tabs, palette).show(ui) {
            self.mode = mode;
            true
        } else {
            false
        }
    }
    pub fn controls(&mut self, ui: &mut egui::Ui) {
        use egui::Key::*;
        let choices = [
            (Quality::Major, "1 Major", Num1),
            (Quality::Minor, "2 Minor", Num2),
            (Quality::Diminished, "3 Dim", Num3),
            (Quality::Augmented, "4 Aug", Num4),
            (Quality::HalfDiminished, "5 Half-dim", Num5),
        ];
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            for (quality, label, key) in choices {
                if ui
                    .selectable_label(self.quality == quality, label)
                    .clicked()
                {
                    action = Some(key);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(format!("6 Inversion: {:?}", self.voicing))
                .clicked()
            {
                action = Some(Num6);
            }
            for (selected, label, key) in [
                (self.seventh, "7 Seventh", Num7),
                (self.voicing == Voicing::Open, "8 Open", Num8),
                (self.lower, "9 Lower root", Num9),
                (self.upper, "0 Upper root", Num0),
            ] {
                if ui.selectable_label(selected, label).clicked() {
                    action = Some(key);
                }
            }
            if ui.small_button("Center").clicked() {
                self.pan = egui::Vec2::ZERO;
                self.zoom = 1.0;
            }
        });
        if !ui.ctx().wants_keyboard_input() {
            for key in [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, Num0] {
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                    self.apply_key(key);
                }
            }
        }
        if let Some(key) = action {
            self.apply_key(key);
        }
    }
    fn apply_key(&mut self, key: egui::Key) {
        use egui::Key::*;
        match key {
            Num1 => self.quality = Quality::Major,
            Num2 => self.quality = Quality::Minor,
            Num3 => self.quality = Quality::Diminished,
            Num4 => self.quality = Quality::Augmented,
            Num5 => {
                self.quality = Quality::HalfDiminished;
                self.seventh = true;
            }
            Num6 => {
                self.voicing = match self.voicing {
                    Voicing::Root | Voicing::Open => Voicing::First,
                    Voicing::First => Voicing::Second,
                    Voicing::Second => Voicing::Root,
                }
            }
            Num7 => self.seventh = !self.seventh,
            Num8 => {
                self.voicing = if self.voicing == Voicing::Open {
                    Voicing::Root
                } else {
                    Voicing::Open
                }
            }
            Num9 => self.lower = !self.lower,
            Num0 => self.upper = !self.upper,
            _ => {}
        }
    }
    pub fn chord(&self, root: u8) -> Option<Vec<u8>> {
        let mut notes = chord(root, self.quality, self.voicing)?;
        if self.seventh {
            let interval = match self.quality {
                Quality::Major | Quality::Augmented => 11,
                Quality::Diminished => 9,
                _ => 10,
            };
            notes.push(root.checked_add(interval).filter(|&n| n <= 127)?);
        }
        if self.lower {
            notes.push(root.checked_sub(12)?);
        }
        if self.upper {
            notes.push(root.checked_add(12).filter(|&n| n <= 127)?);
        }
        notes.sort_unstable();
        notes.dedup();
        Some(notes)
    }
    fn navigate(&mut self, ui: &egui::Ui, rect: Rect) {
        if !super::super::ui::preview_hovered(ui, rect) {
            return;
        }
        let (scroll, pointer, delta, pan) = ui.input(|i| {
            (
                i.raw_scroll_delta.y,
                i.pointer.hover_pos(),
                i.pointer.delta(),
                i.pointer.secondary_down() || i.pointer.middle_down(),
            )
        });
        if pan {
            self.pan += delta;
        }
        if scroll != 0.0 {
            let old = self.zoom;
            self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(0.35, 3.0);
            let anchor = pointer.unwrap_or(rect.center()) - rect.center();
            self.pan = anchor - (anchor - self.pan) * (self.zoom / old);
            ui.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
            });
        }
    }
    pub fn harmony(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        regions: &[Region],
        velocity: u8,
        sounding: &[u8],
        palette: &AppPalette,
    ) -> Option<Strike> {
        let colors = WorkbenchColors::from_palette(palette);
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 8.0, colors.recessed);
        self.navigate(ui, rect);
        let radius = 34.0 * self.zoom;
        let cells = visible_cells(rect, radius, self.pan, self.center);
        let pointer = super::super::ui::preview_hovered(ui, rect)
            .then(|| ui.input(|i| i.pointer.hover_pos()))
            .flatten();
        let hovered = pointer
            .and_then(|p| {
                cells.iter().find(|cell| {
                    surface::contains(
                        pos2(cell.center.y, cell.center.x),
                        radius - 1.0,
                        pos2(p.y, p.x),
                    )
                })
            })
            .and_then(|cell| cell.note())
            .map(|key| Strike { key, velocity });
        let preview = hovered.and_then(|s| self.chord(s.key)).unwrap_or_default();
        let mut covered = [false; 128];
        for region in regions
            .iter()
            .filter(|r| (r.velocity[0]..=r.velocity[1]).contains(&velocity))
        {
            if let Some(keys) =
                covered.get_mut(usize::from(region.key[0])..=usize::from(region.key[1]))
            {
                keys.fill(true);
            }
        }
        for cell in cells {
            let mapped = cell.note().is_some_and(|key| covered[key as usize]);
            let active = cell
                .note()
                .is_some_and(|key| sounding.contains(&key) || preview.contains(&key));
            cell.paint(&painter, radius, mapped, active, palette);
        }
        if let Some(strike) = hovered {
            harmony_tooltip(ui, self.chord(strike.key).as_deref(), regions, velocity);
        }
        surface::gesture(ui, rect, "harmony", hovered)
    }
}

struct Hex {
    center: egui::Pos2,
    pitch: i32,
}
impl Hex {
    fn note(&self) -> Option<u8> {
        u8::try_from(self.pitch).ok().filter(|&n| n <= 127)
    }
    fn paint(
        &self,
        painter: &egui::Painter,
        radius: f32,
        mapped: bool,
        active: bool,
        palette: &AppPalette,
    ) {
        let colors = WorkbenchColors::from_palette(palette);
        let (r, g, b) = palette.midi_note_color(0, 0, 1, self.note().unwrap_or(0), 100);
        let color = egui::Color32::from_rgb(r, g, b);
        let fill = if mapped {
            color.gamma_multiply(if active { 0.85 } else { 0.32 })
        } else {
            colors.panel
        };
        let border = if active {
            colors.text_primary
        } else if mapped {
            color.gamma_multiply(0.7)
        } else {
            colors.border
        };
        let points = (0..6)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 6.0 - std::f32::consts::FRAC_PI_2;
                self.center + vec2(a.cos(), a.sin()) * (radius - 2.0)
            })
            .collect();
        painter.add(egui::Shape::convex_polygon(
            points,
            fill,
            Stroke::new(if active { 2.5_f32 } else { 1.0_f32 }, border),
        ));
        if let Some(key) = self.note() {
            painter.text(
                self.center,
                Align2::CENTER_CENTER,
                surface::note(key),
                FontId::proportional((radius * 0.4).clamp(10.0, 16.0)),
                crate::ui_ext::contrast_text_c32(fill, Some(colors.text_primary), palette),
            );
        }
    }
}
fn visible_cells(rect: Rect, radius: f32, pan: egui::Vec2, root: u8) -> Vec<Hex> {
    let origin = rect.center() + pan;
    let first_row = ((rect.top() - origin.y) / (radius * 1.5)).floor() as i32 - 1;
    let last_row = ((rect.bottom() - origin.y) / (radius * 1.5)).ceil() as i32 + 1;
    let mut cells = Vec::new();
    for r in first_row..=last_row {
        let first =
            ((rect.left() - origin.x) / (radius * 1.7320508) - r as f32 * 0.5).floor() as i32 - 1;
        let last =
            ((rect.right() - origin.x) / (radius * 1.7320508) - r as f32 * 0.5).ceil() as i32 + 1;
        for q in first..=last {
            cells.push(Hex {
                pitch: i32::from(root) + q * 7 + r * 4,
                center: origin
                    + vec2(
                        (q as f32 + r as f32 * 0.5) * radius * 1.7320508,
                        r as f32 * radius * 1.5,
                    ),
            });
        }
    }
    cells
}

pub(super) fn chord(root: u8, quality: Quality, voicing: Voicing) -> Option<Vec<u8>> {
    let (third, fifth) = match quality {
        Quality::Major => (4, 7),
        Quality::Minor => (3, 7),
        Quality::Diminished | Quality::HalfDiminished => (3, 6),
        Quality::Augmented => (4, 8),
    };
    let intervals = match voicing {
        Voicing::Root => vec![0, third, fifth],
        Voicing::First => vec![third, fifth, 12],
        Voicing::Second => vec![fifth, 12, third + 12],
        Voicing::Open => vec![0, fifth, third + 12],
    };
    intervals
        .into_iter()
        .map(|interval| root.checked_add(interval).filter(|&k| k <= 127))
        .collect()
}

fn harmony_tooltip(ui: &egui::Ui, notes: Option<&[u8]>, regions: &[Region], velocity: u8) {
    egui::show_tooltip_at_pointer(ui.ctx(), ui.id().with("chord-details"), |ui| {
        ui.strong(format!("Velocity {velocity}"));
        if let Some(notes) = notes {
            for &key in notes {
                let name = surface::choose(regions, Strike { key, velocity }, None)
                    .map_or("Unmapped", |i| regions[i].name.as_str());
                ui.label(format!("{} · MIDI {key} · {name}", surface::note(key)));
            }
        } else {
            ui.label("Voicing extends beyond MIDI range");
        }
        ui.small("Click / drag to play · wheel to zoom · right-drag to pan");
    });
}

pub(super) fn mapping(
    ui: &mut egui::Ui,
    rect: Rect,
    regions: &[Region],
    selected: Option<Strike>,
    palette: &AppPalette,
) -> Option<Strike> {
    let colors = WorkbenchColors::from_palette(palette);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 8.0, colors.recessed);
    let range = surface::bounds(regions);
    let span = f32::from(range[1] - range[0]) + 1.0;
    let x = |key: f32| rect.left() + rect.width() * (key - f32::from(range[0])) / span;
    let y = |velocity: f32| rect.bottom() - rect.height() * velocity / 128.0;
    for velocity in [32, 64, 96] {
        painter.line_segment(
            [
                pos2(rect.left(), y(velocity as f32)),
                pos2(rect.right(), y(velocity as f32)),
            ],
            Stroke::new(1.0_f32, colors.border),
        );
        painter.text(
            pos2(rect.right() - 4.0, y(velocity as f32) - 2.0),
            Align2::RIGHT_BOTTOM,
            velocity,
            FontId::proportional(10.0),
            colors.text_muted,
        );
    }
    for region in regions {
        let zone = Rect::from_min_max(
            pos2(x(region.key[0] as f32), y(region.velocity[1] as f32 + 1.0)),
            pos2(x(region.key[1] as f32 + 1.0), y(region.velocity[0] as f32)),
        );
        let (r, g, b) = palette.midi_note_color(0, 0, 1, region.root, 100);
        let color = egui::Color32::from_rgb(r, g, b);
        painter.rect_filled(zone.shrink(0.5), 2.0, color.gamma_multiply(0.25));
        painter.rect_stroke(zone, 2.0, Stroke::new(1.0_f32, color));
        if (region.key[0]..=region.key[1]).contains(&region.root) {
            painter.circle_filled(
                pos2(x(region.root as f32 + 0.5), zone.center().y),
                3.0,
                color,
            );
        }
    }
    if let Some(strike) = selected {
        painter.line_segment(
            [
                pos2(rect.left(), y(strike.velocity as f32)),
                pos2(rect.right(), y(strike.velocity as f32)),
            ],
            Stroke::new(1.0_f32, colors.text_primary),
        );
        painter.circle_stroke(
            pos2(x(strike.key as f32 + 0.5), y(strike.velocity as f32)),
            5.0,
            Stroke::new(2.0_f32, colors.text_primary),
        );
    }
    let hovered = super::super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten()
        .map(|p| Strike {
            key: (range[0] as f32 + (p.x - rect.left()) / rect.width() * span)
                .floor()
                .clamp(range[0] as f32, range[1] as f32) as u8,
            velocity: ((rect.bottom() - p.y) / rect.height() * 128.0).clamp(1.0, 127.0) as u8,
        });
    if let Some(strike) = hovered {
        harmony_tooltip(ui, Some(&[strike.key]), regions, strike.velocity);
    }
    surface::gesture(ui, rect, "key-velocity", hovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chord_shortcuts_toggle_without_accumulating_duplicate_notes() {
        let mut explorer = Explorer::new(&[]);
        explorer.apply_key(egui::Key::Num4);
        assert_eq!(explorer.chord(60), Some(vec![60, 64, 68]));
        explorer.apply_key(egui::Key::Num2);
        explorer.apply_key(egui::Key::Num7);
        assert_eq!(explorer.chord(60), Some(vec![60, 63, 67, 70]));
        explorer.apply_key(egui::Key::Num7);
        explorer.apply_key(egui::Key::Num6);
        assert_eq!(explorer.chord(60), Some(vec![63, 67, 72]));
        explorer.apply_key(egui::Key::Num8);
        assert_eq!(explorer.chord(60), Some(vec![60, 67, 75]));
    }
    #[test]
    fn lattice_draws_only_visible_cells_and_preserves_pitch_while_zooming() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0));
        for radius in [12.0, 34.0, 102.0] {
            let cells = visible_cells(rect, radius, egui::Vec2::ZERO, 60);
            assert!(cells.len() < 2500);
            assert_eq!(
                cells
                    .iter()
                    .find(|cell| cell.center == rect.center())
                    .unwrap()
                    .note(),
                Some(60)
            );
            assert!(
                visible_cells(rect, radius, vec2(10000.0, 0.0), 60)
                    .iter()
                    .all(|cell| cell.note().is_none())
            );
        }
    }
    #[test]
    fn default_mapping_and_chord_modifiers_preserve_exact_pitches() {
        let mut explorer = Explorer::new(&[]);
        assert_eq!(explorer.mode, Mode::Mapping);
        explorer.seventh = true;
        explorer.lower = true;
        explorer.upper = true;
        assert_eq!(explorer.chord(60), Some(vec![48, 60, 64, 67, 71, 72]));
        explorer.quality = Quality::Diminished;
        assert_eq!(explorer.chord(60), Some(vec![48, 60, 63, 66, 69, 72]));
        explorer.quality = Quality::HalfDiminished;
        assert_eq!(explorer.chord(60), Some(vec![48, 60, 63, 66, 70, 72]));
        assert_eq!(explorer.chord(5), None);
        assert_eq!(explorer.chord(125), None);
    }
}
