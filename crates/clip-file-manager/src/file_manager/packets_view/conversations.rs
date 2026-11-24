use super::{
    Capture, JOBS, activity,
    capture::{ConversationScope, ConversationSummary},
};
use crate::{
    file_manager::{
        capture_browse::{
            Job, keyed_poll, start,
            table::{Columns, Page, RowOrder, Sort, TableOptions},
        },
        preview_copy::Copies,
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Context, Rect, Ui, vec2};
use little_clip_core::engineering::packets::{Conversation, ConversationKey, Direction, Index};
use std::{cmp::Ordering, fmt::Write, sync::Arc, time::Duration};

const COLUMNS: Columns<6> = Columns {
    id: "packet conversations",
    headings: ["Peer A", "Peer B", "Protocol", "A → B", "B → A", "Duration"],
    fractions: [0.25, 0.25, 0.12, 0.13, 0.13, 0.12],
    min_height: 66.0,
    selected_fill: 0.50,
    hover_fill: 0.23,
};

#[derive(Clone)]
pub(super) struct Selection {
    pub key: ConversationKey,
    pub scope: ConversationScope,
    pub bounds: Option<activity::Bounds>,
}

#[derive(Default)]
pub(super) struct Panel {
    pub open: bool,
    summary: Option<Arc<ConversationSummary>>,
    job: Option<(ConversationScope, Job<ConversationSummary>)>,
    error: Option<String>,
    show_bytes: bool,
    order: RowOrder,
    generation: u64,
    highlighted: Option<usize>,
}

pub(super) enum Action {
    WholeCapture,
    SelectedRange,
    Inspect(Selection),
}

impl Panel {
    pub fn pending(&self) -> bool {
        self.job.is_some()
    }

    pub fn empty(&self) -> bool {
        self.summary.is_none() && self.job.is_none() && self.error.is_none()
    }

    pub fn close(&mut self) {
        self.open = false;
        self.job = None;
    }

    pub fn request(
        &mut self,
        ctx: &Context,
        capture: &Arc<Capture>,
        index: &Arc<Index>,
        scope: ConversationScope,
    ) {
        if self
            .summary
            .as_ref()
            .is_some_and(|ready| ready.scope == scope)
        {
            self.job = None;
            self.error = None;
            return;
        }
        if self
            .job
            .as_ref()
            .is_some_and(|(pending, _)| pending == &scope)
        {
            return;
        }
        self.error = None;
        let capture = capture.clone();
        let index = index.clone();
        let requested = scope.clone();
        self.job = Some((
            scope,
            start(&JOBS, ctx, Duration::from_secs(60), move |cancel| {
                capture.conversations(&index, &requested, cancel)
            }),
        ));
    }

    pub fn poll(&mut self, active: Option<&mut Selection>) {
        if let Some((_, result)) = keyed_poll(&mut self.job) {
            match result {
                Ok(summary) => {
                    self.summary = Some(Arc::new(summary));
                    self.reorder();
                    self.highlight(active.as_ref().map(|active| active.key));
                    if let Some(active) = active {
                        active.bounds = self.bounds(active).or(active.bounds);
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
    }

    pub fn bounds(&self, active: &Selection) -> Option<activity::Bounds> {
        let ready = self
            .summary
            .as_ref()
            .filter(|ready| ready.scope == active.scope)?;
        ready
            .groups
            .iter()
            .find(|group| group.key == active.key)?
            .time_ns
    }

    pub fn highlight(&mut self, key: Option<ConversationKey>) {
        self.highlighted = key.and_then(|key| {
            self.summary
                .as_ref()?
                .groups
                .iter()
                .position(|group| group.key == key)
        });
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        indexed: bool,
        has_range: bool,
        colors: WorkbenchColors,
        copies: &mut Copies,
    ) -> Option<Action> {
        ui.separator();
        let mut action = self.controls(ui, indexed, has_range, copies);
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let Some(summary) = self.summary.as_deref() else {
            return action;
        };
        if summary.groups.is_empty() {
            ui.weak("No TCP or UDP endpoint pairs in this range");
            return action;
        }
        let page = Page {
            offset: 0,
            count: summary.groups.len(),
            selected: self.highlighted.map(|index| index as u64),
            selection: None,
            scroll_to: None,
            generation: self.generation,
            height: (ui.available_height() * 0.28).clamp(86.0, 162.0),
        };
        let options = TableOptions {
            sortable_columns: COLUMNS.headings.len(),
            sort: self.order.sort,
            ..Default::default()
        };
        let output = COLUMNS.show_with_options(
            ui,
            (page, options),
            colors,
            |index| {
                let source = self.order.source_index(index as usize);
                summary
                    .groups
                    .get(source)
                    .map(|group| (source as u64, group))
            },
            |(index, _)| index,
            |ui, rect, (_, group)| row(ui, rect, group, colors, self.show_bytes),
        );
        if let Some((index, _)) = output.clicked {
            let group = &summary.groups[index as usize];
            action = Some(Action::Inspect(Selection {
                key: group.key,
                scope: summary.scope.clone(),
                bounds: group.time_ns,
            }));
        }
        if let Some(sort) = output.sort {
            self.order.sort = Some(sort);
            self.reorder();
        }
        action
    }

    fn reorder(&mut self) {
        if let Some(summary) = &self.summary {
            self.order.rebuild(&summary.groups, |sort, left, right| {
                compare(sort, left, right, self.show_bytes)
            });
        }
        self.generation += 1;
    }

    fn controls(
        &mut self,
        ui: &mut Ui,
        indexed: bool,
        has_range: bool,
        copies: &mut Copies,
    ) -> Option<Action> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Conversations");
            if ui
                .add_enabled(indexed, egui::Button::new("Whole capture"))
                .clicked()
            {
                action = Some(Action::WholeCapture);
            }
            if ui
                .add_enabled(
                    indexed && has_range,
                    egui::Button::new("Use selected range"),
                )
                .on_disabled_hover_text(
                    "Drag across the activity timeline to select a time interval",
                )
                .clicked()
            {
                action = Some(Action::SelectedRange);
            }
            let previously_bytes = self.show_bytes;
            clip_ui::buttons::choice(ui, &mut self.show_bytes, false, "Packet counts");
            clip_ui::buttons::choice(ui, &mut self.show_bytes, true, "Captured bytes");
            if previously_bytes != self.show_bytes && self.order.sort.is_some() {
                self.reorder();
            }
            if self.order.sort.is_some() && ui.small_button("Rank by packet count").clicked() {
                self.order.sort = None;
                self.reorder();
            }
            if let Some(summary) = &self.summary {
                if ui
                    .add_enabled(!copies.busy(), egui::Button::new("Copy summary CSV"))
                    .clicked()
                {
                    let summary = summary.clone();
                    copies.start(ui.ctx(), move |_| copy_summary(&summary));
                }
                coverage(ui, summary);
            }
            if self.pending() {
                ui.weak("Reading conversations…");
            } else if !indexed {
                ui.weak("Waiting for the capture index…");
            }
        });
        action
    }
}

fn coverage(ui: &mut Ui, summary: &ConversationSummary) {
    let scope = &summary.scope;
    let label = scope.time.map_or_else(
        || format!("{} source packets read", summary.scanned),
        |[low, high]| {
            format!(
                "{} selection · {} source packets read",
                activity::relative(high - low),
                summary.scanned
            )
        },
    );
    ui.add(egui::Label::new(egui::RichText::new(label).weak()).extend());
    if summary.scanned < scope.total {
        ui.weak(format!("{} in scope", scope.total));
    }
    if summary.omitted > 0 {
        ui.weak(format!("{} beyond the pair limit", summary.omitted));
    }
    if summary.unclassified > 0 {
        ui.weak(format!("{} without a TCP/UDP pair", summary.unclassified));
    }
}

fn compare(sort: Sort, left: &Conversation, right: &Conversation, show_bytes: bool) -> Ordering {
    let duration = |group: &Conversation| group.time_ns.map(|[start, end]| end - start);
    match sort.column {
        0 | 1 => sort.compare(left.key.peers[sort.column], right.key.peers[sort.column]),
        2 => sort.compare(
            (left.key.protocol().label(), left.key.vlan),
            (right.key.protocol().label(), right.key.vlan),
        ),
        3 | 4 => {
            let direction = sort.column - 3;
            sort.compare(
                amount(left.directions[direction], show_bytes),
                amount(right.directions[direction], show_bytes),
            )
        }
        5 => sort.known_first(duration(left), duration(right)),
        _ => Ordering::Equal,
    }
}

fn amount(direction: Direction, show_bytes: bool) -> u64 {
    if show_bytes {
        direction.captured_bytes
    } else {
        direction.packets
    }
}

fn row(ui: &Ui, rect: Rect, group: &Conversation, colors: WorkbenchColors, show_bytes: bool) {
    let protocol = group.key.protocol();
    let tint = super::table::protocol_color(protocol);
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min, vec2(2.0, rect.height())),
        0.0,
        tint,
    );
    let duration = group.time_ns.map_or_else(
        || "—".to_owned(),
        |[start, end]| activity::relative(end - start),
    );
    let counts = group
        .directions
        .map(|direction| amount(direction, show_bytes).to_string());
    let protocol = group.key.vlan.map_or_else(
        || protocol.label().to_owned(),
        |vlan| format!("{} · VLAN {vlan}", protocol.label()),
    );
    COLUMNS.cells(
        ui,
        rect,
        [
            &group.key.peers[0].to_string(),
            &group.key.peers[1].to_string(),
            &protocol,
            &counts[0],
            &counts[1],
            &duration,
        ],
        colors.text_primary,
    );
}

fn copy_summary(summary: &ConversationSummary) -> Result<String, String> {
    let mut csv = String::from(
        "peer_a,peer_b,protocol,vlan,a_to_b_packets,b_to_a_packets,a_to_b_captured_bytes,b_to_a_captured_bytes,first_unix_ns,last_unix_ns\n",
    );
    for group in &summary.groups {
        let key = group.key;
        let [a, b] = group.directions;
        let [first, last] = group.time_ns.map_or_else(
            || [String::new(), String::new()],
            |times| times.map(|time| time.to_string()),
        );
        let vlan = key.vlan.map_or_else(String::new, |vlan| vlan.to_string());
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{}",
            key.peers[0],
            key.peers[1],
            key.protocol().label(),
            vlan,
            a.packets,
            b.packets,
            a.captured_bytes,
            b.captured_bytes,
            first,
            last,
        );
    }
    little_clip_core::copy_text_to_system_clipboard(&csv).map_err(|error| error.to_string())?;
    Ok(format!("Copied {} conversations", summary.groups.len()))
}
