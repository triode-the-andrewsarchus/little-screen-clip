mod provider;

use super::{Task, event_list, protocols::Source};
use crate::file_manager::signal_view::timeline;
use egui::{Context, Ui};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::Clock,
        logic::Lane,
        waveforms::{display_axis_value, display_value},
    },
};
use provider::{Catalog, Decoder, Output, Request};
use std::{ops::Range, sync::LazyLock, time::Duration};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));
const DEADLINE: Duration = Duration::from_secs(24);

type Pending<T> = Option<Task<Result<T, String>>>;

#[derive(Default)]
pub(in crate::file_manager) struct Panel {
    pub active: bool,
    catalog: Option<Result<Catalog, String>>,
    probe: Pending<Catalog>,
    decode: Pending<Output>,
    output: Option<Result<Output, String>>,
    decoder: Option<usize>,
    decoder_search: String,
    pins: Vec<Option<usize>>,
    rate: String,
    picked: Option<usize>,
    followed: Option<usize>,
    origin: Option<View>,
}

pub(in crate::file_manager) struct View {
    pub span: Range<f64>,
    pub selection: Option<Range<f64>>,
    pub cursor: Option<f64>,
}

impl Panel {
    pub fn open(&mut self, ctx: &Context) {
        self.active = true;
        if self.catalog.is_none() && self.probe.is_none() {
            self.probe = Some(Task::start(&JOBS, DEADLINE, provider::catalog));
            ctx.request_repaint();
        }
    }

    pub fn close(&mut self) -> Option<View> {
        self.active = false;
        self.probe = None;
        self.invalidate()
    }

    pub fn invalidate(&mut self) -> Option<View> {
        self.decode = None;
        self.output = None;
        self.picked = None;
        self.followed = None;
        self.origin.take()
    }

    pub fn channels(&self) -> impl Iterator<Item = usize> + '_ {
        self.pins.iter().flatten().copied()
    }

    pub fn controls(
        &mut self,
        ui: &mut Ui,
        lanes: &[Lane],
        source: impl FnOnce() -> Source,
        span: &Range<f64>,
        selection: Option<&Range<f64>>,
    ) {
        deliver(ui.ctx(), &mut self.probe, &mut self.catalog);
        deliver(ui.ctx(), &mut self.decode, &mut self.output);
        let catalog = match catalog_controls(ui, self.catalog.as_ref()) {
            Ok(Some(catalog)) => catalog,
            Ok(None) => return,
            Err(()) => {
                self.catalog = None;
                self.open(ui.ctx());
                return;
            }
        };
        if self.origin.is_some() {
            return;
        }
        let changed = decoder_choice(ui, catalog, &mut self.decoder, &mut self.decoder_search);
        let Some(decoder) = self.decoder.and_then(|index| catalog.decoders.get(index)) else {
            return;
        };
        if changed {
            self.pins = vec![None; decoder.pins.len()];
        }
        if decoder.defaults {
            ui.weak("Using decoder defaults");
        }
        let wiring_changed = pin_choices(ui, decoder, &mut self.pins, lanes);
        let mapped = decoder
            .pins
            .iter()
            .zip(&self.pins)
            .all(|(pin, lane)| !pin.required || lane.is_some());
        let native = provider::native_clock(lanes, self.pins.iter().flatten().copied());
        let (rate_changed, requested, cancelled) = decode_controls(
            ui,
            native,
            &mut self.rate,
            mapped,
            selection.is_some(),
            self.decode.is_some(),
        );
        if changed || wiring_changed || rate_changed || requested {
            self.decode = None;
            self.output = None;
            self.picked = None;
            self.followed = None;
        }
        if cancelled {
            self.decode = None;
        }
        if requested {
            let request = Request {
                id: decoder.id.clone(),
                version: catalog.version.clone(),
                pins: decoder
                    .pins
                    .iter()
                    .zip(&self.pins)
                    .map(|(pin, lane)| (pin.id.clone(), *lane))
                    .collect(),
                range: selection.unwrap_or(span).clone(),
                grid_hz: provider::parse_rate(&self.rate),
            };
            let source = source();
            let lanes = lanes.to_vec();
            self.decode = Some(Task::start(&JOBS, DEADLINE, move |cancel| {
                provider::decode(&source, &lanes, request, cancel)
            }));
            ui.ctx().request_repaint();
        }
    }

    pub fn return_controls(&mut self, ui: &mut Ui) -> Option<View> {
        if self.origin.is_some() && ui.button("Back to decoded window").clicked() {
            return self.origin.take();
        }
        None
    }

    pub fn table(&mut self, ui: &mut Ui, clock: Clock) -> Option<(Range<f64>, f64)> {
        if self.decode.is_some() {
            ui.weak("Decoding the selected logic samples…");
        }
        let output = match self.output.as_ref()? {
            Ok(output) => output,
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return None;
            }
        };
        let step = 1.0 / output.rate as f64;
        let timestamp =
            |time| display_axis_value(clock.to_workspace(time), step * clock.scale(), "s");
        let parts = coverage(output);
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!(
                "{}–{} · {}",
                timestamp(output.range.start),
                timestamp(output.range.end),
                parts.join(" · "),
            ));
        });
        if output.annotations.is_empty() {
            ui.weak("No annotations in the decoded samples");
            return None;
        }
        let row = event_list(
            ui,
            ("sigrok-annotation-rows", "Sigrok annotation"),
            &mut self.followed,
            self.picked,
            output.annotations.len(),
            |index| {
                let event = &output.annotations[index];
                (timestamp(event.time.start), event.text.clone())
            },
        )?;
        self.picked = Some(row);
        Some((output.annotations[row].time.clone(), step))
    }

    pub fn visit(
        &mut self,
        origin: View,
        time: &Range<f64>,
        domain: &Range<f64>,
        step: f64,
    ) -> Range<f64> {
        self.origin.get_or_insert(origin);
        let padding = ((time.end - time.start) * 0.18).max(step * 2.0);
        timeline::fit(domain, time.start - padding..time.end + padding, step)
    }
}

fn catalog_controls<'a>(
    ui: &mut Ui,
    status: Option<&'a Result<Catalog, String>>,
) -> Result<Option<&'a Catalog>, ()> {
    let detail = match status {
        Some(Ok(catalog)) if !catalog.decoders.is_empty() => return Ok(Some(catalog)),
        Some(Ok(_)) => "No supported logic decoders were found in the installed catalog.",
        Some(Err(error)) => error,
        None => {
            ui.weak("Finding installed Sigrok decoders…");
            return Ok(None);
        }
    };
    let retry = ui
        .horizontal_wrapped(|ui| {
            ui.weak("Sigrok unavailable").on_hover_text(detail);
            ui.button("Retry").clicked()
        })
        .inner;
    if retry { Err(()) } else { Ok(None) }
}

fn decoder_choice(
    ui: &mut Ui,
    catalog: &Catalog,
    selected: &mut Option<usize>,
    search: &mut String,
) -> bool {
    let title = selected
        .and_then(|index| catalog.decoders.get(index))
        .map_or("Sigrok decoder", |entry| &entry.name);
    egui::ComboBox::from_id_salt("sigrok-decoder")
        .selected_text(title)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(search).hint_text("Search decoders"));
            let query = search.trim().to_lowercase();
            let mut changed = false;
            for (index, decoder) in catalog.decoders.iter().enumerate() {
                if decoder.name.to_lowercase().contains(&query)
                    || decoder.id.to_lowercase().contains(&query)
                {
                    let response = ui.selectable_value(selected, Some(index), &decoder.name);
                    if response.changed() {
                        changed = true;
                        ui.close();
                    }
                }
            }
            if catalog.omitted > 0 {
                ui.weak(format!(
                    "{} decoders omitted by the catalog limit",
                    catalog.omitted
                ));
            }
            changed
        })
        .inner
        .unwrap_or_default()
}

fn decode_controls(
    ui: &mut Ui,
    native: Option<(u64, f64)>,
    grid_hz: &mut String,
    mapped: bool,
    has_selection: bool,
    running: bool,
) -> (bool, bool, bool) {
    ui.horizontal_wrapped(|ui| {
        let changed = if let Some((rate, _)) = native {
            ui.weak(format!(
                "Recorded clock · {}",
                display_value(rate as f64, "Hz")
            ));
            false
        } else {
            ui.label("Sample grid (Hz)");
            ui.add(
                egui::TextEdit::singleline(grid_hz)
                    .desired_width(85.0)
                    .char_limit(11),
            )
            .on_hover_text(
                "Choose a sampling rate for timestamped or differently clocked channels.",
            )
            .changed()
        };
        let label = if has_selection {
            "Decode selection"
        } else {
            "Decode visible window"
        };
        let enabled = mapped && (native.is_some() || provider::parse_rate(grid_hz).is_some());
        let requested = ui
            .add_enabled(enabled, egui::Button::new(label))
            .on_disabled_hover_text("Map the required pins and provide a sample grid if needed.")
            .clicked();
        let cancelled = running && ui.button("Cancel decode").clicked();
        (changed, requested, cancelled)
    })
    .inner
}

fn coverage(output: &Output) -> Vec<String> {
    let source = if output.resampled {
        "Sample grid"
    } else {
        "Recorded clock"
    };
    let mut parts = vec![format!(
        "{source} {}",
        display_value(output.rate as f64, "Hz")
    )];
    if output.partial {
        parts.push("Partial window".into());
    }
    if output.off_grid > 0 {
        parts.push(format!(
            "{} off-grid transitions rounded forward",
            output.off_grid
        ));
    }
    if output.gaps > 0 {
        parts.push(format!("{} samples omitted at gaps", output.gaps));
    }
    if output.omitted > 0 {
        parts.push(format!("{} annotations omitted", output.omitted));
    }
    parts
}

fn deliver<T: Send + 'static>(
    ctx: &Context,
    job: &mut Pending<T>,
    target: &mut Option<Result<T, String>>,
) {
    let Some(pending) = job else { return };
    if let Some(result) = pending.poll() {
        *target = Some(
            result
                .map_err(|error| error.to_string())
                .and_then(|value| value),
        );
        *job = None;
    } else {
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}

fn choice<'a>(
    ui: &mut Ui,
    id: &str,
    label: &str,
    selected: &mut Option<usize>,
    entries: impl Iterator<Item = &'a str> + Clone,
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(
            selected
                .and_then(|index| entries.clone().nth(index))
                .unwrap_or(label),
        )
        .show_ui(ui, |ui| {
            for (index, text) in entries.enumerate() {
                changed |= ui.selectable_value(selected, Some(index), text).changed();
            }
        });
    changed
}

fn pin_choices(ui: &mut Ui, decoder: &Decoder, pins: &mut [Option<usize>], lanes: &[Lane]) -> bool {
    ui.horizontal_wrapped(|ui| {
        let mut changed = false;
        for (index, (pin, lane)) in decoder.pins.iter().zip(pins).enumerate() {
            let label = if pin.required {
                pin.name.clone()
            } else {
                format!("{} (optional)", pin.name)
            };
            ui.label(&label);
            ui.push_id(("sigrok-pin", index), |ui| {
                let items = lanes.iter().map(|lane| lane.name.as_str());
                changed |= choice(ui, "lane", "Unmapped", lane, items);
                if !pin.required && lane.is_some() && ui.small_button("Clear").clicked() {
                    *lane = None;
                    changed = true;
                }
            });
        }
        changed
    })
    .inner
}
