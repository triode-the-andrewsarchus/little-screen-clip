//! Live Home rectangles share the native drawing camera and clipped viewport.
use super::*;
use crate::file_manager::home::{Card, Content};
use clip_studios::drawing_canvas::Viewport;
use uuid::Uuid;
mod projects;

pub(super) enum Action {
    Open(Uuid),
    Select(Uuid),
    Begin(Uuid),
    Drag(Uuid, [f64; 2], bool),
    End,
    Remove(Uuid),
    Unpin(Uuid),
    OpenDocument(std::path::PathBuf),
    OpenFolder(std::path::PathBuf),
    ReloadArtwork(Uuid),
}
pub(super) struct Renderer<'a, 'camera> {
    pub viewport: Viewport<'camera>,
    pub pins: &'a [Pin],
    pub arrange: bool,
    pub enabled: bool,
    pub selected: Option<Uuid>,
    pub unpin: bool,
}
impl Renderer<'_, '_> {
    pub fn cards(
        &self,
        ui: &mut Ui,
        cards: &[Card],
        projects: &mut crate::file_manager::home::projects::Projects,
        artwork: &mut crate::file_manager::home::artwork::Assets,
        palette: &AppPalette,
        actions: &mut Vec<Action>,
    ) {
        for card in cards
            .iter()
            .filter(|card| matches!(card.content, Content::Section { .. }))
        {
            self.card(ui, card, actions);
        }
        for card in cards
            .iter()
            .filter(|card| !matches!(card.content, Content::Section { .. }))
        {
            if let Content::Artwork { pin, .. } = card.content
                && let Some(source) = self.pins.iter().find(|source| source.id == pin)
                && self
                    .viewport
                    .bounds(card.bounds())
                    .intersects(self.viewport.rect)
            {
                artwork.paint(ui, self.viewport.bounds(card.bounds()), source, palette);
            }
            self.card(ui, card, actions);
            if matches!(
                card.content,
                Content::Projects { .. } | Content::Project { .. }
            ) {
                self.project_area(ui, card, projects, actions);
            }
        }
    }
    pub fn card(&self, ui: &mut Ui, card: &Card, actions: &mut Vec<Action>) {
        let rect = self.viewport.bounds(card.bounds());
        if !rect.intersects(self.viewport.rect) {
            return;
        }
        let scale = self.viewport.scale() as f32;
        match &card.content {
            Content::Artwork { .. } => {}
            Content::Section { title, color } => self.section(ui, card, rect, title, *color),
            Content::Projects { title } | Content::Project { title, .. } => {
                ui.painter()
                    .rect_filled(rect, 12.0, ui.visuals().panel_fill);
                self.section(ui, card, rect, title, [69, 132, 134]);
            }
            Content::Shortcut { pin, label } => {
                let Some(pin) = self.pins.iter().find(|target| target.id == *pin) else {
                    return;
                };
                self.shortcut(ui, rect, pin, label);
            }
        }
        let hit = match card.content {
            Content::Section { .. } | Content::Projects { .. } | Content::Project { .. } => {
                Rect::from_min_size(rect.min, vec2(rect.width(), 40.0 * scale))
            }
            _ => rect,
        };
        let response = ui.interact(
            hit.intersect(ui.clip_rect()),
            Id::new(("home_card", card.id)),
            Sense::click_and_drag(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, card_label(card, self.pins))
        });
        self.input(ui, &response, card, actions);
        self.menu(&response, card, actions);
        self.resize(ui, rect, card, actions);
    }
    fn input(&self, ui: &Ui, response: &egui::Response, card: &Card, actions: &mut Vec<Action>) {
        if self.enabled {
            if self.arrange {
                response.clone().on_hover_cursor(egui::CursorIcon::Grab);
            } else if matches!(card.content, Content::Shortcut { .. }) {
                response
                    .clone()
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                response.request_focus();
                if self.arrange {
                    actions.push(Action::Select(card.id));
                } else if let Content::Shortcut { pin, .. } = card.content {
                    actions.push(Action::Open(pin));
                }
            }
            if response.has_focus()
                && key(ui.ctx(), Modifiers::NONE, Key::Enter)
                && let Content::Shortcut { pin, .. } = card.content
            {
                actions.push(Action::Open(pin));
            }
            if self.arrange {
                self.drag(ui, response, card.id, false, actions);
            }
        }
    }
    fn menu(&self, response: &egui::Response, card: &Card, actions: &mut Vec<Action>) {
        response.context_menu(|ui| {
            if let Content::Artwork { pin, .. } = card.content {
                if ui.button("Open source").clicked() {
                    actions.push(Action::Open(pin));
                    ui.close_menu();
                }
                if ui.button("Reload artwork").clicked() {
                    actions.push(Action::ReloadArtwork(pin));
                    ui.close_menu();
                }
            }
            if let Content::Shortcut { pin, .. } = card.content {
                if ui.button("Open").clicked() {
                    actions.push(Action::Open(pin));
                    ui.close_menu();
                }
                if ui
                    .add_enabled(self.unpin, egui::Button::new("Unpin reference"))
                    .clicked()
                {
                    actions.push(Action::Unpin(pin));
                    ui.close_menu();
                }
            }
            if ui
                .add_enabled(
                    self.enabled && self.arrange,
                    egui::Button::new("Remove from desk"),
                )
                .clicked()
            {
                actions.push(Action::Remove(card.id));
                ui.close_menu();
            }
        });
    }
    fn resize(&self, ui: &Ui, rect: Rect, card: &Card, actions: &mut Vec<Action>) {
        let scale = self.viewport.scale() as f32;
        if self.selected == Some(card.id) && self.arrange {
            ui.painter().rect_stroke(
                rect,
                9.0,
                egui::Stroke::new(1.8_f32, Color32::from_rgb(64, 126, 150)),
            );
            let edge = (16.0 * scale).clamp(8.0, 20.0);
            let handle =
                Rect::from_min_size(rect.right_bottom() - vec2(edge, edge), vec2(edge, edge));
            ui.painter()
                .rect_filled(handle.shrink(2.0), 3.0, Color32::from_rgb(64, 126, 150));
            let response = ui.interact(
                handle.intersect(ui.clip_rect()),
                Id::new(("home_resize", card.id)),
                Sense::drag(),
            );
            if self.enabled {
                self.drag(ui, &response, card.id, true, actions);
            }
        }
    }
    fn drag(
        &self,
        ui: &Ui,
        response: &egui::Response,
        id: Uuid,
        resize: bool,
        actions: &mut Vec<Action>,
    ) {
        if response.drag_started() {
            actions.push(Action::Begin(id));
        }
        if response.dragged() {
            let delta = ui.input(|input| input.pointer.delta());
            actions.push(Action::Drag(
                id,
                [
                    f64::from(delta.x) / self.viewport.scale(),
                    f64::from(delta.y) / self.viewport.scale(),
                ],
                resize,
            ));
        }
        if response.drag_stopped() {
            actions.push(Action::End);
        }
    }
    fn section(&self, ui: &Ui, card: &Card, rect: Rect, title: &str, color: [u8; 3]) {
        let ink = Color32::from_rgb(color[0], color[1], color[2]);
        let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
        painter.rect(
            rect,
            12.0,
            ink.linear_multiply(0.07),
            egui::Stroke::new(1.3_f32, ink.linear_multiply(0.6)),
        );
        painter.text(
            rect.left_top() + vec2(16.0, 12.0) * self.viewport.scale() as f32,
            egui::Align2::LEFT_TOP,
            title,
            FontId::proportional((22.0 * self.viewport.scale() as f32).clamp(6.0, 64.0)),
            ui.visuals().text_color(),
        );
        if self.selected == Some(card.id) {
            painter.line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                egui::Stroke::new(2.0_f32, ink),
            );
        }
    }
    fn shortcut(&self, ui: &Ui, rect: Rect, pin: &Pin, label: &str) {
        let scale = self.viewport.scale() as f32;
        let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
        painter.rect(
            rect,
            10.0,
            ui.visuals().panel_fill,
            egui::Stroke::new(1.2_f32, Color32::from_rgb(183, 178, 164)),
        );
        let ink = ui.visuals().text_color();
        let icon = if pin.folder { Icon::Folder } else { Icon::Text };
        icon.paint(
            &painter,
            Rect::from_min_size(
                rect.min + vec2(16.0, 16.0) * scale,
                vec2(30.0, 30.0) * scale,
            ),
            Color32::from_rgb(76, 127, 128),
        );
        painter.text(
            rect.min + vec2(60.0, 18.0) * scale,
            egui::Align2::LEFT_TOP,
            if label.is_empty() {
                pin_name(pin)
            } else {
                label.into()
            },
            FontId::proportional((18.0 * scale).clamp(6.0, 64.0)),
            ink,
        );
        painter.text(
            rect.min + vec2(16.0, 64.0) * scale,
            egui::Align2::LEFT_TOP,
            display_path(&pin.path),
            FontId::proportional((11.0 * scale).clamp(5.0, 32.0)),
            ui.visuals().weak_text_color(),
        );
    }
}
fn card_label(card: &Card, pins: &[Pin]) -> String {
    match &card.content {
        Content::Section { title, .. }
        | Content::Projects { title }
        | Content::Project { title, .. } => title.clone(),
        Content::Shortcut { pin, label } | Content::Artwork { pin, label } => {
            if label.is_empty() {
                pins.iter()
                    .find(|p| p.id == *pin)
                    .map_or_else(|| "Unavailable shortcut".into(), pin_name)
            } else {
                label.clone()
            }
        }
    }
}
pub(super) fn pin_name(pin: &Pin) -> String {
    pin.path.file_name().map_or_else(
        || display_path(&pin.path),
        |name| name.to_string_lossy().into_owned(),
    )
}
