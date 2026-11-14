//! One-line location context; older ancestors stay accessible in overflow.
use super::*;

fn tail(widths: &[f32], budget: f32) -> usize {
    let mut start = 0;
    let mut used: f32 = widths.iter().sum();
    while start + 1 < widths.len() && used > budget {
        used -= widths[start];
        start += 1;
    }
    start
}
impl FileManager {
    pub(super) fn column_path_bar(&self, ui: &mut Ui, colors: WorkbenchColors) -> Option<PathBuf> {
        let selected = self
            .location()
            .and_then(|location| location.selected.as_deref());
        let entry = selected.and_then(|path| {
            self.displayed_entries()
                .into_iter()
                .find(|entry| entry.path == path)
        });
        let file = entry.is_some_and(|entry| !entry.directory);
        let path = selected.unwrap_or(self.path());
        let logical = self
            .archives
            .label(path)
            .unwrap_or_else(|| path.to_path_buf());
        let mut paths: Vec<_> = logical.ancestors().collect();
        paths.reverse();
        let font = FontId::proportional(12.0);
        let labels: Vec<_> = paths
            .iter()
            .map(|path| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
            })
            .collect();
        let widths: Vec<_> = labels
            .iter()
            .map(|label| {
                ui.fonts(|fonts| {
                    fonts
                        .layout_no_wrap(label.to_string(), font.clone(), colors.text_secondary)
                        .size()
                        .x
                })
                .clamp(16.0, 160.0)
                    + 20.0
            })
            .collect();
        let start = tail(&widths, (ui.available_width() - 36.0).max(0.0));
        let mut target = None;
        ui.push_id("miller-path-bar", |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                if start > 0 {
                    ui.menu_button("…", |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(320.0)
                            .show(ui, |ui| {
                                for path in paths[..start].iter().rev() {
                                    if ui.button(display_path(path)).clicked() {
                                        target = Some(path.to_path_buf());
                                        ui.close_menu();
                                    }
                                }
                            });
                    })
                    .response
                    .on_hover_text("Earlier folders");
                }
                for index in start..paths.len() {
                    if index > start || start > 0 {
                        ui.weak("›");
                    }
                    let is_file = file && index + 1 == paths.len();
                    let width = (widths[index] - 20.0).min(ui.available_width().max(1.0));
                    let response = ui
                        .add_sized(
                            [width, 22.0],
                            egui::Label::new(
                                egui::RichText::new(labels[index].as_ref())
                                    .font(font.clone())
                                    .color(if index + 1 == paths.len() {
                                        colors.text_primary
                                    } else {
                                        colors.text_secondary
                                    }),
                            )
                            .truncate(true)
                            .sense(if is_file {
                                Sense::hover()
                            } else {
                                Sense::click()
                            }),
                        )
                        .on_hover_text(display_path(paths[index]));
                    if !is_file && response.clicked() {
                        target = Some(paths[index].to_path_buf());
                    }
                    response.context_menu(|ui| {
                        if ui.button("Copy path").clicked() {
                            ui.output_mut(|output| output.copied_text = display_path(paths[index]));
                            ui.close_menu();
                        }
                    });
                }
            });
        });
        target
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_preserves_the_nearest_folders_and_never_drops_the_leaf() {
        assert_eq!(tail(&[30.0, 90.0, 100.0], 250.0), 0);
        assert_eq!(tail(&[30.0, 90.0, 100.0], 195.0), 1);
        assert_eq!(tail(&[30.0, 90.0, 100.0], 10.0), 2);
    }
}
