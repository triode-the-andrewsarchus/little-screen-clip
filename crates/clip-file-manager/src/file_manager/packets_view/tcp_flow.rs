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
use little_clip_core::engineering::packets::{Index, SequencePosition, TcpPoint};
use std::{fmt::Write, sync::Arc, time::Duration};

mod plot;

pub(super) struct View {
    pub open: bool,
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
            Ok(flow) => flow,
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
        ui.horizontal_wrapped(|ui| {
            ui.strong("TCP sequence").on_hover_text(
                "Each direction measures sequence bytes from its first observed sequence.",
            );
            if last.connection != 0 {
                egui::ComboBox::from_id_salt("tcp-flow-connection")
                    .selected_text(format!("Connection {}", self.connection + 1))
                    .show_ui(ui, |ui| {
                        for session in 0..=last.connection {
                            ui.selectable_value(
                                &mut self.connection,
                                session,
                                format!("Connection {}", session + 1),
                            );
                        }
                    });
            }
            ui.checkbox(&mut self.acks, "Acknowledgments");
            if ui
                .add_enabled(!copies.busy(), egui::Button::new("Copy TCP CSV"))
                .clicked()
            {
                let flow = flow.clone();
                let connection = self.connection;
                copies.start(ui.ctx(), move |_| copy(&flow, connection, origin));
            }
        });
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
        plot::draw(ui, points, self.acks, selected, origin, colors)
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
         acknowledgment_raw,acknowledgment_relative,declared_payload_bytes,raw_window,flags,sequence_position\n",
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
            "{},{seconds},{},{},{},{},{},{},{},{},{:#x},{}",
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
        );
    }
    little_clip_core::copy_text_to_system_clipboard(&output).map_err(|error| error.to_string())?;
    Ok(format!("Copied {} TCP frames", points.len()))
}

fn blank(number: Option<impl ToString>) -> String {
    number.map_or_else(String::new, |number| number.to_string())
}
