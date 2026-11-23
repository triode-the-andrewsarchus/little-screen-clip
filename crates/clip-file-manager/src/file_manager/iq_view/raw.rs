use egui::Ui;
use little_clip_core::engineering::iq::{Encoding, RawInterpretation};
use std::path::Path;

pub(super) struct Fields {
    encoding: Option<Encoding>,
    sample_rate: String,
    center: String,
}

impl Fields {
    pub(super) fn for_path(path: &Path) -> Self {
        let extension = path.extension().and_then(|value| value.to_str());
        let encoding = match extension.map(str::to_ascii_lowercase).as_deref() {
            Some("cu8") => Some(Encoding::Cu8),
            Some("cs8" | "ci8") => Some(Encoding::Ci8),
            _ => None,
        };
        Self {
            encoding,
            sample_rate: String::new(),
            center: String::new(),
        }
    }

    pub(super) fn show(
        &mut self,
        ui: &mut Ui,
        applying: bool,
    ) -> Option<Result<RawInterpretation, String>> {
        let mut apply = false;
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("raw_iq_encoding")
                .selected_text(
                    self.encoding
                        .map_or("Choose sample encoding", Encoding::name),
                )
                .show_ui(ui, |ui| {
                    for encoding in Encoding::ALL {
                        ui.selectable_value(&mut self.encoding, Some(encoding), encoding.name());
                    }
                });
            ui.label("Sample rate");
            ui.add(
                egui::TextEdit::singleline(&mut self.sample_rate)
                    .id_salt("raw_iq_rate")
                    .hint_text("Hz, optional")
                    .desired_width(99.0),
            );
            ui.label("Recorded center");
            ui.add(
                egui::TextEdit::singleline(&mut self.center)
                    .id_salt("raw_iq_center")
                    .hint_text("Hz, optional")
                    .desired_width(99.0),
            );
            apply = ui
                .add_enabled(
                    !applying && self.encoding.is_some(),
                    egui::Button::new("Apply"),
                )
                .clicked();
            if applying {
                ui.weak("Reading recording…");
            }
        });
        apply.then(|| self.interpret())
    }

    fn interpret(&self) -> Result<RawInterpretation, String> {
        let encoding = self.encoding.ok_or("Choose the sample encoding.")?;
        let sample_rate_hz = optional_number(&self.sample_rate, "sample rate")?;
        if sample_rate_hz.is_some_and(|rate| rate <= 0.0) {
            return Err("Sample rate must be greater than zero.".into());
        }
        let center_hz = optional_number(&self.center, "center frequency")?;
        Ok(RawInterpretation {
            encoding,
            sample_rate_hz,
            center_hz,
        })
    }
}

fn optional_number(value: &str, field: &str) -> Result<Option<f64>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(Some)
        .ok_or_else(|| format!("Enter a valid {field} in Hz."))
}
