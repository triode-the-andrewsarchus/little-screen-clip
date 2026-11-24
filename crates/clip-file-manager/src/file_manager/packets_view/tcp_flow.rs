use super::{
    Capture, JOBS,
    capture::{TcpFlow, TcpFlowQuery},
    conversations::Selection,
};
use crate::{
    file_manager::{
        capture_browse::{Job, keyed_poll, start},
        preview_copy::Copies,
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Context, Ui};
use little_clip_core::engineering::{
    packets::{AckDelay, Index, SequencePosition, TcpPoint},
    waveforms::display_value,
};
use std::{fmt::Write, sync::Arc, time::Duration};

mod plot;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Sequence,
    Delay,
}

pub(super) struct View {
    pub open: bool,
    mode: Mode,
    acks: bool,
    connection: u32,
    selected: Option<u64>,
    pending: Option<(TcpFlowQuery, Job<TcpFlow>)>,
    ready: Option<(TcpFlowQuery, Result<Arc<TcpFlow>, String>)>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            open: false,
            mode: Mode::default(),
            acks: true,
            connection: 0,
            selected: None,
            pending: None,
            ready: None,
        }
    }
}

impl View {
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn suspend(&mut self) {
        self.pending = None;
    }

    pub fn poll(&mut self) {
        if let Some((query, result)) = keyed_poll(&mut self.pending) {
            self.connection = 0;
            self.selected = None;
            self.ready = Some((query, result.map(Arc::new)));
        }
    }

    pub(super) fn request(
        &mut self,
        ctx: &Context,
        capture: &Arc<Capture>,
        index: &Arc<Index>,
        selection: &Selection,
    ) {
        let query = key(selection);
        if self
            .ready
            .as_ref()
            .is_some_and(|(ready, _)| *ready == query)
        {
            self.pending = None;
            return;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|(pending, _)| *pending == query)
        {
            return;
        }
        let capture = capture.clone();
        let index = index.clone();
        let requested = query.clone();
        self.pending = Some((
            query,
            start(&JOBS, ctx, Duration::from_secs(45), move |cancel| {
                capture.tcp_flow(&index, &requested, cancel)
            }),
        ));
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        selection: &Selection,
        selected: Option<u64>,
        origin: Option<i128>,
        colors: WorkbenchColors,
        copies: &mut Copies,
    ) -> Option<u64> {
        let query = key(selection);
        ui.separator();
        let Some((_, result)) = self.ready.as_ref().filter(|(key, _)| *key == query) else {
            ui.weak("Reading TCP sequence fields…");
            return None;
        };
        let flow = match result {
            Ok(flow) => flow.clone(),
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.small_button("Retry").clicked() {
                    self.ready = None;
                    ui.ctx().request_repaint();
                }
                return None;
            }
        };
        let Some(last) = flow.points.last() else {
            ui.weak("No complete TCP base headers in this conversation range.");
            return None;
        };
        if self.selected != selected
            && let Some(point) = selected.and_then(|row| find_row(&flow.points, row))
        {
            self.connection = point.connection;
        }
        self.selected = selected;
        self.controls(ui, &flow, last.connection, origin, copies);
        if flow.scanned < query.scope.total {
            ui.weak(format!(
                "{} of {} source frames inspected · select a narrower range for more",
                flow.scanned, query.scope.total
            ));
        }
        let points = connection(&flow.points, self.connection);
        let unknown_time = points
            .iter()
            .filter(|point| point.time_ns.is_none())
            .count();
        if unknown_time > 0 {
            ui.weak(format!("{unknown_time} frames without timestamps"));
        }
        let measured = if self.mode == Mode::Delay {
            related_ack(points, selected)
        } else {
            None
        };
        let marker = measured.map(|(ack, _)| ack.row).or(selected);
        let clicked = plot::draw(ui, points, marker, origin, colors, self.mode, self.acks);
        if self.mode == Mode::Delay {
            return clicked.or(delay_sources(ui, measured));
        }
        clicked
    }

    fn controls(
        &mut self,
        ui: &mut Ui,
        flow: &Arc<TcpFlow>,
        last: u32,
        origin: Option<i128>,
        copies: &mut Copies,
    ) {
        ui.horizontal_wrapped(|ui| {
            clip_ui::buttons::choice(ui, &mut self.mode, Mode::Sequence, "TCP sequence").on_hover_text(
                "Each direction measures sequence bytes from its first observed sequence.",
            );
            clip_ui::buttons::choice(ui, &mut self.mode, Mode::Delay, "ACK delay").on_hover_text(
                "Time from a recorded segment to its exact cumulative ACK on the same interface. Overlapping sends retire ambiguous pairs.",
            );
            if last > 64 {
                let mut current = self.connection + 1;
                ui.add(egui::DragValue::new(&mut current).range(1..=last + 1)
                    .prefix("Connection ").suffix(format!(" / {}", last + 1)));
                self.connection = current - 1;
            } else if last != 0 {
                egui::ComboBox::from_id_salt("tcp-flow-connection")
                    .selected_text(format!("Connection {}", self.connection + 1))
                    .show_ui(ui, |ui| {
                        for session in 0..=last {
                            ui.selectable_value(
                                &mut self.connection,
                                session,
                                format!("Connection {}", session + 1),
                            );
                        }
                    });
            }
            if self.mode == Mode::Sequence {
                ui.checkbox(&mut self.acks, "Acknowledgments");
            }
            if ui
                .add_enabled(!copies.busy(), egui::Button::new("Copy TCP CSV"))
                .clicked()
            {
                let flow = flow.clone();
                let connection = self.connection;
                copies.start(ui.ctx(), move |_| copy(&flow, connection, origin));
            }
        });
    }
}

fn key(selection: &Selection) -> TcpFlowQuery {
    TcpFlowQuery {
        key: selection.key,
        scope: selection.scope.clone(),
    }
}

fn connection(points: &[TcpPoint], index: u32) -> &[TcpPoint] {
    let start = points.partition_point(|point| point.connection < index);
    let end = points.partition_point(|point| point.connection <= index);
    &points[start..end]
}

fn find_row(points: &[TcpPoint], row: u64) -> Option<&TcpPoint> {
    let index = points.binary_search_by_key(&row, |point| point.row).ok()?;
    points.get(index)
}

fn delay_sources(ui: &mut Ui, measured: Option<(&TcpPoint, AckDelay)>) -> Option<u64> {
    ui.horizontal_wrapped(|ui| {
        let Some((point, delay)) = measured else {
            ui.weak("Select a delay point to inspect its two frames.");
            return None;
        };
        let sent = ui
            .button(format!("Segment frame {}", delay.segment_row + 1))
            .clicked();
        ui.monospace(display_value(delay.nanoseconds as f64 / 1e9, "s"));
        let ack = ui.button(format!("ACK frame {}", point.row + 1)).clicked();
        sent.then_some(delay.segment_row)
            .or_else(|| ack.then_some(point.row))
    })
    .inner
}

fn related_ack(points: &[TcpPoint], selected: Option<u64>) -> Option<(&TcpPoint, AckDelay)> {
    let row = selected?;
    if let Some(point) = find_row(points, row)
        && let Some(delay) = point.ack_delay
    {
        return Some((point, delay));
    }
    points.iter().find_map(|point| {
        let delay = point.ack_delay.filter(|delay| delay.segment_row == row)?;
        Some((point, delay))
    })
}

fn note(position: SequencePosition) -> &'static str {
    match position {
        SequencePosition::Unknown => "",
        SequencePosition::AtPreviousEnd => "At previous high-water mark",
        SequencePosition::AbovePreviousEnd => "Above previous high-water mark",
        SequencePosition::BelowPreviousEnd => "Below previous high-water mark",
    }
}

fn copy(flow: &TcpFlow, selected: u32, origin: Option<i128>) -> Result<String, String> {
    let points = connection(&flow.points, selected);
    let mut output = String::from(
        "frame,time_from_capture_s,sender,sequence_raw,sequence_relative,sequence_end_relative,\
         acknowledgment_raw,acknowledgment_relative,declared_payload_bytes,raw_window,flags,sequence_position,ack_segment_frame,ack_delay_ns\n",
    );
    for point in points {
        let segment = point.segment;
        let seconds = blank(
            point
                .time_ns
                .zip(origin)
                .map(|(time, start)| (time - start) as f64 / 1e9),
        );
        let _ = writeln!(
            output,
            "{},{seconds},{},{},{},{},{},{},{},{},{:#x},{},{},{}",
            point.row + 1,
            ['A', 'B'][point.sender],
            segment.sequence,
            blank(point.sequence),
            blank(point.sequence_end),
            segment.acknowledgment,
            blank(point.acknowledged),
            blank(segment.payload_len),
            segment.advertised_window,
            segment.flags,
            note(point.position),
            blank(point.ack_delay.map(|delay| delay.segment_row + 1)),
            blank(point.ack_delay.map(|delay| delay.nanoseconds)),
        );
    }
    little_clip_core::copy_text_to_system_clipboard(&output).map_err(|error| error.to_string())?;
    Ok(format!("Copied {} TCP frames", points.len()))
}

fn blank(number: Option<impl ToString>) -> String {
    number.map_or_else(String::new, |number| number.to_string())
}
