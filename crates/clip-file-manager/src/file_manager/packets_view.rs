//! Linked, read-only views of a packet capture and its source records.

mod activity;
mod capture;
mod conversations;
mod display_filter;
mod filter_ui;
mod inspect;
pub(super) mod strip;
mod table;
mod tcp_flow;

use super::{
    capture_browse::{
        Job, PAGE_ROWS, keyed_poll, poll,
        rows::{RowWindows, needed_window},
        start,
        table::{Page, RowOrder, TableOptions},
    },
    preview_copy::{self, Copies},
};
use crate::{file_source::ReadCheck, ui_appearance::WorkbenchColors};
use capture::{Capture, ConversationScope, Filter, Filtered, TimeRestriction};
use conversations::{Action as ConversationAction, Selection as ConversationSelection};
use egui::{Context, Id, Layout, PointerButton, Sense, Ui, vec2};
use filter_ui::{AppliedFilter, ConversationVisit, FilterCheckpoint, FilterRequest};
use little_clip_core::{
    BackgroundJobs,
    engineering::packets::{
        ActivityBin, Compression, ConversationKey, Index, Packet, PacketRow, Protocol, Summary,
    },
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
    scroll_offset: f32,
    restore_scroll: Option<f32>,
    order: RowOrder,
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
    inspector: inspect::Inspector,
    display_filter: String,
    protocol_colors: bool,
    endpoint: String,
    filtered: Option<AppliedFilter>,
    filter_job: Option<(FilterRequest, Job<Filtered>)>,
    previous_filter: Option<FilterCheckpoint>,
    conversation_visit: Option<ConversationVisit>,
    filter_error: Option<String>,
    conversation_panel: conversations::Panel,
    conversation: Option<ConversationSelection>,
    tcp_flow: tcp_flow::View,
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
            scroll_offset: 0.0,
            restore_scroll: None,
            order: RowOrder::default(),
            packet_number: 1,
            windows: RowWindows::new(3),
            selected,
            range: None,
            anchor: selected,
            brushed: None,
            packet: None,
            packet_job: None,
            packet_failed: None,
            inspector: inspect::Inspector::default(),
            display_filter: String::new(),
            protocol_colors: true,
            endpoint: String::new(),
            filtered: None,
            filter_job: None,
            previous_filter: None,
            conversation_visit: None,
            filter_error: None,
            conversation_panel: conversations::Panel::default(),
            conversation: None,
            tcp_flow: tcp_flow::View::default(),
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
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            ui.spacing_mut().button_padding.y = 2.0;
            self.controls(ui, copies);
        });
        self.timeline(ui, colors);
        ui.push_id(self.conversation_visit.is_some(), |ui| {
            self.conversations(ui, colors, copies);
            self.draw_tcp_flow(ui, colors, copies);
        });
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
                let interfaces = index.interfaces().count();
                let unit = if interfaces == 1 {
                    "interface"
                } else {
                    "interfaces"
                };
                ui.weak(format!("{} packets · {interfaces} {unit}", index.points()));
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
                ui.monospace(format!(
                    "Source selection  {}–{}",
                    range.start + 1,
                    range.end
                ));
            }
            self.copy_action(ui, copies);
            if ui
                .add(egui::Button::new("Conversations").selected(self.conversation_panel.open))
                .clicked()
            {
                if self.conversation_panel.open {
                    self.conversation_panel.close();
                } else {
                    self.conversation_panel.open = true;
                }
            }
            ui.checkbox(&mut self.protocol_colors, "Protocol colors");
        });
        self.filter_controls(ui);
        if let Some(error) = self.error.as_deref() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn copy_action(&self, ui: &mut Ui, copies: &mut Copies) {
        let label = self.range.as_ref().map_or_else(
            || "Copy packet".to_owned(),
            |rows| format!("Copy {} source packets", rows.end - rows.start),
        );
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

    fn conversations(&mut self, ui: &mut Ui, colors: WorkbenchColors, copies: &mut Copies) {
        if !self.conversation_panel.open {
            return;
        }
        let has_range = self.range.is_some();
        let action =
            self.conversation_panel
                .show(ui, self.index.is_some(), has_range, colors, copies);
        match action {
            Some(ConversationAction::WholeCapture) => self.read_conversations(ui.ctx(), false),
            Some(ConversationAction::SelectedRange) => self.read_conversations(ui.ctx(), true),
            Some(ConversationAction::Inspect(selection)) => {
                self.use_conversation(ui.ctx(), Some(selection));
            }
            None if self.conversation_panel.empty() => self.read_conversations(ui.ctx(), false),
            None => {}
        }
    }

    fn draw_tcp_flow(&mut self, ui: &mut Ui, colors: WorkbenchColors, copies: &mut Copies) {
        let active = self
            .conversation
            .as_ref()
            .filter(|selection| selection.key.protocol() == Protocol::Tcp);
        let Some((selection, index)) = active
            .zip(self.index.as_ref())
            .filter(|_| self.tcp_flow.open)
        else {
            self.tcp_flow.suspend();
            return;
        };
        let origin = self.origin();
        self.tcp_flow
            .request(ui.ctx(), &self.capture, index, selection);
        if let Some(frame) =
            self.tcp_flow
                .show(ui, selection, self.selected, origin, colors, copies)
        {
            self.focus_packet(frame);
            ui.ctx().request_repaint();
        }
    }

    fn read_conversations(&mut self, ctx: &Context, selection: bool) {
        let Some(index) = &self.index else { return };
        let rows = if selection {
            let Some(rows) = self.range.clone() else {
                return;
            };
            rows.start.min(index.points())..rows.end.min(index.points())
        } else {
            0..index.points()
        };
        let time = if selection { self.brushed } else { None };
        let scope = ConversationScope::new(rows, time);
        self.conversation_panel
            .request(ctx, &self.capture, index, scope);
    }

    fn use_conversation(&mut self, ctx: &Context, selection: Option<ConversationSelection>) {
        let scope = selection.map(|active| (active.key, active.scope));
        let query = self.filter_basis().with_conversation(scope);
        self.request_filter(ctx, query);
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
        let brush = drag.map(|(a, b)| {
            [a, b].map(|end| activity::at(rect, domain, rect.left() + rect.width() * end))
        });
        let mut picked = None;
        if let Some(band) = brush {
            if response.drag_stopped_by(PointerButton::Primary)
                && let Some(range) = activity::source_span(bins, band)
            {
                picked = Some((range.start, Some((range, band))));
            }
        } else if response.clicked_by(PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(bin) = activity::nearest(bins, activity::at(rect, domain, pos.x))
        {
            picked = Some((bin.first_row, None));
        }
        if let Some(band) = self
            .conversation
            .as_ref()
            .and_then(|selection| selection.bounds)
        {
            activity::highlight(&painter, rect, domain, band, colors);
        }
        if let Some(band) = self
            .filtered
            .as_ref()
            .and_then(|applied| applied.query.time())
        {
            activity::highlight_filter(&painter, rect, domain, band, colors);
        }
        if let Some(band) = brush.or_else(|| self.selection_time()) {
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
            if self.order.sort.is_some() && ui.small_button("Restore capture order").clicked() {
                self.order.sort = None;
                self.reorder_packets();
            }
            if self.sortable_packet_rows().is_none() && count > 0 {
                ui.weak("Capture order")
                    .on_hover_text("Apply a filter to sort its matches across all result pages");
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
        let options = TableOptions {
            sortable_columns: if self.sortable_packet_rows().is_some() {
                6
            } else {
                0
            },
            sort: self.order.sort,
            scroll_offset: self.restore_scroll.take(),
        };
        let page = Page {
            count,
            offset: self.page,
            selected: self.selected,
            selection: self.range.as_ref(),
            scroll_to: scroll,
            generation: self.generation,
            height,
        };
        let output = table::show(
            ui,
            (page, options),
            self.origin(),
            colors,
            self.protocol_colors,
            |index| self.row_at(index),
        );
        self.scroll_offset = output.scroll_offset;
        if let Some(sort) = output.sort {
            self.order.sort = Some(sort);
            self.reorder_packets();
        }
        if let Some((ordinal, extend)) = output.clicked {
            self.select(ordinal, extend);
        }
        self.request_rows(ui.ctx(), output.visible);
    }

    fn inspect_conversation(&mut self, ui: &mut Ui, row: u64) {
        let Some(index) = &self.index else { return };
        let Some(packet) = self
            .packet
            .as_ref()
            .filter(|packet| packet.header.row == row)
        else {
            return;
        };
        let Some((key, _)) = ConversationKey::identify(packet.protocol()) else {
            return;
        };
        if self
            .conversation
            .as_ref()
            .is_some_and(|active| active.key == key)
        {
            return;
        }
        if ui
            .add_enabled(
                self.filter_job.is_none(),
                egui::Button::new("Filter to this conversation").small(),
            )
            .on_hover_text("Keep the current filters and restrict matches to this endpoint pair")
            .clicked()
        {
            self.visit_conversation(
                ui.ctx(),
                key,
                ConversationScope::around(index.points(), row),
            );
        }
    }

    fn inspector(&mut self, ui: &mut Ui) {
        if self.conversation_visit.is_some() && ui.small_button("Back to packet results").clicked()
        {
            self.return_from_conversation();
        }
        ui.push_id(self.conversation_visit.is_some(), |ui| {
            self.inspect_packet(ui)
        });
    }

    fn inspect_packet(&mut self, ui: &mut Ui) {
        ui.spacing_mut().interact_size.y = 21.0;
        ui.spacing_mut().button_padding.y = 2.0;
        let Some(row) = self.selected else {
            ui.weak("Select a packet to inspect its captured bytes");
            return;
        };
        self.inspect_conversation(ui, row);
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
            let origin = self.origin();
            let draft = self.draft_changed();
            if let Some(expression) = self.inspector.show(ui, packet, interface, origin, draft) {
                self.use_field_filter(ui.ctx(), expression);
            }
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
            .map_or(self.source_rows(), |filtered| {
                filtered.result.rows.len() as u64
            })
    }

    fn row_at(&self, view_row: u64) -> Option<&PacketRow> {
        if let Some(rows) = self.sortable_packet_rows() {
            return rows.get(self.order.source_index(usize::try_from(view_row).ok()?));
        }
        self.source_row(view_row)
    }

    fn sortable_packet_rows(&self) -> Option<&[PacketRow]> {
        if let Some(applied) = &self.filtered {
            Some(&applied.result.rows)
        } else {
            let prefix = &self.summary.packets;
            (self.summary.complete
                || self
                    .index
                    .as_ref()
                    .is_some_and(|index| index.points() == prefix.len() as u64))
            .then_some(prefix)
        }
    }

    fn rebuild_packet_order(&mut self) {
        if let Some(applied) = &self.filtered {
            self.order.rebuild(&applied.result.rows, table::compare);
        } else if self.sortable_packet_rows().is_some() {
            self.order.rebuild(&self.summary.packets, table::compare);
        } else {
            self.order = RowOrder::default();
        }
    }

    fn reorder_packets(&mut self) {
        self.rebuild_packet_order();
        self.generation += 1;
        self.set_page(0);
        if let Some(ordinal) = self.selected {
            self.reveal(ordinal);
        }
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
            filtered.ordinal_index(ordinal).map(|row| row as u64)
        } else {
            Some(ordinal)
        };
        if let Some(mut row) = view_row {
            if self.order.sort.is_some() {
                row = self.order.display_index(row as usize) as u64;
            }
            self.page = row / PAGE_ROWS * PAGE_ROWS;
            self.scroll_to = Some((row - self.page).saturating_sub(4) as usize);
        }
    }

    fn set_page(&mut self, start: u64) {
        self.page = start;
        self.restore_scroll = None;
        self.scroll_to = Some(0);
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
        self.conversation_panel.poll(self.conversation.as_mut());
        self.tcp_flow.poll();
        self.poll_filter();
    }

    fn pending(&self) -> bool {
        self.index_job.is_some()
            || self.windows.pending()
            || self.packet_job.is_some()
            || self.filter_job.is_some()
            || self.conversation_panel.pending()
            || self.tcp_flow.pending()
    }
}
