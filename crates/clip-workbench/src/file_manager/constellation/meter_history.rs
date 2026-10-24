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

pub(super) fn paint(ui: &mut egui::Ui, plot: egui::Rect, history: &MeterHistory) {
    use egui::{Pos2, Stroke};
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let painter = ui.painter_at(plot);
    let now = history.now();
    let points: Vec<_> = history
        .snapshot()
        .into_iter()
        .filter(|(time, _)| now.saturating_sub(*time) <= 3000)
        .collect();
    let x_at = |time| plot.right() - now.saturating_sub(time) as f32 / 3000.0 * plot.width();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_history_retains_transients_and_wraps_without_growing() {
        let meter = MeterHistory::default();
        for index in 0..1000 {
            meter.push([index as f32, -6.0, if index == 990 { 8.0 } else { 0.0 }]);
        }
        let snapshot = meter.snapshot();
        assert_eq!(snapshot.len(), CAPACITY);
        assert_eq!(snapshot.first().unwrap().1[0], (1000 - CAPACITY) as f32);
        assert_eq!(snapshot.last().unwrap().1[0], 999.0);
        assert!(snapshot.iter().any(|(_, values)| values[2] == 8.0));
    }
}
