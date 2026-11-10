//! Shared note labels, coverage, piano reference and chromatic hex interaction.
use super::Region;
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, pos2, vec2};
use little_clip_core::palette::AppPalette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Strike {
    pub key: u8,
    pub velocity: u8,
}

pub(super) fn note(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
    ];
    format!("{}{}", NAMES[key as usize % 12], key as i16 / 12 - 1)
}

pub(super) fn bounds(regions: &[Region]) -> [u8; 2] {
    let low = regions.iter().map(|r| r.key[0]).min().unwrap_or(48);
    let high = regions.iter().map(|r| r.key[1]).max().unwrap_or(71);
    [
        low / 12 * 12,
        ((high as u16 / 12 + 1) * 12 - 1).min(127) as u8,
    ]
}

pub(super) fn matches(region: &Region, strike: Strike) -> bool {
    (region.key[0]..=region.key[1]).contains(&strike.key)
        && (region.velocity[0]..=region.velocity[1]).contains(&strike.velocity)
}

pub(super) fn choose(regions: &[Region], strike: Strike, previous: Option<usize>) -> Option<usize> {
    // Repeated strikes expose overlapping recordings without claiming to implement
    // the source instrument's round-robin or articulation engine.
    let start = previous
        .and_then(|i| i.checked_add(1))
        .unwrap_or(0)
        .min(regions.len());
    (start..regions.len())
        .chain(0..start.min(regions.len()))
        .find(|&i| matches(&regions[i], strike))
}

pub(super) fn contains(center: Pos2, radius: f32, point: Pos2) -> bool {
    let d = (point - center).abs();
    d.y <= radius * 0.8660254 && d.x + d.y / 1.7320508 <= radius
}

/// Equal-width chromatic keys expose pitch coverage and vertical playing intensity.
pub(super) fn strip_keys(
    ui: &mut egui::Ui,
    rect: Rect,
    regions: &[Region],
    sounding: &[u8],
    palette: &AppPalette,
) -> Option<Strike> {
    let colors = WorkbenchColors::from_palette(palette);
    let range = bounds(regions);
    let width = rect.width() / f32::from(range[1] - range[0] + 1);
    let pointer = super::super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    let mut hovered = None;
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    for key in range[0]..=range[1] {
        let cell = Rect::from_min_size(
            pos2(rect.left() + f32::from(key - range[0]) * width, rect.top()),
            vec2(width, rect.height()),
        );
        let mapped = regions.iter().any(|r| (r.key[0]..=r.key[1]).contains(&key));
        let native = regions
            .iter()
            .any(|r| r.root == key && (r.key[0]..=r.key[1]).contains(&key));
        let fill = if mapped {
            let (r, g, b) = palette.midi_note_color(0, 0, 1, key, 100);
            let mut color = egui::ecolor::Hsva::from(Color32::from_rgb(r, g, b));
            if !native {
                color.s *= 0.28;
            }
            Color32::from(color)
        } else {
            colors.panel
        };
        painter.rect_filled(cell.shrink(1.0), 3.0, fill);
        if sounding.contains(&key) {
            painter.rect_stroke(
                cell.shrink(1.0),
                3.0,
                Stroke::new(2.0_f32, colors.text_primary),
            );
        }
        if width >= 15.0 {
            painter.text(
                cell.center(),
                Align2::CENTER_CENTER,
                note(key),
                FontId::proportional(9.0),
                crate::ui_ext::contrast_text_c32(fill, Some(colors.text_primary), palette),
            );
        }
        if let Some(point) = pointer.filter(|p| cell.contains(*p)) {
            let strike = Strike {
                key,
                velocity: ((rect.bottom() - point.y) / rect.height() * 126.0 + 1.0)
                    .round()
                    .clamp(1.0, 127.0) as u8,
            };
            hovered = Some(strike);
            painter.circle_filled(point, 3.0, colors.text_primary);
            egui::show_tooltip_at_pointer(ui.ctx(), ui.id().with("strip-note"), |ui| {
                ui.strong(format!("{} · velocity {}", note(key), strike.velocity));
                ui.label(if !mapped {
                    "Unmapped"
                } else if native {
                    "Recorded at this pitch"
                } else {
                    "Repitched recording"
                });
                ui.small("Higher: louder · lower: softer · click or drag to audition");
            });
        }
    }
    gesture(ui, rect, "strip-keys", hovered)
}

pub(super) fn gesture(
    ui: &mut egui::Ui,
    rect: Rect,
    name: &str,
    hovered: Option<Strike>,
) -> Option<Strike> {
    let response = ui.interact(rect, ui.id().with(name), egui::Sense::click_and_drag());
    let pressed = response.clicked() || response.drag_started() || response.dragged();
    let id = ui.id().with((name, "last-strike"));
    let previous = ui.data(|data| data.get_temp::<Strike>(id));
    if !ui.input(|i| i.pointer.primary_down()) {
        ui.data_mut(|data| data.remove::<Strike>(id));
    }
    let strike =
        hovered.filter(|strike| pressed && (response.clicked() || Some(*strike) != previous));
    if let Some(strike) = strike {
        ui.data_mut(|data| data.insert_temp(id, strike));
    }
    strike
}

pub(super) fn keyboard(
    ui: &mut egui::Ui,
    rect: Rect,
    range: [u8; 2],
    selected: &[u8],
    regions: &[Region],
    palette: &AppPalette,
) -> Option<u8> {
    let colors = WorkbenchColors::from_palette(palette);
    let white = |key: u8| !matches!(key % 12, 1 | 3 | 6 | 8 | 10);
    let whites = (range[0]..=range[1]).filter(|&k| white(k)).count().max(1);
    let width = rect.width() / whites as f32;
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let mut hit = None;
    for black in [false, true] {
        let mut index = 0;
        for key in range[0]..=range[1] {
            let is_white = white(key);
            if is_white != black {
                let x = rect.left() + width * index as f32;
                let key_rect = if black {
                    Rect::from_min_size(
                        pos2(x - width * 0.32, rect.top()),
                        vec2(width * 0.64, rect.height() * 0.6),
                    )
                } else {
                    Rect::from_min_size(pos2(x, rect.top()), vec2(width - 1.0, rect.height()))
                };
                let active = selected.contains(&key);
                let fill = if active {
                    let (r, g, b) = palette.midi_note_color(0, 0, 1, key, 100);
                    Color32::from_rgb(r, g, b)
                } else if black {
                    Color32::from_gray(35)
                } else {
                    Color32::from_gray(220)
                };
                painter.rect_filled(key_rect, 2.0, fill);
                if regions.iter().any(|r| (r.key[0]..=r.key[1]).contains(&key)) {
                    let (r, g, b) = palette.midi_note_color(0, 0, 1, key, 100);
                    painter.rect_filled(
                        Rect::from_min_size(key_rect.min, vec2(key_rect.width(), 3.0)),
                        0.0,
                        Color32::from_rgb(r, g, b),
                    );
                }
                if key % 12 == 0 && width > 8.0 {
                    painter.text(
                        key_rect.center_bottom() - vec2(0.0, 3.0),
                        Align2::CENTER_BOTTOM,
                        note(key),
                        FontId::proportional(if width < 16.0 { 9.0 } else { 10.0 }),
                        crate::ui_ext::contrast_text_c32(fill, None, palette),
                    );
                }
                if super::super::ui::preview_hovered(ui, key_rect) {
                    hit = Some(key);
                }
            }
            if is_white {
                index += 1;
            }
        }
    }
    if let Some(key) = hit {
        egui::show_tooltip_at_pointer(ui.ctx(), ui.id().with("piano-note"), |ui| {
            ui.label(format!("{} · MIDI {key} · click to audition", note(key)));
        });
    }
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, colors.border));
    ui.interact(rect, ui.id().with("piano-reference"), egui::Sense::click())
        .clicked()
        .then_some(hit)
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_hit_testing_rejects_corners_and_gaps() {
        let center = pos2(100.0, 100.0);
        assert!(contains(center, 20.0, center));
        assert!(contains(center, 20.0, pos2(119.0, 100.0)));
        assert!(!contains(center, 20.0, pos2(119.0, 115.0)));
        assert!(!contains(center, 20.0, pos2(100.0, 118.0)));
    }
}
