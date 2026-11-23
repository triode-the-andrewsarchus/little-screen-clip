use super::{JOBS, Task, Viewer, math};
use crate::file_manager::{
    logic_view,
    preview_copy::{self, Copies},
};
use egui::Ui;
use little_clip_core::{
    engineering::{analysis::Expression, waveforms::Recording},
    palette::AppPalette,
    staging::{ArtifactStore, TemporaryWorkspace},
};
use std::{
    io::{BufWriter, Write},
    ops::Range,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::Duration,
};

#[derive(Default)]
pub(super) struct Conversion {
    pending: Option<Task<Result<Converted, String>>>,
    opened: Option<Converted>,
    error: Option<String>,
}

struct Converted {
    view: logic_view::Viewer,
    _workspace: TemporaryWorkspace,
}

impl Viewer {
    pub(super) fn logic_subview(
        &mut self,
        ui: &mut Ui,
        palette: &AppPalette,
        copies: &mut Copies,
    ) -> bool {
        if let Some(result) = self.conversion.pending.as_ref().and_then(Task::poll) {
            self.conversion.pending = None;
            match result
                .map_err(|err| err.to_string())
                .and_then(|result| result)
            {
                Ok(view) => self.conversion.opened = Some(view),
                Err(error) => self.conversion.error = Some(error),
            }
        }
        if self.conversion.pending.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(25));
        }
        let Some(converted) = &mut self.conversion.opened else {
            return false;
        };
        let mut back = ui.button("Back to waveform").clicked()
            || (!egui::Popup::is_any_open(ui.ctx())
                && ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                }));
        let mut shell = crate::shell::StudioShell::new(
            "Logic from math",
            crate::icons::Icon::Waves,
            palette,
            &mut back,
        );
        shell.render_header(ui, |_| {});
        converted.view.content(ui, copies, None);
        if back {
            self.conversion.opened = None;
        }
        true
    }

    pub(super) fn logic_menu(&mut self, ui: &mut Ui, copies: &mut Copies) {
        if let Some(error) = &self.conversion.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.conversion.pending.is_some() {
            ui.spinner();
        }
        let Some(expression) = self.math_state.predicate() else {
            return;
        };
        let action = ui
            .menu_button("Logic from math", |ui| {
                let open = ui
                    .add_enabled(
                        self.conversion.pending.is_none(),
                        egui::Button::new("Inspect in logic viewer"),
                    )
                    .clicked();
                let copy = ui
                    .add_enabled(!copies.busy(), egui::Button::new("Copy as VCD"))
                    .clicked();
                if open || copy {
                    ui.close();
                }
                (open, copy)
            })
            .inner;
        let Some((open, copy)) = action else { return };
        if !open && !copy {
            return;
        }
        let recording = self.recording.clone();
        let request = Request {
            channel: self.active,
            span: self.selection().unwrap_or_else(|| self.span.clone()),
            expression: expression.clone(),
        };
        self.conversion.error = None;
        if open {
            let repaint = ui.ctx().clone();
            self.conversion.pending =
                Some(Task::start(&JOBS, Duration::from_secs(45), move |cancel| {
                    let result = convert(&recording, request, cancel);
                    repaint.request_repaint();
                    result
                }));
        } else {
            copies.start(ui.ctx(), move |cancel| {
                preview_copy::copy_file(recording.source().path(), "logic", "vcd", |output| {
                    request.write(&recording, output, cancel)
                })?;
                Ok("Copied thresholded logic capture".into())
            });
        }
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

fn convert(
    recording: &Recording,
    request: Request,
    cancel: &AtomicBool,
) -> Result<Converted, String> {
    let workspace = ArtifactStore::new()
        .map_err(message)?
        .create_temporary_workspace("scope-logic", "threshold")
        .map_err(message)?;
    let directory = little_clip_core::kernel_interface::pin_content_directory(workspace.path())
        .map_err(message)?;
    let confined =
        PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join("math.vcd");
    let output = std::fs::File::create_new(confined).map_err(message)?;
    request.write(recording, output, cancel)?;
    let view = logic_view::Viewer::load(&workspace.path().join("math.vcd"), cancel)?;
    Ok(Converted {
        view,
        _workspace: workspace,
    })
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
