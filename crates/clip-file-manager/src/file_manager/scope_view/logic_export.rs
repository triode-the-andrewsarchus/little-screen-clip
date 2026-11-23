use super::{Viewer, math};
use crate::file_manager::preview_copy::{self, Copies};
use egui::Ui;
use little_clip_core::engineering::{analysis::Expression, waveforms::Recording};
use std::{
    io::{BufWriter, Write},
    ops::Range,
    sync::atomic::AtomicBool,
};

impl Viewer {
    pub(super) fn predicate_actions(
        &self,
        ui: &mut Ui,
        copies: &mut Copies,
        can_add_equation: bool,
    ) -> bool {
        let Some(expression) = self.math_state.predicate() else {
            return false;
        };
        let promote = ui
            .add_enabled(can_add_equation, egui::Button::new("Add digital lane"))
            .on_hover_text("Use this condition as a digital input for equations and decoders")
            .on_disabled_hover_text("Finish the current equation edit first")
            .clicked();
        let selection = self.selection();
        if ui
            .add_enabled(!copies.busy(), egui::Button::new("Copy VCD"))
            .on_hover_text(if selection.is_some() {
                "Copy this condition across the selected time range"
            } else {
                "Copy this condition across the visible time range"
            })
            .clicked()
        {
            let recording = self.recording.clone();
            let request = Request {
                channel: self.active,
                span: selection.unwrap_or_else(|| self.span.clone()),
                expression: expression.clone(),
            };
            copies.start(ui.ctx(), move |cancel| {
                preview_copy::copy_file(recording.source().path(), "logic", "vcd", |output| {
                    request.write(&recording, output, cancel)
                })?;
                Ok("Copied thresholded logic capture".into())
            });
        }
        promote
    }
}

struct Request {
    channel: usize,
    span: Range<f64>,
    expression: Expression,
}

impl Request {
    fn write(
        &self,
        recording: &Recording,
        output: impl Write,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let mut output = BufWriter::new(output);
        let interval = recording.channels()[self.channel].interval_seconds;
        let (scale, unit) = if interval >= 1e-12 {
            (1e12, "ps")
        } else {
            (1e15, "fs")
        };
        writeln!(output, "$timescale 1{unit} $end\n$scope module Scope $end\n$var wire 1 ! Math $end\n$upscope $end\n$enddefinitions $end").map_err(message)?;
        let mut previous = None;
        let end = recording.checked(cancel, |capture| {
            math::exact_predicate(
                capture,
                self.channel,
                self.span.clone(),
                &self.expression,
                cancel,
                |time, state| {
                    let level = match state {
                        Some(true) => '1',
                        Some(false) => '0',
                        None => 'x',
                    };
                    if previous != Some(level) {
                        previous = Some(level);
                        writeln!(output, "#{}\n{level}!", (time * scale).round() as u64)
                            .map_err(message)?;
                    }
                    Ok(())
                },
            )
        })?;
        writeln!(output, "#{}", (end * scale).ceil() as u64).map_err(message)?;
        output.flush().map_err(message)
    }
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_source::ReadCheck;
    use little_clip_core::engineering::logic::{Capture, Level};

    #[test]
    fn waveform_predicate_emits_a_readable_transition_exact_vcd() {
        let mut sample = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        sample.write_all(b"X,CH1,Start,Increment,\nSequence,Volt,-0.000002,0.000001\n0,0,\n1,2,\n2,2,\n3,0,\n4,0,\n5,2,\n").unwrap();
        let cancel = AtomicBool::new(false);
        let recording = Recording::open(sample.path(), ReadCheck::default(), &cancel).unwrap();
        let request = Request {
            channel: 0,
            span: -2e-6..4e-6,
            expression: Expression::parse("CH1 > 1V", &["CH1"], 0).unwrap(),
        };
        let mut vcd = tempfile::Builder::new().suffix(".vcd").tempfile().unwrap();
        request.write(&recording, &mut vcd, &cancel).unwrap();
        let capture = Capture::open(std::fs::File::open(vcd.path()).unwrap(), vcd.path()).unwrap();
        let edges = capture.read_edges(0, 0.0..6e-6, 20, &cancel).unwrap();
        assert_eq!(edges.initial, Some(Level::Low));
        let levels = edges
            .edges
            .iter()
            .map(|edge| edge.level)
            .collect::<Vec<_>>();
        assert_eq!(levels, vec![Level::High, Level::Low, Level::High]);
        for (edge, time) in edges.edges.iter().zip([1e-6, 3e-6, 5e-6]) {
            assert!((edge.time_seconds - time).abs() <= 1e-12);
        }
    }
}
