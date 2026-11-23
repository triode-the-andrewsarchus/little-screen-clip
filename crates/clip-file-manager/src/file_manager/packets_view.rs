//! Linked, read-only views of a packet capture and its source records.

mod activity;
mod capture;
mod inspect;
pub(super) mod strip;
mod table;

use super::{
    capture_browse::{
        Job, PAGE_ROWS, keyed_poll, poll,
        rows::{RowWindows, needed_window},
        start,
        table::Page,
    },
    preview_copy::{self, Copies},
};
use crate::{file_source::ReadCheck, ui_appearance::WorkbenchColors};
use capture::{Capture, Filter, Filtered};
use egui::{Context, Id, Layout, PointerButton, Sense, Ui, vec2};
use little_clip_core::{
    BackgroundJobs,
    engineering::packets::{ActivityBin, Compression, Index, Packet, PacketRow, Protocol, Summary},
    palette::AppPalette,
};
use std::{
    ops::Range,
    path::Path,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(3, 5));
pub(super) struct Viewer {
    capture: Arc<Capture>,
    summary: Summary,
    initial_bins: Vec<ActivityBin>,
    index: Option<Arc<Index>>,
    index_job: Option<Job<Index>>,
    index_failed: bool,
    page: u64,
    scroll_to: Option<usize>,
    generation: u64,
    packet_number: u64,
    windows: RowWindows<PacketRow>,
    selected: Option<u64>,
    range: Option<Range<u64>>,
    anchor: Option<u64>,
    brushed: Option<activity::Bounds>,
    packet: Option<Packet>,
    packet_job: Option<(u64, Job<Packet>)>,
    packet_failed: Option<u64>,
    filter_protocol: Option<Protocol>,
    endpoint: String,
    filter: Option<Filter>,
    filtered: Option<Filtered>,
    filter_job: Option<Job<Filtered>>,
    error: Option<String>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (capture, summary) = Capture::open(path, cancel)?;
        let initial_bins = summary.activity();
        let selected = summary.packets.first().map(|row| row.header.row);
        Ok(Self {
            capture: Arc::new(capture),
            summary,
            initial_bins,
            index: None,
            index_job: None,
            index_failed: false,
            page: 0,
            generation: 0,
            scroll_to: None,
            packet_number: 1,
            windows: RowWindows::new(3),
            selected,
            range: None,
            anchor: selected,
            brushed: None,
            packet: None,
            packet_job: None,
            packet_failed: None,
            filter_protocol: None,
            endpoint: String::new(),
            filter: None,
            filtered: None,
            filter_job: None,
            error: None,
        })
    }

    pub fn set_check(&mut self, check: ReadCheck) {
        if let Some(capture) = Arc::get_mut(&mut self.capture) {
            capture.check = check;
        }
    }

    pub fn focus_packet(&mut self, ordinal: u64) {
        self.select(ordinal, false);
        self.reveal(ordinal);
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette, copies: &mut Copies) {
        self.poll();
        let colors = WorkbenchColors::from_palette(palette);
        let mut shell = crate::shell::StudioShell::new(
            "Packet capture",
            crate::icons::Icon::Crosshair,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.controls(ui, copies);
        self.timeline(ui, colors);
        self.navigation(ui);
        self.browser(ui, colors);
        self.request_index(ui.ctx());
        self.request_packet(ui.ctx());
        if self.pending() {
            ui.ctx().request_repaint_after(Duration::from_millis(24));
        }
    }

    fn controls(&mut self, ui: &mut Ui, copies: &mut Copies) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(capture::name(self.summary.format));
            if let Some(index) = &self.index {
                ui.weak(format!(
                    "{} packets · {} interfaces",
                    index.points(),
                    index.interfaces().count()
                ));
                if index.untimed_packets() > 0 {
                    ui.weak(format!("{} untimed", index.untimed_packets()));
                }
            } else if self.summary.complete {
                ui.weak(format!("{} packets", self.summary.packets.len()));
            } else if self.index_failed {
                ui.weak("Showing initial packets");
            } else {
                ui.weak("Reading full activity…");
            }
            let compression = self.summary.format.compression;
            if compression != Compression::Plain {
                let label = match compression {
                    Compression::Plain => "",
                    Compression::Gzip => "gzip",
                    Compression::Zstd => "zstd",
                    Compression::Lz4 => "LZ4",
                };
                ui.weak(label)
                    .on_hover_text("Seeking may decode earlier data in the compressed stream");
            }
            if let Some(range) = self.range.as_ref() {
                ui.monospace(format!("Packets {}–{}", range.start + 1, range.end));
            }
            self.copy_action(ui, copies);
        });
        self.filter_controls(ui);
        if let Some(error) = self.error.as_deref() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn copy_action(&self, ui: &mut Ui, copies: &mut Copies) {
        let label = if self.range.is_some() {
            "Copy source range"
        } else {
            "Copy packet"
        };
        let rows = self
            .range
            .clone()
            .or_else(|| self.selected.map(|row| row..row + 1));
        if ui
            .add_enabled(
                self.index.is_some() && rows.is_some() && !copies.busy(),
                egui::Button::new(label),
            )
            .on_hover_text("Copy the selected source packets as a capture file")
            .clicked()
            && let (Some(index), Some(rows)) = (self.index.clone(), rows)
        {
            let capture = self.capture.clone();
            copies.start(ui.ctx(), move |cancel| capture.copy(&index, rows, cancel));
        }
    }

    fn filter_controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("capture protocol")
                .selected_text(
                    self.filter_protocol
                        .map_or("All protocols", Protocol::label),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter_protocol, None, "All protocols");
                    for protocol in [
                        Protocol::Tcp,
                        Protocol::Udp,
                        Protocol::Dns,
                        Protocol::Icmp,
                        Protocol::Icmpv6,
                        Protocol::Arp,
                        Protocol::Ipv4,
                        Protocol::Ipv6,
                        Protocol::Ethernet,
                        Protocol::Unknown,
                    ] {
                        ui.selectable_value(
                            &mut self.filter_protocol,
                            Some(protocol),
                            protocol.label(),
                        );
                    }
                });
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.endpoint)
                    .hint_text("Address or port")
                    .id_salt("capture endpoint")
                    .char_limit(80)
                    .desired_width(155.0),
            );
            let submitted =
                search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(self.index.is_some(), egui::Button::new("Filter"))
                .clicked()
                || submitted
            {
                self.apply_filter(ui.ctx());
            }
            if self.filter.is_some() && ui.small_button("Clear").clicked() {
                self.filter_protocol = None;
                self.endpoint.clear();
                self.clear_filter();
            }
            if self.filter_job.is_some() {
                ui.weak("Filtering…");
            } else if let Some(result) = &self.filtered {
                ui.weak(format!("{} matches", result.rows.len()));
                if result.scanned < self.source_rows() {
                    ui.weak(format!("First {} packets searched", result.scanned));
                }
            }
        });
    }

    fn timeline(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let bins = self.bins();
        activity::with_mark(&painter, rect, bins, self.selected_time(), colors);
        let Some(domain) = activity::bounds(bins) else {
            return;
        };
        let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
        let (_, drag) =
            preview_copy::drag_range(ui, &response, Id::unique("capture brush"), modified);
        let mut picked = None;
        if let Some((a, b)) = drag {
            let band = [
                activity::at(rect, domain, rect.left() + rect.width() * a),
                activity::at(rect, domain, rect.left() + rect.width() * b),
            ];
            self.brushed = Some(band);
            if response.drag_stopped_by(PointerButton::Primary)
                && let Some(range) = activity::source_span(self.bins(), band)
            {
                picked = Some((range.start, Some((range, band))));
            }
        } else if response.clicked_by(PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(bin) = activity::nearest(bins, activity::at(rect, domain, pos.x))
        {
            picked = Some((bin.first_row, None));
            self.brushed = None;
        }
        if let Some(band) = self.selection_time() {
            activity::highlight(&painter, rect, domain, band, colors);
        }
        if let Some((row, span)) = picked {
            self.focus_packet(row);
            if let Some((range, band)) = span {
                self.range = Some(range);
                self.brushed = Some(band);
            }
        }
        let label = format!(
            "{} of timed activity · Click to inspect · Drag to select the source packet span",
            activity::relative(domain[1] - domain[0]),
        );
        response.on_hover_text(label);
    }

    fn navigation(&mut self, ui: &mut Ui) {
        let count = self.visible_rows();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.page > 0, egui::Button::new("Previous page"))
                .clicked()
            {
                self.set_page(self.page.saturating_sub(PAGE_ROWS));
            }
            if ui
                .add_enabled(
                    self.page + PAGE_ROWS < count,
                    egui::Button::new("Next page"),
                )
                .clicked()
            {
                self.set_page(self.page + PAGE_ROWS);
            }
            if count > PAGE_ROWS {
                ui.weak(format!(
                    "Page {} / {}",
                    self.page / PAGE_ROWS + 1,
                    count.div_ceil(PAGE_ROWS)
                ));
            }
            let max = self.source_rows();
            ui.add_enabled(
                max > 0,
                egui::DragValue::new(&mut self.packet_number)
                    .range(1..=max.max(1))
                    .prefix("Packet "),
            );
            if ui.add_enabled(max > 0, egui::Button::new("Go")).clicked() {
                self.focus_packet(self.packet_number.saturating_sub(1).min(max - 1));
            }
        });
    }

    fn browser(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let width = ui.available_width();
        let height = ui.available_height().max(140.0);
        if width >= 760.0 {
            ui.horizontal_top(|ui| {
                let left = (width * 0.61).min(width - 290.0);
                ui.allocate_ui_with_layout(
                    vec2(left, height),
                    Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(left);
                        self.packet_table(ui, height, colors);
                    },
                );
                ui.separator();
                ui.vertical(|ui| self.inspector(ui));
            });
        } else {
            self.packet_table(ui, (height * 0.48).max(88.0), colors);
            ui.separator();
            self.inspector(ui);
        }
    }

    fn packet_table(&mut self, ui: &mut Ui, height: f32, colors: WorkbenchColors) {
        let count = self.visible_rows().saturating_sub(self.page).min(PAGE_ROWS) as usize;
        let scroll = self.scroll_to.take();
        let output = table::show(
            ui,
            Page {
                count,
                offset: self.page,
                selected: self.selected,
                selection: self.range.as_ref(),
                scroll_to: scroll,
                generation: self.generation,
                height,
            },
            self.origin(),
            colors,
            |index| self.row_at(index),
        );
        if let Some((ordinal, extend)) = output.clicked {
            self.select(ordinal, extend);
        }
        self.request_rows(ui.ctx(), output.visible);
    }

    fn inspector(&self, ui: &mut Ui) {
        let Some(row) = self.selected else {
            ui.weak("Select a packet to inspect its captured bytes");
            return;
        };
        if let Some(packet) = self
            .packet
            .as_ref()
            .filter(|packet| packet.header.row == row)
        {
            let id = packet.header.interface;
            let interface = self
                .index
                .as_ref()
                .and_then(|index| index.interface(id))
                .or_else(|| {
                    self.summary
                        .interfaces
                        .iter()
                        .find(|interface| interface.id == id)
                });
            inspect::show(ui, packet, interface, self.origin());
        } else if self.packet_failed == Some(row) {
            ui.weak(format!("Packet {} unavailable", row + 1));
        } else {
            ui.weak(format!("Reading packet {}…", row + 1));
        }
    }

    fn source_rows(&self) -> u64 {
        self.index
            .as_ref()
            .map_or(self.summary.packets.len() as u64, |index| index.points())
    }

    fn visible_rows(&self) -> u64 {
        self.filtered
            .as_ref()
            .map_or(self.source_rows(), |filtered| filtered.rows.len() as u64)
    }

    fn row_at(&self, view_row: u64) -> Option<&PacketRow> {
        if let Some(filtered) = &self.filtered {
            return filtered.rows.get(view_row as usize);
        }
        self.source_row(view_row)
    }

    fn source_row(&self, ordinal: u64) -> Option<&PacketRow> {
        self.summary
            .packets
            .get(usize::try_from(ordinal).ok()?)
            .or_else(|| self.windows.get(ordinal))
    }

    fn selected_time(&self) -> Option<i128> {
        let ordinal = self.selected?;
        self.packet
            .as_ref()
            .filter(|packet| packet.header.row == ordinal)
            .map(|packet| packet.header)
            .or_else(|| self.source_row(ordinal).map(|row| row.header))?
            .time
            .map(|time| time.unix_nanoseconds())
    }

    fn selection_time(&self) -> Option<activity::Bounds> {
        self.brushed.or_else(|| {
            let range = self.range.as_ref()?;
            let first = self
                .source_row(range.start)?
                .header
                .time?
                .unix_nanoseconds();
            let last = self
                .source_row(range.end.saturating_sub(1))?
                .header
                .time?
                .unix_nanoseconds();
            Some([first, last])
        })
    }

    fn bins(&self) -> &[ActivityBin] {
        self.index
            .as_ref()
            .map_or(&self.initial_bins, |index| index.activity())
    }

    fn origin(&self) -> Option<i128> {
        self.index
            .as_ref()
            .and_then(|index| index.time_bounds_ns().map(|bounds| bounds[0]))
            .or_else(|| self.initial_bins.first().map(|first| first.start_ns))
    }

    fn select(&mut self, ordinal: u64, extend: bool) {
        self.packet_number = ordinal.saturating_add(1);
        if self.selected != Some(ordinal) {
            self.packet_job = None;
            self.packet_failed = None;
        }
        self.selected = Some(ordinal);
        if extend {
            let anchor = self.anchor.unwrap_or(ordinal);
            self.range = Some(anchor.min(ordinal)..anchor.max(ordinal).saturating_add(1));
        } else {
            self.range = None;
            self.anchor = Some(ordinal);
        }
        self.brushed = None;
    }

    fn reveal(&mut self, ordinal: u64) {
        let view_row = if let Some(filtered) = &self.filtered {
            filtered
                .rows
                .binary_search_by_key(&ordinal, |row| row.header.row)
                .ok()
                .map(|row| row as u64)
        } else {
            Some(ordinal)
        };
        if let Some(row) = view_row {
            self.page = row / PAGE_ROWS * PAGE_ROWS;
            self.scroll_to = Some((row - self.page).saturating_sub(4) as usize);
        }
    }

    fn set_page(&mut self, start: u64) {
        self.page = start;
        self.scroll_to = Some(0);
    }

    fn clear_filter(&mut self) {
        self.filter = None;
        self.filter_job = None;
        self.filtered = None;
        self.error = None;
        self.generation += 1;
        if let Some(ordinal) = self.selected {
            self.reveal(ordinal);
        } else {
            self.set_page(0);
        }
    }

    fn apply_filter(&mut self, ctx: &Context) {
        let Some(index) = self.index.clone() else {
            return;
        };
        let filter = Filter::new(self.filter_protocol, &self.endpoint);
        if !filter.active() {
            self.clear_filter();
            return;
        }
        self.filter = Some(filter.clone());
        self.error = None;
        let capture = self.capture.clone();
        self.filter_job = Some(start(&JOBS, ctx, Duration::from_secs(90), move |cancel| {
            capture.filter(&index, &filter, cancel)
        }));
    }

    fn request_index(&mut self, ctx: &Context) {
        if self.index.is_some() || self.index_job.is_some() || self.index_failed {
            return;
        }
        let capture = self.capture.clone();
        self.index_job = Some(start(&JOBS, ctx, Duration::from_secs(180), move |cancel| {
            capture.index(cancel)
        }));
    }

    fn request_rows(&mut self, ctx: &Context, visible: Range<u64>) {
        if self.windows.pending() || self.filtered.is_some() || visible.is_empty() {
            return;
        }
        let Some(index) = self.index.clone() else {
            return;
        };
        let Some(range) = needed_window(visible, index.points(), |row| {
            self.source_row(row).is_some()
        }) else {
            return;
        };
        let capture = self.capture.clone();
        self.windows.request(
            &JOBS,
            ctx,
            Duration::from_secs(75),
            range,
            move |rows, cancel| capture.rows(&index, rows, cancel),
        );
    }

    fn request_packet(&mut self, ctx: &Context) {
        let Some(row) = self.selected else { return };
        if self.packet_failed == Some(row)
            || self.packet_job.is_some()
            || self
                .packet
                .as_ref()
                .is_some_and(|packet| packet.header.row == row)
            || row >= self.source_rows()
        {
            return;
        }
        let capture = self.capture.clone();
        let index = self.index.clone();
        self.packet_job = Some((
            row,
            start(&JOBS, ctx, Duration::from_secs(75), move |cancel| {
                capture.packet(index.as_deref(), row, cancel)
            }),
        ));
    }

    fn poll(&mut self) {
        if let Some(index) = poll(&mut self.index_job) {
            match index {
                Ok(index) => self.index = Some(Arc::new(index)),
                Err(error) => {
                    self.index_failed = true;
                    self.error = Some(error);
                }
            }
        }
        if let Some(error) = self.windows.poll_error() {
            self.error = Some(error);
        }
        if let Some((row, result)) = keyed_poll(&mut self.packet_job) {
            match result {
                Ok(packet) => self.packet = Some(packet),
                Err(error) => {
                    self.packet_failed = Some(row);
                    self.error = Some(error);
                }
            }
        }
        if let Some(filtered) = poll(&mut self.filter_job) {
            match filtered {
                Ok(filtered) => {
                    self.filtered = Some(filtered);
                    self.generation += 1;
                    self.set_page(0);
                    if let Some(ordinal) = self
                        .filtered
                        .as_ref()
                        .and_then(|filtered| filtered.rows.first())
                        .map(|row| row.header.row)
                    {
                        self.focus_packet(ordinal);
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
    }

    fn pending(&self) -> bool {
        self.index_job.is_some()
            || self.windows.pending()
            || self.packet_job.is_some()
            || self.filter_job.is_some()
    }
}
