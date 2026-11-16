//! Sidebar Mounts section displaying active filesystem mounts, removable drives,
//! fuse-binwalk mounts, and unmounted storage devices.

use super::*;
use crate::file_manager::mounts::{MountEntry, MountKind};

impl FileManager {
    pub(super) fn mounts_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("Mounts");
            if ui.small_button("Jobs…").clicked() {
                self.drives.open_details();
            }
        });
        ui.separator();

        let mut open = None;
        let mut mount_target = None;
        let mut unmount_target = None;

        egui::ScrollArea::vertical()
            .id_source("files_mount_rows")
            .min_scrolled_height(0.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.ram_mounts_ui(ui);
                for index in 0..self.mounts.entries.len() {
                    let entry = self.mounts.entries[index].clone();
                    let (o, m, u) = self.mount_row_ui(ui, &entry, colors);
                    if o.is_some() {
                        open = o;
                    }
                    if m.is_some() {
                        mount_target = m;
                    }
                    if u.is_some() {
                        unmount_target = u;
                    }
                }
            });

        if let Some(path) = open {
            self.navigate(path, Navigation::New);
        }
        if let Some(entry) = mount_target {
            self.set_status(
                Icon::Hourglass,
                format!("Mounting '{}'…", entry.label),
                false,
            );
            self.mounts
                .request_mount(&entry, super::super::BackgroundJobs::global());
        }
        if let Some(entry) = unmount_target {
            self.set_status(
                Icon::Hourglass,
                format!("Unmounting '{}'…", entry.label),
                false,
            );
            if !self.eject_enrolled_drive(&entry) {
                self.mounts
                    .request_unmount(&entry, super::super::BackgroundJobs::global());
            }
        }
    }

    fn mount_row_ui(
        &mut self,
        ui: &mut Ui,
        entry: &MountEntry,
        colors: WorkbenchColors,
    ) -> (Option<PathBuf>, Option<MountEntry>, Option<MountEntry>) {
        let mut open = None;
        let mut mount = None;
        let mut unmount = None;

        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        let has_eject = entry.is_mounted && entry.is_removable;
        // An open popup must follow its mount across inventory reordering.
        let identity = Id::new((&entry.id, &entry.device_path, &entry.mount_point));
        let (body_rect, eject_rect, eject_response, response) =
            allocate_mount_row(ui, rect, has_eject, identity);

        let is_active = entry
            .mount_point
            .as_deref()
            .is_some_and(|mp| self.path() == mp || self.path().starts_with(mp));
        let hovering = response.hovered() || eject_response.as_ref().is_some_and(|r| r.hovered());

        row_background(ui, rect, is_active, hovering, colors);

        if let Some(mp) = &entry.mount_point
            && ui.is_enabled()
        {
            self.drop_targets
                .push((rect.intersect(ui.clip_rect()), mp.clone()));
        }

        paint_mount_row(ui, rect, body_rect, entry, is_active, colors);

        if let (Some(e_rect), Some(e_resp)) = (eject_rect, &eject_response)
            && render_eject_button(ui, e_rect, e_resp, entry.kind, colors)
        {
            unmount = Some(entry.clone());
        }

        if response.clicked() {
            response.request_focus();
            if entry.is_mounted {
                open = entry.mount_point.clone();
            } else {
                mount = Some(entry.clone());
            }
        }

        if !super::popup_open(ui.ctx()) {
            response.clone().on_hover_text(mount_tooltip(entry));
        }

        response.context_menu(|ui| {
            mount_context_menu(ui, entry, &mut open, &mut unmount, &mut mount);
            self.drive_context_menu(ui, entry);
        });

        (open, mount, unmount)
    }
}

fn allocate_mount_row(
    ui: &mut Ui,
    rect: Rect,
    has_eject: bool,
    identity: Id,
) -> (Rect, Option<Rect>, Option<egui::Response>, egui::Response) {
    let (body_rect, eject_rect, eject_response) = if has_eject {
        let eject_width = 24.0;
        let e_rect = Rect::from_min_max(
            pos2(rect.max.x - eject_width - 4.0, rect.min.y + 2.0),
            pos2(rect.max.x - 4.0, rect.max.y - 2.0),
        );
        let b_rect = Rect::from_min_max(rect.min, pos2(e_rect.min.x, rect.max.y));
        let e_resp = ui.interact(e_rect, identity.with("eject"), Sense::click());
        (b_rect, Some(e_rect), Some(e_resp))
    } else {
        (rect, None, None)
    };
    let response = ui.interact(body_rect, identity.with("row"), Sense::click());
    (body_rect, eject_rect, eject_response, response)
}

fn paint_mount_row(
    ui: &mut Ui,
    rect: Rect,
    body_rect: Rect,
    entry: &MountEntry,
    is_active: bool,
    colors: WorkbenchColors,
) {
    let icon_rect = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(20.0, 20.0));
    let icon_color = if !entry.is_mounted {
        colors.text_muted
    } else if is_active {
        colors.accent
    } else {
        colors.text_secondary
    };
    entry.kind.icon().paint(ui.painter(), icon_rect, icon_color);

    let label_color = if !entry.is_mounted {
        colors.text_secondary
    } else {
        colors.text_primary
    };
    let display_label = if entry.is_mounted {
        entry.label.clone()
    } else {
        format!("{} (unmounted)", entry.label)
    };
    paint_text(
        ui,
        rect,
        32.0,
        body_rect.width() - 32.0,
        &display_label,
        label_color,
    );
}

fn render_eject_button(
    ui: &mut Ui,
    e_rect: Rect,
    e_resp: &egui::Response,
    kind: MountKind,
    colors: WorkbenchColors,
) -> bool {
    if e_resp.hovered() {
        ui.painter().rect_filled(e_rect, 3.0, colors.recessed);
        ui.painter()
            .rect_stroke(e_rect, 3.0, egui::Stroke::new(1.0_f32, colors.accent));
    }
    let eject_icon_rect = Rect::from_min_size(e_rect.center() - vec2(8.0, 8.0), vec2(16.0, 16.0));
    let eject_icon_color = if e_resp.hovered() {
        colors.text_primary
    } else {
        colors.text_secondary
    };
    Icon::Eject.paint(ui.painter(), eject_icon_rect, eject_icon_color);

    if !super::popup_open(ui.ctx()) {
        let label = if kind == MountKind::FuseBinwalk {
            "Unmount fuse-binwalk"
        } else {
            "Eject / Unmount"
        };
        e_resp.clone().on_hover_text(label);
    }

    e_resp.clicked()
}

fn mount_tooltip(entry: &MountEntry) -> String {
    if entry.is_mounted {
        if let Some(mp) = &entry.mount_point {
            if !entry.detail.is_empty() {
                format!("{}\n{}", mp.display(), entry.detail)
            } else {
                mp.display().to_string()
            }
        } else {
            entry.detail.clone()
        }
    } else if let Some(dev) = &entry.device_path {
        format!("{}\n{}\nClick to mount", dev.display(), entry.detail)
    } else {
        format!("{}\nClick to mount", entry.detail)
    }
}

fn mount_context_menu(
    ui: &mut Ui,
    entry: &MountEntry,
    open: &mut Option<PathBuf>,
    unmount: &mut Option<MountEntry>,
    mount: &mut Option<MountEntry>,
) {
    if entry.is_mounted {
        if let Some(mp) = &entry.mount_point
            && ui.button("Open in Files").clicked()
        {
            *open = Some(mp.clone());
            ui.close_menu();
        }
        if entry.is_removable {
            let label = if entry.kind == MountKind::FuseBinwalk {
                "Unmount fuse-binwalk"
            } else {
                "Eject / Unmount"
            };
            if ui.button(label).clicked() {
                *unmount = Some(entry.clone());
                ui.close_menu();
            }
        }
    } else if ui.button("Mount").clicked() {
        *mount = Some(entry.clone());
        ui.close_menu();
    }

    if let Some(target) = entry.mount_point.as_ref().or(entry.device_path.as_ref())
        && ui.button("Copy path").clicked()
    {
        ui.output_mut(|o| o.copied_text = target.display().to_string());
        ui.close_menu();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_mount_menu_keeps_its_partition_when_inventory_reorders() {
        let context = egui::Context::default();
        let mut frame = 0;
        let mut render = |order: [&str; 2], events: Vec<egui::Event>| {
            frame += 1;
            let mut open = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0))),
                time: Some(frame as f64 * 0.1),
                events,
                ..Default::default()
            };
            let _ = context.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    for partition in &order {
                        let (rect, _) = ui.allocate_exact_size(vec2(250.0, 32.0), Sense::hover());
                        let (_, _, _, response) =
                            allocate_mount_row(ui, rect, true, Id::new(partition));
                        response.context_menu(|ui| {
                            open.push((*partition).to_owned());
                            ui.label(*partition);
                        });
                    }
                });
            });
            open
        };
        let position = pos2(40.0, 20.0);
        render(["portable", "other"], vec![]);
        for pressed in [true, false] {
            render(
                ["portable", "other"],
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(render(["portable", "other"], vec![]), vec!["portable"]);
        assert_eq!(render(["other", "portable"], vec![]), vec!["portable"]);
    }
}
