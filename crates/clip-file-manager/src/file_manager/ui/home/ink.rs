//! Compact tablet-only brush controls; ink shares the Home layout history.
use super::*;
use crate::file_manager::home::ink::Color;
impl FileManager {
    pub(super) fn home_ink_controls(&mut self, ui: &mut Ui, palette: &AppPalette) {
        if !self.home_surface.ink.available && self.home_surface.session.board.ink.is_empty() {
            return;
        }
        let mut clear = false;
        ui.menu_button("Ink",|ui| {
            let ink=&mut self.home_surface.ink;
            ui.checkbox(&mut ink.enabled,"Draw with tablet pen");
            ui.weak("Arrange lets the pen place artwork instead.");
            if !ink.available {ui.weak("Connect a pressure-sensitive pen to draw.");}
            ui.horizontal(|ui| {ui.selectable_value(&mut ink.marker,false,"Pen");ui.selectable_value(&mut ink.marker,true,"Marker");});
            ui.horizontal(|ui| {ui.label("Width");for width in [0.75,1.5,3.] {ui.selectable_value(&mut ink.width,width,format!("{width}"));}});
            ui.horizontal(|ui| {
                for (color,label) in [(Color::Theme,"Theme ink"),(Color::Teal,"Teal"),(Color::Amber,"Amber"),(Color::Rose,"Rose")] {
                    ui.selectable_value(&mut ink.color,color,label);
                }
            });
            ui.separator();
            clear=ui.add_enabled(!self.home_surface.session.board.ink.is_empty(),
                clip_ui::buttons::ButtonRole::Destructive.button("Clear tablet ink",palette))
                .on_hover_text("Removes tablet strokes only. Undo restores them; imported artwork stays in place.").clicked();
        });
        if clear {
            self.home_surface.session.edit(|board| board.ink.clear());
        }
    }
    pub(super) fn home_ink_feedback(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let ink = &mut self.home_surface.ink;
        if let Some(error) = ink.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ink.drawing()
                    && ui
                        .add(
                            clip_ui::buttons::ButtonRole::Destructive
                                .button("Discard unfinished ink", palette),
                        )
                        .clicked()
                {
                    ink.discard();
                }
                if ui
                    .add(
                        clip_ui::buttons::ButtonRole::Secondary
                            .button("Dismiss", palette)
                            .small(),
                    )
                    .clicked()
                {
                    ink.error = None;
                }
            });
        }
    }
}
