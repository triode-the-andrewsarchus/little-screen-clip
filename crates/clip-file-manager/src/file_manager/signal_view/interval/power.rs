use super::{overlap, times};
use crate::file_manager::signal_view::{Capture, Format, Source, alignment};
use egui::{Button, Ui};
use little_clip_core::engineering::{
    analysis::Clock,
    waveforms::{Integral, PowerTotals, PowerWindows, display_value},
};
use std::ops::Range;

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Basis {
    #[default]
    Common,
    Each,
}

#[derive(Clone, PartialEq)]
pub(super) enum Scope {
    Each,
    Common(PowerWindows),
}

impl Scope {
    pub fn has_windows(&self) -> bool {
        match self {
            Self::Each => true,
            Self::Common(windows) => windows.current.is_some() || windows.power.is_some(),
        }
    }
}

pub(super) struct Comparison {
    common: PowerWindows,
    unavailable: bool,
}

impl Comparison {
    pub fn from_sources(
        sources: &[Source],
        preview: Option<alignment::Suggestion>,
        shared: &Range<f64>,
    ) -> Option<Self> {
        let mut common = PowerWindows {
            current: Some(shared.clone()),
            power: Some(shared.clone()),
        };
        let mut participants = 0;
        let mut unavailable = false;
        for source in sources {
            if let Some(coverage) = coverage(source) {
                participants += 1;
                common = aligned_overlap(common, coverage, source.display_clock(preview));
            } else if matches!(source.capture, Capture::Unavailable(_))
                && Format::for_path(&source.path) == Some(Format::PowerRecording)
            {
                participants += 1;
                unavailable = true;
            }
        }
        (participants > 1).then(|| Self {
            common: if unavailable {
                PowerWindows::default()
            } else {
                common
            },
            unavailable,
        })
    }

    pub fn controls(&self, ui: &mut Ui, basis: &mut Basis) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Power totals");
            for (value, label, hint) in [
                (
                    Basis::Common,
                    "Common interval",
                    "Use the part of the pinned time covered by all power recordings",
                ),
                (
                    Basis::Each,
                    "Each recording",
                    "Use each recording’s own coverage of the pinned time",
                ),
            ] {
                if ui
                    .add(Button::new(label).selected(*basis == value).small())
                    .on_hover_text(hint)
                    .clicked()
                {
                    *basis = value;
                }
            }
            if *basis == Basis::Common {
                self.describe(ui);
            }
        });
    }

    pub fn scope(&self, source: &Source, clock: Clock) -> Scope {
        let Some(coverage) = coverage(source) else {
            return Scope::Each;
        };
        Scope::Common(PowerWindows {
            current: native_overlap(
                self.common.current.as_ref(),
                coverage.current.as_ref(),
                clock,
            ),
            power: native_overlap(self.common.power.as_ref(), coverage.power.as_ref(), clock),
        })
    }

    fn describe(&self, ui: &mut Ui) {
        match (&self.common.current, &self.common.power) {
            (_, _) if self.unavailable => {
                ui.weak("A power recording is unavailable");
            }
            (None, None) => {
                ui.weak("No common current or power coverage");
            }
            (Some(current), Some(power)) if current == power => {
                ui.monospace(times(current));
            }
            (current, power) => {
                for (label, window) in [("Current", current), ("Power", power)] {
                    if let Some(range) = window {
                        ui.monospace(format!("{label} {}", times(range)));
                    } else {
                        ui.weak(format!("{label} has no common coverage"));
                    }
                }
            }
        }
    }
}

fn coverage(source: &Source) -> Option<PowerWindows> {
    let Capture::Analog(view) = &source.capture else {
        return None;
    };
    view.recording().power_coverage()
}

fn aligned_overlap(common: PowerWindows, coverage: PowerWindows, clock: Clock) -> PowerWindows {
    let intersect = |a: Option<Range<f64>>, b: Option<Range<f64>>| {
        let (a, b) = a.zip(b)?;
        overlap(&a, &clock.workspace_range(&b))
    };
    PowerWindows {
        current: intersect(common.current, coverage.current),
        power: intersect(common.power, coverage.power),
    }
}

fn native_overlap(
    workspace: Option<&Range<f64>>,
    coverage: Option<&Range<f64>>,
    clock: Clock,
) -> Option<Range<f64>> {
    overlap(&clock.native_range(workspace?), coverage?)
}

pub(super) struct Selection {
    pub channel: usize,
    pub native: Range<f64>,
    pub signal: &'static str,
}

pub(super) fn readings(
    ui: &mut Ui,
    totals: &PowerTotals,
    scope: &Scope,
    clock: Clock,
) -> Option<Selection> {
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
        match scope {
            Scope::Each => describe_integrals(ui, charge, energy, clock),
            Scope::Common(windows) => {
                for (label, requested, reading) in [
                    ("Current", windows.current.as_ref(), charge),
                    ("Power", windows.power.as_ref(), energy),
                ] {
                    if requested.is_some() && reading.is_none() {
                        ui.weak(format!("{label} unavailable in this recording"));
                    }
                }
            }
        }
        selected
    })
    .inner
}

fn describe_integrals(
    ui: &mut Ui,
    charge: Option<&Integral>,
    energy: Option<&Integral>,
    clock: Clock,
) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_power_uses_each_signal_coverage_on_corrected_clocks() {
        let pin = 0.5..2.5;
        let a = PowerWindows {
            current: Some(0.0..3.0),
            power: Some(0.75..3.0),
        };
        let b = PowerWindows {
            current: Some(0.0..2.0),
            power: Some(0.5..1.5),
        };
        let correction = Clock::anchored(0.0, 0.125, 1.25).unwrap();
        let common = aligned_overlap(
            PowerWindows {
                current: Some(pin.clone()),
                power: Some(pin.clone()),
            },
            a,
            Clock::default(),
        );
        let joint = aligned_overlap(common.clone(), b.clone(), correction);
        assert_eq!(joint.current, Some(pin));
        assert_eq!(joint.power, Some(0.75..2.0));
        assert_eq!(
            native_overlap(joint.power.as_ref(), b.power.as_ref(), correction),
            Some(0.5..1.5)
        );
        let no_power = aligned_overlap(
            common.clone(),
            PowerWindows {
                current: b.current.clone(),
                power: None,
            },
            correction,
        );
        assert_eq!(no_power.current, joint.current);
        assert_eq!(no_power.power, None);
        assert_eq!(
            aligned_overlap(common, b, Clock::offset(3.0)),
            PowerWindows::default()
        );
    }
}
