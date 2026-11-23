use super::times;
use egui::Ui;
use little_clip_core::engineering::{
    analysis::Clock,
    waveforms::{Integral, PowerTotals, display_value},
};
use std::ops::Range;

pub(super) struct Selection {
    pub channel: usize,
    pub native: Range<f64>,
    pub signal: &'static str,
}

pub(super) fn readings(ui: &mut Ui, totals: &PowerTotals, clock: Clock) -> Option<Selection> {
    ui.horizontal_wrapped(|ui| {
        let charge = totals.charge.as_ref();
        let energy = totals.energy.as_ref();
        let mut selected = None;
        if let Some(reading) = charge {
            let seconds = reading.time.end - reading.time.start;
            let avg = display_value(reading.value / seconds, "A");
            selected = reading_button(ui, "Avg current", avg, reading, "Current", clock);
            selected = reading_button(
                ui,
                "Charge",
                milli_hours(reading, clock, "mAh"),
                reading,
                "Current",
                clock,
            )
            .or(selected);
        }
        if let Some(reading) = energy {
            selected = reading_button(
                ui,
                "Energy",
                milli_hours(reading, clock, "mWh"),
                reading,
                "Power",
                clock,
            )
            .or(selected);
        }
        match (charge, energy) {
            (Some(charge), Some(energy)) if charge.time != energy.time => {
                integral_time(ui, "Current interval", charge, clock);
                integral_time(ui, "Power interval", energy, clock);
            }
            (Some(reading), _) | (_, Some(reading)) => {
                integral_time(ui, "Integrated", reading, clock);
            }
            (None, None) => {
                ui.weak("Charge and energy unavailable");
            }
        }
        selected
    })
    .inner
}

fn reading_button(
    ui: &mut Ui,
    label: &str,
    value: String,
    reading: &Integral,
    signal: &'static str,
    clock: Clock,
) -> Option<Selection> {
    let quality = if reading.approximate { "≈ " } else { "" };
    let partial = if reading.partial { " (partial)" } else { "" };
    let window = clock.workspace_range(&reading.time);
    let samples = if reading.approximate {
        "Includes estimates from stored means."
    } else {
        "Values are held between sample times."
    };
    ui.small_button(format!("{label} {quality}{value}{partial}"))
        .on_hover_text(format!(
            "Show {signal} · {:.9} s to {:.9} s\n{samples}",
            window.start, window.end,
        ))
        .clicked()
        .then(|| Selection {
            channel: reading.channel,
            native: reading.time.clone(),
            signal,
        })
}

fn milli_hours(reading: &Integral, clock: Clock, unit: &str) -> String {
    let value = reading.value * clock.scale() / 3.6;
    if value != 0.0 && value.abs() < 0.0001 {
        format!("{value:.3e} {unit}")
    } else {
        let number = format!("{value:.6}");
        format!(
            "{} {unit}",
            number.trim_end_matches('0').trim_end_matches('.')
        )
    }
}

fn integral_time(ui: &mut Ui, label: &str, reading: &Integral, clock: Clock) {
    ui.weak(format!(
        "{label} {}",
        times(&clock.workspace_range(&reading.time))
    ));
}
