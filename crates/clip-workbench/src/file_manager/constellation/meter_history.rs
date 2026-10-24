//! Fixed audio-clock history. Atomic slots keep metering allocation-free on the mixer thread.
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
const CAPACITY: usize = 768;
struct Slot {
    sequence: AtomicUsize,
    time: AtomicU64,
    values: [AtomicU32; 3],
}
pub(super) struct MeterHistory {
    origin: Instant,
    next: AtomicUsize,
    slots: [Slot; CAPACITY],
}
impl Default for MeterHistory {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            next: AtomicUsize::new(0),
            slots: std::array::from_fn(|_| Slot {
                sequence: AtomicUsize::new(usize::MAX),
                time: AtomicU64::new(0),
                values: std::array::from_fn(|_| AtomicU32::new(0)),
            }),
        }
    }
}
impl MeterHistory {
    pub fn push(&self, values: [f32; 3]) {
        let sequence = self.next.fetch_add(1, Ordering::SeqCst);
        let slot = &self.slots[sequence % CAPACITY];
        slot.sequence.store(usize::MAX, Ordering::SeqCst);
        slot.time.store(self.now(), Ordering::SeqCst);
        for (target, value) in slot.values.iter().zip(values) {
            target.store(value.to_bits(), Ordering::SeqCst);
        }
        slot.sequence.store(sequence, Ordering::SeqCst);
    }
    pub fn now(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
    pub fn snapshot(&self) -> Vec<(u64, [f32; 3])> {
        let end = self.next.load(Ordering::SeqCst);
        (end.saturating_sub(CAPACITY)..end)
            .filter_map(|sequence| {
                let slot = &self.slots[sequence % CAPACITY];
                if slot.sequence.load(Ordering::SeqCst) != sequence {
                    return None;
                }
                let time = slot.time.load(Ordering::SeqCst);
                let values =
                    std::array::from_fn(|i| f32::from_bits(slot.values[i].load(Ordering::SeqCst)));
                (slot.sequence.load(Ordering::SeqCst) == sequence).then_some((time, values))
            })
            .collect()
    }
}

pub(super) fn show(ui: &mut egui::Ui, history: &MeterHistory) {
    use egui::{Pos2, Stroke, Vec2};
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let plot = rect.shrink2(Vec2::new(4.0, 16.0));
    let now = history.now();
    let points: Vec<_> = history
        .snapshot()
        .into_iter()
        .filter(|(time, _)| now.saturating_sub(*time) <= 3000)
        .collect();
    let x_at = |time| plot.right() - now.saturating_sub(time) as f32 / 3000.0 * plot.width();
    for (time, [input, output, _]) in &points {
        let x = x_at(*time);
        let y =
            |level: f32| plot.bottom() - ((level + 60.0) / 66.0).clamp(0.0, 1.0) * plot.height();
        painter.line_segment(
            [Pos2::new(x, plot.bottom()), Pos2::new(x, y(*input))],
            Stroke::new(1.0_f32, ui.visuals().weak_text_color().gamma_multiply(0.4)),
        );
        painter.line_segment(
            [Pos2::new(x, plot.bottom()), Pos2::new(x, y(*output))],
            Stroke::new(1.0_f32, ui.visuals().hyperlink_color.gamma_multiply(0.7)),
        );
    }
    let reduction = points
        .iter()
        .map(|(time, values)| {
            Pos2::new(
                x_at(*time),
                plot.top() + (values[2] / 24.0).clamp(0.0, 1.0) * plot.height(),
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        reduction,
        Stroke::new(1.5_f32, egui::Color32::from_rgb(235, 175, 70)),
    ));
    painter.text(
        rect.left_top() + Vec2::new(5.0, 2.0),
        egui::Align2::LEFT_TOP,
        "Level / gain reduction · 3 s",
        egui::FontId::proportional(10.0),
        ui.visuals().text_color(),
    );
}
