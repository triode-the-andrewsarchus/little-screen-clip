//! The Shelf is a compact two-row hotbar, with actions for its selected card.

use super::super::FileManager;
use super::preview::{Previews, Ready, Source};
use super::{Action, Entry, Payload, ResultValue, Selected, Shelf, ipc};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::palette::AppPalette;
use little_clip_core::{HandSide, SlotId};

impl FileManager {
    pub(in crate::file_manager) fn shelf_ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        self.shelf.previews.poll(ui.ctx());
        if self.shelf.checked_at.elapsed() >= std::time::Duration::from_millis(500) {
            self.shelf.refresh_inventory();
        }
        let mut action = None;
        self.shelf_header(ui, colors, &mut action);
        let height = ((ui.available_height() - 10.0) / 2.0).clamp(24.0, 96.0);
        self.shelf.rows_ui(ui, palette, height, &mut action);
        if let Some(action) = action {
            self.shelf_action(action);
        }
    }

    fn shelf_header(&mut self, ui: &mut Ui, colors: WorkbenchColors, action: &mut Option<Action>) {
        egui::ScrollArea::horizontal().id_source("shelf_actions").max_height(28.0).show(ui, |ui| ui.horizontal(|ui| {
            ui.add(Icon::Layers.label(egui::RichText::new("Shelf").strong()));
            ui.small(format!("{} collected", self.shelf.entries.len()));
            self.shelf.selected_controls(ui, self.directory.is_some(), action);
            ui.menu_button("More", |ui| {
                if ui.add_enabled(!self.shelf.busy(), egui::Button::new("Refresh hands and previews")).clicked() { *action = Some(Action::Refresh); ui.close_menu(); }
                if ui.add_enabled(!self.shelf.busy(), egui::Button::new("Check file references")).clicked() { *action = Some(Action::Check); ui.close_menu(); }
                ui.separator();
                ui.label("Collections stay for this session. File cards use live references; content cards retain captured bytes.");
                if let Some(error) = &self.shelf.inventory_error { ui.colored_label(colors.warning, error); }
            });
            if self.shelf.busy() { ui.spinner(); }
            if let Some(notice) = &self.shelf.notice {
                ui.add(Icon::Info.image_tinted(16.0, colors.warning)).on_hover_text(notice);
            }
        }));
    }

    fn shelf_action(&mut self, action: Action) {
        match action {
            Action::Select(selected) => self.shelf.selected = Some(selected),
            Action::Place(Payload::Files { paths, cut }) => {
                self.queue_transfer(paths, self.path().into(), cut)
            }
            Action::Place(Payload::Content(content)) => {
                self.name_content(content, self.path().into())
            }
            Action::Remove(selected) => self.shelf.remove(selected),
            Action::CollectSlot(slot, id) => self.shelf.collect_slot(slot, id),
            Action::CollectSelection => self.shelf.add(
                Payload::Files {
                    paths: self.selected_paths(),
                    cut: false,
                },
                "File group".into(),
            ),
            Action::Capture => self.shelf.submit(false, || {
                super::super::transfers::clipboard::read_payload()
                    .map(|payload| ResultValue::Payload(payload, "Captured clipboard".into()))
                    .map_err(|error| error.to_string())
            }),
            Action::Refresh => {
                self.shelf.previews.clear();
                self.shelf.refresh_inventory();
            }
            Action::Check => self.shelf.check_references(),
            Action::Send(payload, hand) => self.shelf.submit(true, move || {
                let request = super::send_request(payload, hand)?;
                match ipc(request).map_err(|error| {
                    format!("Send not confirmed: {error}. Check the hand before retrying.")
                })? {
                    little_clip_core::ipc::IpcResponse::Success { .. } => Ok(ResultValue::Sent),
                    _ => Err("Unexpected hand response; inspect the hand before retrying.".into()),
                }
            }),
        }
    }
}

impl Shelf {
    fn remove(&mut self, selected: Selected) {
        let id = match selected {
            Selected::Collected(id) => {
                self.entries.retain(|entry| entry.id != id);
                Some(id)
            }
            Selected::Slot(_) => None,
        };
        if let Some(id) = id {
            self.previews.invalidate(id);
        }
        self.selected = None;
    }

    fn selected_controls(&mut self, ui: &mut Ui, can_place: bool, action: &mut Option<Action>) {
        if let Some(Selected::Slot(slot)) = self.selected {
            if let Some(card) = self.summary.as_ref().and_then(|summary| summary.card(slot))
                && ui
                    .add_enabled(!self.busy(), egui::Button::new("Collect hand item"))
                    .clicked()
            {
                *action = Some(Action::CollectSlot(slot, card.item_id));
            }
            return;
        }
        let busy = self.busy();
        let selected = self.selected;
        if let Some(entry) = self.selected_entry_mut() {
            if let Payload::Files { cut, .. } = &mut entry.payload {
                if ui.add(egui::Button::new("Copy").selected(!*cut)).clicked() {
                    *cut = false;
                }
                if ui.add(egui::Button::new("Move").selected(*cut)).clicked() {
                    *cut = true;
                }
            }
            let label = match &entry.payload {
                Payload::Files { cut: true, .. } => "Move here",
                Payload::Files { .. } => "Copy here",
                _ => "Create file…",
            };
            if ui
                .add_enabled(can_place, egui::Button::new(label))
                .clicked()
            {
                *action = Some(Action::Place(entry.payload.clone()));
            }
            ui.menu_button("Item", |ui| entry_menu(ui, entry, busy, selected, action));
        }
    }

    fn rows_ui(
        &mut self,
        ui: &mut Ui,
        palette: &AppPalette,
        height: f32,
        action: &mut Option<Action>,
    ) {
        let colors = WorkbenchColors::from_palette(palette);
        let mut hands = Vec::new();
        for slot in [
            SlotId::LeftHand,
            SlotId::RightHand,
            SlotId::LeftPrior1,
            SlotId::LeftPrior2,
            SlotId::LeftPrior3,
            SlotId::RightPrior2,
            SlotId::RightPrior1,
        ] {
            let card = self.summary.as_ref().and_then(|summary| summary.card(slot));
            if card.is_some() || matches!(slot, SlotId::LeftHand | SlotId::RightHand) {
                hands.push(Card {
                    key: Selected::Slot(slot),
                    id: card.map(|card| card.item_id),
                    label: slot.label().into(),
                    fallback: if card.is_some() {
                        "Loading preview…".into()
                    } else if self.inventory_error.is_some() {
                        "Offline".into()
                    } else {
                        "Empty".into()
                    },
                    detail: "Double-click to collect".into(),
                    source: card.map(|card| SourceRef::Slot(slot, card.item_id)),
                    color: if slot == SlotId::RightHand {
                        crate::hex_to_c32(&palette.right_hand_border)
                    } else {
                        colors.accent
                    },
                });
            }
        }
        let collected: Vec<_> = self
            .entries
            .iter()
            .map(|entry| entry_card(entry, Selected::Collected(entry.id), colors.border))
            .collect();
        let ids = hands
            .iter()
            .chain(&collected)
            .filter_map(|card| card.id)
            .collect();
        self.previews.retain(&ids);
        card_row(
            ui,
            "Hands",
            &hands,
            &mut self.previews,
            self.selected,
            colors,
            height,
            action,
        );
        ui.add_space(6.0);
        card_row(
            ui,
            "Collected",
            &collected,
            &mut self.previews,
            self.selected,
            colors,
            height,
            action,
        );
    }
}

enum SourceRef<'a> {
    Payload(&'a Payload),
    Slot(SlotId, uuid::Uuid),
}
struct Card<'a> {
    key: Selected,
    id: Option<uuid::Uuid>,
    label: String,
    fallback: String,
    detail: String,
    source: Option<SourceRef<'a>>,
    color: egui::Color32,
}

fn entry_card(entry: &Entry, key: Selected, color: egui::Color32) -> Card<'_> {
    let (fallback, detail) = match &entry.payload {
        Payload::Files { paths, cut } => (
            paths
                .first()
                .map(|path| {
                    super::super::display_path(std::path::Path::new(
                        path.file_name().unwrap_or_default(),
                    ))
                })
                .unwrap_or_default(),
            format!(
                "{} files · {}",
                paths.len(),
                if *cut { "Move" } else { "Copy" }
            ),
        ),
        Payload::Content(content) => (
            content.default_name(),
            format!("Captured {}", content.extension.to_uppercase()),
        ),
    };
    Card {
        key,
        id: Some(entry.id),
        label: entry.label.clone(),
        fallback,
        detail,
        source: Some(SourceRef::Payload(&entry.payload)),
        color,
    }
}

fn card_row(
    ui: &mut Ui,
    label: &str,
    cards: &[Card<'_>],
    previews: &mut Previews,
    selected: Option<Selected>,
    colors: WorkbenchColors,
    height: f32,
    action: &mut Option<Action>,
) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            vec2(90.0, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(vec2(90.0, height));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(label)
                            .small()
                            .color(colors.text_secondary),
                    )
                    .wrap(false),
                );
            },
        );
        egui::ScrollArea::horizontal()
            .id_source(("shelf_row", label))
            .max_height(height)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if label == "Collected" {
                        collection_actions(ui, colors, height - 12.0, action);
                    }
                    for card in cards {
                        card_ui(ui, card, previews, selected, colors, height - 12.0, action);
                    }
                });
            });
    });
}

fn collection_actions(
    ui: &mut Ui,
    colors: WorkbenchColors,
    height: f32,
    action: &mut Option<Action>,
) {
    for (title, hint, next) in [
        (
            "Add selected files",
            "Keep a group on this Shelf",
            Action::CollectSelection,
        ),
        (
            "Capture clipboard",
            "Keep current text, image or files",
            Action::Capture,
        ),
    ] {
        let (rect, response) = ui.allocate_exact_size(vec2(196.0, height), Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect(
            rect,
            3.0,
            colors.recessed,
            Stroke::new(1.0_f32, colors.border),
        );
        Icon::Plus.paint(
            &painter,
            Rect::from_center_size(pos2(rect.min.x + 20.0, rect.center().y), vec2(18.0, 18.0)),
            colors.text_muted,
        );
        painter.text(
            pos2(
                rect.min.x + 38.0,
                if height >= 48.0 {
                    rect.min.y + 12.0
                } else {
                    rect.center().y
                },
            ),
            if height >= 48.0 {
                Align2::LEFT_TOP
            } else {
                Align2::LEFT_CENTER
            },
            title,
            FontId::proportional(13.0),
            colors.text_secondary,
        );
        if height >= 48.0 {
            painter.text(
                rect.min + vec2(40.0, 31.0),
                Align2::LEFT_TOP,
                hint,
                FontId::proportional(11.0),
                colors.text_muted,
            );
        }
        response.clone().on_hover_text(match &next {
            Action::CollectSelection => "Select files in the folder, then click to keep them as a group. You can add more groups at any time.",
            _ => "Capture the desktop clipboard as a new collection. It stays available after the clipboard changes. Click again to capture another item.",
        });
        if response.clicked() {
            *action = Some(next);
        }
    }
}

fn card_ui(
    ui: &mut Ui,
    card: &Card<'_>,
    previews: &mut Previews,
    selected: Option<Selected>,
    colors: WorkbenchColors,
    height: f32,
    action: &mut Option<Action>,
) {
    let (rect, response) = ui.allocate_exact_size(
        vec2(196.0, height),
        Sense {
            focusable: false,
            ..Sense::click()
        },
    );
    if ui.is_rect_visible(rect) {
        request_preview(previews, card);
        if card.id.is_some_and(|id| previews.get(id).is_none()) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        paint_card(
            ui,
            rect,
            card,
            card.id.and_then(|id| previews.get(id)),
            selected == Some(card.key),
            colors,
        );
    }
    response.clone().on_hover_ui(|ui| {
        ui.strong(&card.label);
        ui.label(
            card.id
                .and_then(|id| previews.get(id))
                .map_or(&card.fallback, |ready| &ready.data.title),
        );
        ui.label(&card.detail);
        ui.label("Click to select. Use the Shelf toolbar to place or collect this item.");
    });
    if response.clicked() {
        *action = Some(Action::Select(card.key));
    }
    if response.double_clicked() {
        match (card.key, card.id) {
            (Selected::Slot(slot), Some(id)) => *action = Some(Action::CollectSlot(slot, id)),
            _ => {}
        }
    }
}

fn request_preview(previews: &mut Previews, card: &Card<'_>) {
    if let Some(id) = card.id
        && previews.needs(id)
    {
        let source = match &card.source {
            Some(SourceRef::Payload(payload)) => Source::Payload((*payload).clone()),
            Some(SourceRef::Slot(slot, id)) => Source::Slot(*slot, *id),
            None => return,
        };
        previews.request(id, source);
    }
}

fn paint_card(
    ui: &Ui,
    rect: Rect,
    card: &Card<'_>,
    ready: Option<&Ready>,
    selected: bool,
    colors: WorkbenchColors,
) {
    let painter = ui.painter_at(rect.shrink(1.0));
    if rect.height() < 44.0 {
        paint_compact_card(ui, rect, card, ready, selected, colors);
        return;
    }
    let border = if selected { colors.focus } else { card.color };
    painter.rect(
        rect.shrink(1.0),
        3.0,
        if selected {
            colors.raised
        } else {
            colors.panel
        },
        Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, border),
    );
    painter.line_segment(
        [
            rect.min + vec2(1.0, 22.0),
            pos2(rect.max.x - 1.0, rect.min.y + 22.0),
        ],
        Stroke::new(1.0_f32, colors.border),
    );
    painter.text(
        rect.min + vec2(9.0, 5.0),
        Align2::LEFT_TOP,
        &card.label,
        FontId::proportional(11.0),
        border,
    );
    let body = Rect::from_min_max(rect.min + vec2(8.0, 28.0), rect.max - vec2(8.0, 6.0));
    let media = Rect::from_min_size(body.min, vec2(72.0, body.height().max(1.0)));
    paint_media(ui, media, ready, colors);
    let text = Rect::from_min_max(pos2(media.max.x + 8.0, body.min.y), body.max);
    let title = ready.map_or(&card.fallback, |ready| &ready.data.title);
    let detail = ready.map_or(&card.detail, |ready| &ready.data.detail);
    let title_rect = Rect::from_min_max(
        text.min,
        pos2(
            text.max.x,
            if text.height() >= 35.0 {
                text.max.y - 16.0
            } else {
                text.max.y
            },
        ),
    );
    let clipped = ui.painter_at(title_rect);
    let galley = clipped.layout(
        title.clone(),
        FontId::proportional(12.0),
        colors.text_primary,
        text.width(),
    );
    clipped.galley(text.min, galley, colors.text_primary);
    if text.height() >= 35.0 {
        painter.text(
            pos2(text.min.x, text.max.y - 12.0),
            Align2::LEFT_TOP,
            detail,
            FontId::proportional(10.0),
            colors.text_muted,
        );
    }
}

fn paint_compact_card(
    ui: &Ui,
    rect: Rect,
    card: &Card<'_>,
    ready: Option<&Ready>,
    selected: bool,
    colors: WorkbenchColors,
) {
    let painter = ui.painter_at(rect);
    painter.rect(
        rect.shrink(1.0),
        3.0,
        colors.panel,
        Stroke::new(1.0_f32, if selected { colors.focus } else { card.color }),
    );
    let size = (rect.height() - 6.0).max(1.0);
    let media = Rect::from_center_size(
        pos2(rect.min.x + size / 2.0 + 5.0, rect.center().y),
        vec2(size, size),
    );
    paint_media(ui, media, ready, colors);
    painter.text(
        pos2(media.max.x + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        ready.map_or(&card.fallback, |ready| &ready.data.title),
        FontId::proportional(11.0),
        colors.text_primary,
    );
}

fn paint_media(ui: &Ui, rect: Rect, ready: Option<&Ready>, colors: WorkbenchColors) {
    let painter = ui.painter_at(rect);
    if let Some(texture) = ready.and_then(|ready| ready.texture.as_ref()) {
        let scale =
            (rect.width() / texture.size_vec2().x).min(rect.height() / texture.size_vec2().y);
        painter.image(
            texture.id(),
            Rect::from_center_size(rect.center(), texture.size_vec2() * scale),
            Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else if let Some(ready) = ready {
        if !ready.data.waveform.is_empty() {
            paint_waveform(
                &painter,
                rect,
                &ready.data.waveform,
                &ready.data.waveform_colors,
                colors.accent,
            );
        } else if !ready.data.text.is_empty() {
            let galley = painter.layout(
                ready.data.text.clone(),
                FontId::monospace(8.0),
                colors.text_secondary,
                rect.width(),
            );
            painter.galley(rect.min, galley, colors.text_secondary);
        } else {
            ready.data.icon.paint(
                &painter,
                Rect::from_center_size(rect.center(), vec2(24.0, 24.0)),
                colors.text_muted,
            );
        }
    } else {
        Icon::Clipboard.paint(
            &painter,
            Rect::from_center_size(rect.center(), vec2(24.0, 24.0)),
            colors.text_muted,
        );
    }
}

fn paint_waveform(
    painter: &egui::Painter,
    rect: Rect,
    peaks: &[f32],
    rgb: &[[u8; 3]],
    color: egui::Color32,
) {
    for (index, peak) in peaks.iter().enumerate() {
        let x = rect.min.x + (index as f32 + 0.5) * rect.width() / peaks.len() as f32;
        let extent = peak * rect.height() * 0.45;
        painter.line_segment(
            [
                pos2(x, rect.center().y - extent),
                pos2(x, rect.center().y + extent),
            ],
            Stroke::new(
                1.0_f32,
                rgb.get(index).map_or(color, |value| {
                    egui::Color32::from_rgb(value[0], value[1], value[2])
                }),
            ),
        );
    }
}

fn entry_menu(
    ui: &mut Ui,
    entry: &mut Entry,
    busy: bool,
    selected: Option<Selected>,
    action: &mut Option<Action>,
) {
    ui.add(
        egui::TextEdit::singleline(&mut entry.label)
            .hint_text("Group name")
            .desired_width(170.0),
    );
    if let Some(message) = &entry.availability {
        ui.label(message);
    }
    for (label, hand) in [
        ("Send to Left hand", HandSide::Left),
        ("Send to Right hand", HandSide::Right),
    ] {
        if ui
            .add_enabled(
                !busy && send_supported(&entry.payload),
                egui::Button::new(label),
            )
            .clicked()
        {
            *action = Some(Action::Send(entry.payload.clone(), hand));
            ui.close_menu();
        }
    }
    if let Some(selected) = selected
        && ui
            .add_enabled(!busy, egui::Button::new("Remove from Shelf"))
            .clicked()
    {
        *action = Some(Action::Remove(selected));
        ui.close_menu();
    }
}

fn send_supported(payload: &Payload) -> bool {
    match payload {
        Payload::Files { .. } => true,
        Payload::Content(content) => {
            content.mime.starts_with("image/")
                || content.mime.starts_with("text/")
                || matches!(
                    content.mime.as_str(),
                    "application/json" | "application/xml" | "UTF8_STRING"
                )
        }
    }
}
