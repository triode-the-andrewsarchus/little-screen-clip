//! Draft criteria, bounded filter requests, and the query that produced the table.

use super::{
    ConversationSelection, Filter, Filtered, JOBS, Protocol, TimeRestriction, Viewer, activity,
    inspect::FieldFilter, keyed_poll, start,
};
use egui::{Button, Context, Ui};
use std::{ops::Range, time::Duration};

struct FilterLocation {
    selected: Option<u64>,
    anchor: Option<u64>,
    range: Option<Range<u64>>,
    brushed: Option<activity::Bounds>,
}

impl FilterLocation {
    fn capture(viewer: &Viewer) -> Self {
        Self {
            selected: viewer.selected,
            anchor: viewer.anchor,
            range: viewer.range.clone(),
            brushed: viewer.brushed,
        }
    }

    fn restore(self, viewer: &mut Viewer) {
        if let Some(selected) = self.selected {
            viewer.select(selected, false);
        } else {
            viewer.selected = None;
            viewer.packet_job = None;
            viewer.packet_failed = None;
        }
        viewer.anchor = self.anchor;
        viewer.range = self.range;
        viewer.brushed = self.brushed;
    }
}

pub(super) struct FilterCheckpoint {
    applied: Option<AppliedFilter>,
    location: FilterLocation,
}

pub(super) struct FilterRequest {
    query: Filter,
    location: FilterLocation,
}

pub(super) struct AppliedFilter {
    pub query: Filter,
    pub result: Filtered,
}

impl AppliedFilter {
    pub fn ordinal_index(&self, ordinal: u64) -> Option<usize> {
        self.result
            .rows
            .binary_search_by_key(&ordinal, |row| row.header.row)
            .ok()
    }

    fn summary(&self, ui: &mut Ui, origin: Option<i128>) {
        let Filtered {
            rows,
            scanned,
            scope_size,
        } = &self.result;
        let unit = if rows.len() == 1 { "match" } else { "matches" };
        ui.strong(format!("{} {unit}", rows.len()));
        if scanned < scope_size {
            ui.weak(format!("{scanned} of {scope_size} source packets searched"));
        }
        if let Some(protocol) = self.query.protocol {
            ui.label(protocol.label());
        }
        if let Some(endpoint) = self.query.endpoint_label() {
            ui.label(endpoint);
        }
        if let Some((key, _)) = self.query.conversation() {
            ui.monospace(format!("{} ↔ {}", key.peers[0], key.peers[1]));
        }
        let scope = match (self.query.time(), origin, self.query.conversation()) {
            (Some([start, end]), Some(origin), _) => format!(
                "Scope  {} – {}",
                activity::relative(start - origin),
                activity::relative(end - origin),
            ),
            (Some(_), _, _) => "Scope  selected time".into(),
            (_, _, Some((_, scope))) => format!(
                "Scope  conversation packets {}–{}",
                scope.rows.start + 1,
                scope.rows.start.saturating_add(scope.total),
            ),
            _ => "Scope  whole capture".into(),
        };
        ui.weak(scope);
        let display = self.query.display();
        if !display.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new(display).monospace()).truncate())
                .on_hover_text(display);
        }
    }
}

impl Viewer {
    pub(super) fn filter_controls(&mut self, ui: &mut Ui) {
        self.protocol_buttons(ui);
        self.query_editor(ui);
        self.filter_result(ui);
    }

    fn protocol_buttons(&mut self, ui: &mut Ui) {
        let current = self.current_filter().and_then(|filter| filter.protocol);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label("Protocol");
            for protocol in [
                None,
                Some(Protocol::Tcp),
                Some(Protocol::Udp),
                Some(Protocol::Dns),
                Some(Protocol::Icmp),
                Some(Protocol::Icmpv6),
                Some(Protocol::Arp),
                Some(Protocol::Ipv4),
                Some(Protocol::Ipv6),
                Some(Protocol::Ethernet),
                Some(Protocol::Unknown),
            ] {
                let label = protocol.map_or("All", Protocol::label);
                if ui
                    .add_enabled(
                        self.index.is_some(),
                        Button::new(label).small().selected(current == protocol),
                    )
                    .clicked()
                {
                    let mut query = self.filter_basis();
                    query.protocol = protocol;
                    self.request_filter(ui.ctx(), query);
                }
            }
        });
    }

    fn query_editor(&mut self, ui: &mut Ui) {
        let dirty = self.draft_changed();
        ui.horizontal_wrapped(|ui| {
            let endpoint = ui.add(
                egui::TextEdit::singleline(&mut self.endpoint)
                    .hint_text("IP or port")
                    .id_salt("capture endpoint")
                    .char_limit(80)
                    .desired_width(130.0),
            );
            let display = ui.add(
                egui::TextEdit::singleline(&mut self.display_filter)
                    .id_salt("packet display filter")
                    .hint_text("Display filter  e.g. tcp.port in {80,443}")
                    .char_limit(2048)
                    .desired_width((ui.available_size_before_wrap().x - 108.0).clamp(205.0, 445.0)),
            );
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                display.request_focus();
            }
            let submitted = (display.lost_focus() || endpoint.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(self.index.is_some(), Button::new("Apply filter"))
                .clicked()
                || submitted
            {
                let query = self
                    .filter_basis()
                    .with_text(&self.endpoint, &self.display_filter);
                self.request_filter(ui.ctx(), query);
            }
            if dirty {
                ui.weak("Unapplied edits");
            }
            if (dirty || self.current_filter().is_some())
                && ui.small_button("Clear filters").clicked()
            {
                self.endpoint.clear();
                self.display_filter.clear();
                self.clear_filter();
            }
        });
    }

    fn filter_result(&mut self, ui: &mut Ui) {
        if self.filtered.is_some()
            || self.filter_job.is_some()
            || self.range.is_some()
            || self.previous_filter.is_some()
            || self.draft_changed()
            || self.filter_error.is_some()
        {
            ui.horizontal_wrapped(|ui| {
                if let Some(applied) = &self.filtered {
                    ui.weak("Applied");
                    applied.summary(ui, self.origin());
                } else {
                    ui.weak("Applied  whole capture");
                }
                if self.filter_job.is_some() {
                    ui.spinner();
                    ui.weak("Filtering…");
                }
                self.time_buttons(ui);
                self.recovery_actions(ui);
            });
        }
        if let Some(error) = self.filter_error.as_deref() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        self.selection_outside_results(ui);
    }

    fn recovery_actions(&mut self, ui: &mut Ui) {
        if self.filter_job.is_some() || self.draft_changed() || self.filter_error.is_some() {
            let label = if self.filter_job.is_some() {
                "Cancel filtering"
            } else {
                "Revert draft"
            };
            if ui.small_button(label).clicked() {
                self.restore_filter_editor();
            }
        }
        if self.previous_filter.is_some() && ui.small_button("Undo filter").clicked() {
            self.undo_filter();
        }
    }

    fn time_buttons(&mut self, ui: &mut Ui) {
        let has_time = self.current_filter().and_then(Filter::time).is_some();
        if let Some((rows, [first, last])) = self.range.clone().zip(self.selection_time()) {
            if ui
                .add_enabled(
                    self.index.is_some(),
                    Button::new("Filter to selected time").small(),
                )
                .clicked()
            {
                let bounds = [first.min(last), first.max(last)];
                let query = self
                    .filter_basis()
                    .with_time(Some(TimeRestriction { bounds, rows }));
                self.request_filter(ui.ctx(), query);
            }
        }
        if has_time && ui.small_button("Remove time filter").clicked() {
            let query = self.filter_basis().with_time(None);
            self.request_filter(ui.ctx(), query);
        }
    }

    fn selection_outside_results(&mut self, ui: &mut Ui) {
        let Some((applied, selected)) = self.filtered.as_ref().zip(self.selected) else {
            return;
        };
        if applied.ordinal_index(selected).is_some() {
            return;
        }
        let first = applied.result.rows.first().map(|row| row.header.row);
        ui.horizontal_wrapped(|ui| {
            ui.weak(format!("Packet {} is outside the match set", selected + 1));
            if let Some(first) = first
                && ui.small_button("First match").clicked()
            {
                let location = FilterLocation::capture(self);
                FilterLocation {
                    selected: Some(first),
                    ..location
                }
                .restore(self);
                self.reveal(first);
            }
        });
    }

    fn current_filter(&self) -> Option<&Filter> {
        self.filter_job
            .as_ref()
            .map(|(request, _)| &request.query)
            .or_else(|| self.filtered.as_ref().map(|applied| &applied.query))
    }

    pub(super) fn filter_basis(&self) -> Filter {
        self.current_filter().cloned().unwrap_or_default()
    }

    pub(super) fn draft_changed(&self) -> bool {
        self.current_filter().map_or_else(
            || !self.endpoint.trim().is_empty() || !self.display_filter.trim().is_empty(),
            |query| !query.text_matches(&self.endpoint, &self.display_filter),
        )
    }

    pub(super) fn use_field_filter(&mut self, ctx: &Context, action: FieldFilter) {
        let expression = match action {
            FieldFilter::Add(expression) => expression,
            FieldFilter::Replace(expression) => {
                let query = self.filter_basis().replace_criteria(expression);
                self.endpoint.clear();
                self.display_filter = query.display().to_owned();
                self.request_filter(ctx, query);
                return;
            }
        };
        let pending = self.draft_changed();
        let basis = self.filter_basis();
        let text = if pending {
            self.display_filter.trim()
        } else {
            basis.display()
        };
        let next = if text.is_empty() {
            expression
        } else if text == expression {
            return;
        } else {
            format!("({text}) && ({expression})")
        };
        if next.chars().count() > 2048 {
            self.filter_error =
                Some("The combined display filter exceeds 2,048 characters.".into());
            return;
        }
        self.display_filter = next;
        if !pending {
            let query = basis.with_text(&self.endpoint, &self.display_filter);
            self.request_filter(ctx, query);
        }
    }

    fn clear_filter(&mut self) {
        self.filter_job = None;
        self.filter_error = None;
        if self.filtered.is_some() {
            self.accept_filter(None, FilterLocation::capture(self));
        }
    }

    fn accept_filter(&mut self, next: Option<AppliedFilter>, location: FilterLocation) {
        self.previous_filter = Some(FilterCheckpoint {
            applied: std::mem::replace(&mut self.filtered, next),
            location,
        });
        if self.selected.is_none()
            && let Some(first) = self
                .filtered
                .as_ref()
                .and_then(|applied| applied.result.rows.first())
        {
            self.select(first.header.row, false);
        }
        self.refresh_results();
    }

    fn refresh_results(&mut self) {
        let selected_conversation = self
            .filtered
            .as_ref()
            .and_then(|applied| applied.query.conversation());
        self.conversation = selected_conversation.map(|(key, scope)| {
            let mut active = ConversationSelection {
                key: *key,
                scope: scope.clone(),
                bounds: None,
            };
            active.bounds = self.conversation_panel.bounds(&active).or_else(|| {
                let previous = self.conversation.as_ref()?;
                (previous.key == *key && previous.scope == *scope)
                    .then_some(previous.bounds)
                    .flatten()
            });
            active
        });
        self.conversation_panel
            .highlight(self.conversation.as_ref().map(|active| active.key));
        self.generation += 1;
        self.set_page(0);
        if let Some(ordinal) = self.selected {
            self.reveal(ordinal);
        }
    }

    fn restore_filter_editor(&mut self) {
        self.filter_job = None;
        self.filter_error = None;
        let query = self.filtered.as_ref().map(|applied| &applied.query);
        self.endpoint = query.map_or_else(String::new, Filter::endpoint_text);
        self.display_filter = query.map_or_else(String::new, |query| query.display().to_owned());
    }

    fn undo_filter(&mut self) {
        let Some(previous) = self.previous_filter.take() else {
            return;
        };
        self.filtered = previous.applied;
        self.restore_filter_editor();
        previous.location.restore(self);
        self.refresh_results();
    }

    pub(super) fn request_filter(&mut self, ctx: &Context, query: Filter) {
        let Some(index) = self.index.clone() else {
            return;
        };
        if !query.active() {
            self.clear_filter();
            return;
        }
        if self
            .filter_job
            .as_ref()
            .is_some_and(|(pending, _)| pending.query == query)
        {
            return;
        }
        self.filter_error = None;
        if self
            .filtered
            .as_ref()
            .is_some_and(|applied| applied.query == query)
        {
            self.filter_job = None;
            return;
        }
        let capture = self.capture.clone();
        let requested = query.clone();
        self.filter_job = Some((
            FilterRequest {
                query,
                location: FilterLocation::capture(self),
            },
            start(&JOBS, ctx, Duration::from_secs(90), move |cancel| {
                capture.filter(&index, &requested, cancel)
            }),
        ));
    }

    pub(super) fn poll_filter(&mut self) {
        let Some((request, result)) = keyed_poll(&mut self.filter_job) else {
            return;
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.filter_error = Some(error);
                return;
            }
        };
        self.accept_filter(
            Some(AppliedFilter {
                query: request.query,
                result,
            }),
            request.location,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::atomic::AtomicBool};

    #[test]
    fn undo_restores_applied_query_coverage_and_source_selection() {
        let mut source = tempfile::Builder::new().suffix(".pcap").tempfile().unwrap();
        source
            .write_all(&[
                0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 1, 0,
                0, 0,
            ])
            .unwrap();
        let mut view = Viewer::load(source.path(), &AtomicBool::new(false)).unwrap();
        view.select(1, false);
        view.range = Some(1..7);
        view.brushed = Some([500_000_000, 6_100_000_000]);
        let location = FilterLocation::capture(&view);
        let query = Filter::default().with_text("443", "tcp");
        view.filtered = Some(AppliedFilter {
            query: query.clone(),
            result: Filtered {
                rows: Vec::new(),
                scanned: 64,
                scope_size: 100,
            },
        });
        let refined = query
            .clone()
            .with_text("443", "tcp && ip.src == 203.0.113.3");
        view.accept_filter(
            Some(AppliedFilter {
                query: refined,
                result: Filtered {
                    rows: Vec::new(),
                    scanned: 100,
                    scope_size: 100,
                },
            }),
            location,
        );
        view.select(5, false);
        view.display_filter = "invalid draft".into();
        view.filter_error = Some("Invalid display filter".into());

        view.undo_filter();
        let applied = view.filtered.as_ref().unwrap();
        assert!(applied.query == query);
        assert_eq!(
            (applied.result.scanned, applied.result.scope_size),
            (64, 100)
        );
        assert_eq!((view.selected, view.anchor), (Some(1), Some(1)));
        assert_eq!(view.range, Some(1..7));
        assert_eq!(view.brushed, Some([500_000_000, 6_100_000_000]));
        assert_eq!(
            (view.endpoint.as_str(), view.display_filter.as_str()),
            ("443", "tcp")
        );
        assert!(view.filter_error.is_none() && view.previous_filter.is_none());
    }
}
