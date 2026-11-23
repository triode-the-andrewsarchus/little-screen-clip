use super::super::{
    capture::{Band, Cutoff},
    plot::Projection,
};
use super::{
    data::{Outcome, Pair, Reads, Source, TraceKeys},
    sweep_colors,
};
use egui::{Button, Ui};
use little_clip_core::engineering::{
    touchstone::{
        BandSearch, Complex64, ThresholdBand, TracePoint,
        comparison::{Delay, Sample},
    },
    waveforms::display_value,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Metric {
    #[default]
    None,
    Cutoff,
    Delay,
}

pub(super) struct Tools {
    metric: Metric,
    below: bool,
    cutoff_db: f64,
    visible_scope: bool,
    aperture_mhz: f64,
}

impl Tools {
    pub(super) fn new(band: Band) -> Self {
        Self {
            metric: Metric::None,
            below: true,
            cutoff_db: -10.0,
            visible_scope: false,
            aperture_mhz: ((band[1] - band[0]) * 1e-7).max(1e-6),
        }
    }

    pub(super) fn controls(&mut self, ui: &mut Ui, cutoff_available: bool, delay_available: bool) {
        ui.horizontal_wrapped(|ui| {
            for (metric, title, available) in [
                (Metric::Cutoff, "Compare bands", cutoff_available),
                (Metric::Delay, "Compare group delay", delay_available),
            ] {
                let selected = self.metric == metric;
                if ui.add_enabled(available || selected, Button::new(title).selected(selected))
                    .on_disabled_hover_text("This comparison needs both sweeps on the same measurement basis and an overlapping frequency range.")
                    .clicked() {
                    self.metric = if selected { Metric::None } else { metric };
                }
            }
        });
        match self.metric {
            Metric::Cutoff if cutoff_available => self.cutoff_controls(ui),
            Metric::Delay if delay_available => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Requested aperture");
                    ui.add(
                        egui::DragValue::new(&mut self.aperture_mhz)
                            .range(1e-9..=1e12)
                            .max_decimals(9)
                            .speed(0.2)
                            .suffix(" MHz")
                            .update_while_editing(false),
                    );
                });
            }
            _ => {}
        }
    }

    pub(super) fn active(&self) -> bool {
        self.metric != Metric::None
    }

    pub(super) fn cutoff_db(&self) -> Option<f64> {
        (self.metric == Metric::Cutoff).then_some(self.cutoff_db)
    }

    pub(super) fn set_cutoff(&mut self, db: f64) {
        self.cutoff_db = db.clamp(-240.0, 240.0);
    }

    fn cutoff_controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            for (below, title) in [(true, "At most"), (false, "At least")] {
                if ui
                    .add(Button::new(title).selected(self.below == below))
                    .clicked()
                {
                    self.below = below;
                }
            }
            ui.add(
                egui::DragValue::new(&mut self.cutoff_db)
                    .range(-240.0..=240.0)
                    .speed(0.1)
                    .suffix(" dB")
                    .update_while_editing(false),
            );
            ui.separator();
            for (visible, title) in [(false, "Shared span"), (true, "Visible overlap")] {
                if ui
                    .add(Button::new(title).selected(self.visible_scope == visible))
                    .clicked()
                {
                    self.visible_scope = visible;
                }
            }
        });
    }

    pub(super) fn threshold(
        &self,
        common: Option<Band>,
        viewport: Band,
        marker: f64,
    ) -> Option<(Band, Cutoff)> {
        if self.metric != Metric::Cutoff {
            return None;
        }
        let [lo, hi] = common?;
        let band = if self.visible_scope {
            [lo.max(viewport[0]), hi.min(viewport[1])]
        } else {
            [lo, hi]
        };
        (band[1] > band[0] && (band[0]..=band[1]).contains(&marker)).then_some((
            band,
            if self.below {
                Cutoff::Below(self.cutoff_db)
            } else {
                Cutoff::Above(self.cutoff_db)
            },
        ))
    }

    pub(super) fn aperture(&self, common: Option<Band>, marker: f64) -> Option<Band> {
        if self.metric != Metric::Delay {
            return None;
        }
        let [lo, hi] = common?;
        let half = self.aperture_mhz * 0.5e6;
        let band = [(marker - half).max(lo), (marker + half).min(hi)];
        ((lo..=hi).contains(&marker) && band[1] > band[0]).then_some(band)
    }

    pub(super) fn metrics(&self, ui: &mut Ui, context: Metrics<'_>) -> Option<f64> {
        let Metrics {
            reads,
            traces,
            order,
            threshold,
            aperture,
            marker,
        } = context;
        match self.metric {
            Metric::None => None,
            Metric::Cutoff => {
                if let Some((band, cutoff)) = threshold {
                    show_bands(
                        ui,
                        reads.bands(traces, band, cutoff),
                        order,
                        band,
                        marker,
                        self.below,
                    )
                } else {
                    ui.weak("Place the marker inside the comparison range to measure its band.");
                    None
                }
            }
            Metric::Delay => {
                if let Some(band) = aperture {
                    show_delay(ui, reads.delay(traces, band), order, band)
                } else {
                    ui.weak("Place the marker inside the common coverage to measure delay.");
                    None
                }
            }
        }
    }
}

pub(super) struct Metrics<'a> {
    pub reads: &'a Reads,
    pub traces: TraceKeys,
    pub order: [usize; 2],
    pub threshold: Option<(Band, Cutoff)>,
    pub aperture: Option<Band>,
    pub marker: f64,
}

pub(super) fn marker(
    ui: &mut Ui,
    sources: &[Source; 2],
    order: [usize; 2],
    traces: TraceKeys,
    values: Outcome<'_, Option<Sample>>,
    compatible: bool,
    projection: Projection,
) -> Option<f64> {
    let result = values.and_then(Result::ok);
    let selected = sweep_columns(ui, order, "marker", |ui, index| {
        let Some(trace) = traces[index] else {
            ui.weak("Trace unavailable");
            return None;
        };
        let sample = match result.and_then(|pair| pair[index].as_ref()) {
            Some(Ok(Some(sample))) => sample,
            Some(Ok(None)) => {
                ui.weak("Outside measured coverage");
                return None;
            }
            Some(Err(error)) => {
                read_error(ui, error);
                return None;
            }
            None => {
                if let Some(Err(error)) = values {
                    read_error(ui, error);
                } else {
                    ui.weak("Reading the marker…");
                }
                return None;
            }
        };
        ui.horizontal_wrapped(|ui| {
            if sample.exact() {
                ui.strong("Exact");
            } else {
                ui.strong("Interpolated").on_hover_text(
                    "Interpolated between recorded rows in the displayed projection",
                );
            }
            let value = projection.marker_value(*sample);
            if value.is_finite() {
                let units = unit(projection, sources[index].magnitude_unit(trace));
                let digits = if projection == Projection::Decibels && value <= -160.0 {
                    "≤ −160".to_owned()
                } else {
                    format!("{value:.3}")
                };
                ui.monospace(format!("{digits}{units}"));
            } else {
                ui.weak("Phase unavailable");
            }
            if sample.exact() {
                ui.monospace(rectangular(sample.value));
            }
        });
        rows(ui, &sample.rows)
    });
    if compatible && let Some([a, b]) = ordered(result, order, |sample| *sample) {
        let source = &sources[order[0]];
        let magnitude_unit = traces[order[0]].map_or("", |trace| source.magnitude_unit(trace));
        let [a, b] = [a, b].map(|sample| projection.marker_value(sample));
        let clipped = projection == Projection::Decibels && a.min(b) <= -160.0;
        let difference = projection.difference(b, a);
        if difference.is_finite() && !clipped {
            ui.monospace(format!(
                "B − A  {difference:+.4}{}",
                unit(projection, magnitude_unit)
            ));
        }
    }
    selected
}

pub(super) fn selected_band(search: &BandSearch, marker: f64) -> Option<ThresholdBand> {
    let next = search
        .bands
        .partition_point(|band| band.edges[0].frequency_hz <= marker);
    let band = *search.bands.get(next.checked_sub(1)?)?;
    (marker <= band.edges[1].frequency_hz).then_some(band)
}

fn show_bands(
    ui: &mut Ui,
    response_data: Outcome<'_, BandSearch>,
    order: [usize; 2],
    scope: Band,
    marker: f64,
    below: bool,
) -> Option<f64> {
    ui.weak(format!("Cutoff search over {}", span(scope)));
    let result = response(ui, response_data, "Searching both sweeps…")?;
    ui.horizontal_wrapped(|ui| {
        if let Some([a, b]) = ordered(Some(result), order, |search| selected_band(search, marker)) {
            if [a, b]
                .iter()
                .all(|band| band.edges.iter().all(|edge| !edge.limited))
            {
                ui.monospace(format!(
                    "B − A bandwidth  {}",
                    display_value(b.width_hz() - a.width_hz(), "Hz")
                ));
            } else {
                ui.weak("Bandwidth delta unavailable at a coverage boundary.");
            }
            let difference = b.extreme_db - a.extreme_db;
            if below && difference.is_finite() {
                ui.monospace(format!("B − A observed notch minimum  {difference:+.3} dB"));
            }
        }
    });
    sweep_columns(ui, order, "band at marker", |ui, index| {
        match &result[index] {
            Some(Ok(search)) => return band_details(ui, search, marker, below),
            Some(Err(error)) => read_error(ui, error),
            None => {}
        }
        None
    })
}

fn band_details(ui: &mut Ui, search: &BandSearch, marker: f64, below: bool) -> Option<f64> {
    let Some(band) = selected_band(search, marker) else {
        ui.weak(if search.found > search.bands.len() {
            "No retained band here. Use a narrower visible range."
        } else {
            "No matching band at this marker."
        });
        return None;
    };
    let width = display_value(band.width_hz(), "Hz");
    let limited = band.edges.iter().any(|edge| edge.limited);
    ui.label(if limited {
        format!("Observed width ≥ {width}")
    } else {
        format!("Bandwidth {width}")
    });
    let label = if below {
        "Observed notch minimum"
    } else {
        "Observed peak"
    };
    let mut jump = ui
        .small_button(format!("{label}  {:.3} dB", band.extreme_db))
        .on_hover_text(row_tip(band.extreme))
        .clicked()
        .then_some(band.extreme.frequency_hz);
    for (title, edge) in ["Low", "High"].into_iter().zip(band.edges) {
        ui.horizontal_wrapped(|ui| {
            let kind = if edge.limited { "coverage" } else { "crossing" };
            if ui
                .small_button(format!(
                    "{title} {kind} · {}",
                    display_value(edge.frequency_hz, "Hz")
                ))
                .clicked()
            {
                jump = Some(edge.frequency_hz);
            }
            jump = rows(ui, &edge.rows).or(jump);
        });
    }
    jump
}

fn show_delay(
    ui: &mut Ui,
    response_data: Outcome<'_, Option<Delay>>,
    order: [usize; 2],
    aperture: Band,
) -> Option<f64> {
    ui.weak(format!("Shared delay aperture over {}", span(aperture)));
    let result = response(ui, response_data, "Calculating both delays…")?;
    if let Some([a, b]) = ordered(Some(result), order, |delay| *delay) {
        ui.monospace(format!(
            "B − A delay  {}",
            display_value(b.seconds - a.seconds, "s")
        ));
    }
    sweep_columns(ui, order, "group delay", |ui, index| {
        let mut selected = None;
        match &result[index] {
            Some(Ok(Some(delay))) => {
                ui.monospace(display_value(delay.seconds, "s"));
                for (label, points) in ["Low edge", "High edge"].into_iter().zip(delay.edges) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(label);
                        selected = rows(ui, &points).or(selected);
                    });
                }
            }
            Some(Ok(None)) => {
                ui.weak("Continuous measured phase unavailable");
            }
            Some(Err(error)) => read_error(ui, error),
            None => {}
        }
        selected
    })
}

fn sweep_columns(
    ui: &mut Ui,
    order: [usize; 2],
    label: &str,
    mut body: impl FnMut(&mut Ui, usize) -> Option<f64>,
) -> Option<f64> {
    let colors = sweep_colors(ui);
    let mut selected = None;
    ui.columns(2, |columns| {
        for (slot, index) in order.into_iter().enumerate() {
            let ui = &mut columns[slot];
            ui.colored_label(colors[slot], format!("{} {label}", letter(slot)));
            selected = body(ui, index).or(selected);
        }
    });
    selected
}

fn rows(ui: &mut Ui, points: &[TracePoint; 2]) -> Option<f64> {
    let mut jump = None;
    ui.horizontal_wrapped(|ui| {
        for point in points
            .iter()
            .enumerate()
            .filter(|(index, point)| *index == 0 || point.index != points[0].index)
            .map(|(_, point)| *point)
        {
            if ui
                .small_button(format!(
                    "Row {} · {}",
                    point.index + 1,
                    display_value(point.frequency_hz, "Hz")
                ))
                .on_hover_text(row_tip(point))
                .clicked()
            {
                jump = Some(point.frequency_hz);
            }
        }
    });
    jump
}

fn ordered<T, U>(
    values: Option<&Pair<T>>,
    [a, b]: [usize; 2],
    read: impl Fn(&T) -> Option<U>,
) -> Option<[U; 2]> {
    let values = values?;
    let side = |slot: usize| read(values[slot].as_ref()?.as_ref().ok()?);
    Some([side(a)?, side(b)?])
}

fn response<'a, T>(ui: &mut Ui, result: Outcome<'a, T>, pending: &str) -> Option<&'a Pair<T>> {
    match result {
        Some(Ok(result)) => Some(result),
        Some(Err(error)) => {
            read_error(ui, error);
            None
        }
        None => {
            ui.weak(pending);
            None
        }
    }
}

fn read_error(ui: &mut Ui, error: &str) {
    ui.colored_label(ui.visuals().error_fg_color, error);
}

fn span([lo, hi]: Band) -> String {
    format!("{} to {}", display_value(lo, "Hz"), display_value(hi, "Hz"))
}

pub(super) fn letter(slot: usize) -> &'static str {
    ["A", "B"][slot]
}

fn row_tip(point: TracePoint) -> String {
    format!(
        "Source row {} · {} · {}",
        point.index + 1,
        display_value(point.frequency_hz, "Hz"),
        rectangular(point.value)
    )
}

fn rectangular(value: Complex64) -> String {
    format!("({:.6}{:+.6}j)", value.re, value.im)
}

fn unit(projection: Projection, magnitude: &str) -> String {
    match (projection, magnitude) {
        (Projection::Decibels, _) => " dB".to_owned(),
        (Projection::Phase, _) => "°".to_owned(),
        (_, "") => String::new(),
        (_, unit) => format!(" {unit}"),
    }
}
