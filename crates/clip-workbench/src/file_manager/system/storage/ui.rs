use super::{Layout, Storage, bytes};
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};

pub(in crate::file_manager::system) fn paint(
    ui: &mut Ui,
    rect: Rect,
    storage: &Storage,
    colors: WorkbenchColors,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.panel);
    if storage.partitions.is_empty()
        && (!storage.dependencies.is_empty() || storage.backing.is_some())
    {
        let mut names = storage
            .dependencies
            .iter()
            .take(3)
            .map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        if let Some(backing) = &storage.backing {
            names.insert(
                0,
                backing
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        names.push(storage.name.clone());
        names.extend(storage.holders.iter().take(2).map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }));
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            names.join(" → "),
            FontId::proportional(13.0),
            colors.text_primary,
        );
        return;
    }
    let Some(capacity) = storage.disk_capacity.filter(|value| *value != 0) else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Layout unavailable",
            FontId::proportional(12.0),
            colors.text_secondary,
        );
        return;
    };
    if storage.partitions.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No partition ranges observed",
            FontId::proportional(12.0),
            colors.text_secondary,
        );
    }
    for (index, partition) in storage.partitions.iter().enumerate() {
        let left =
            rect.left() + (partition.start as f64 / capacity as f64 * rect.width() as f64) as f32;
        let right = rect.left()
            + ((partition.start.saturating_add(partition.bytes)) as f64 / capacity as f64
                * rect.width() as f64) as f32;
        let bar = Rect::from_min_max(
            pos2(left, rect.top()),
            pos2(right.max(left + 0.5).min(rect.right()), rect.bottom()),
        );
        let color = if partition.selected {
            colors.accent
        } else {
            let shades = [
                Color32::from_rgb(78, 128, 169),
                Color32::from_rgb(80, 145, 118),
                Color32::from_rgb(150, 120, 173),
                Color32::from_rgb(169, 137, 77),
            ];
            shades[index % shades.len()]
        };
        painter.rect_filled(
            bar,
            1.0,
            if storage.retained {
                color.gamma_multiply(0.5)
            } else {
                color
            },
        );
        if bar.width() > 70.0 {
            painter.with_clip_rect(bar.intersect(rect)).text(
                bar.center(),
                Align2::CENTER_CENTER,
                &partition.name,
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        if !crate::file_manager::ui::popup_open(ui.ctx()) {
            ui.interact(
                bar,
                ui.id().with(("partition-preview", &partition.path)),
                Sense::hover(),
            )
            .on_hover_text(partition_hover(partition, storage));
        }
    }
    if !crate::file_manager::ui::popup_open(ui.ctx()) {
        ui.interact(rect, ui.id().with(("partition-layout", &storage.disk)), Sense::hover())
            .on_hover_text(format!("{}\nCapacity: {}\nCoverage: {}\nGray areas are outside observed partition ranges, not a free-space measurement.", storage.disk.display(), bytes(capacity), storage.coverage));
    }
}
fn partition_hover(partition: &super::Partition, storage: &Storage) -> String {
    let geometry = format!(
        "{}\n{}\nStart: {} · Size: {}",
        partition.name,
        partition.path.display(),
        bytes(partition.start),
        bytes(partition.bytes)
    );
    if storage.layout == Layout::Flash {
        return format!(
            "{geometry}\nObserved raw-flash extent, not filesystem usage or per-block health.{}",
            if storage.retained {
                " Last observation."
            } else {
                ""
            }
        );
    }
    format!(
        "{geometry}\nFilesystem metadata: {} · label {} · UUID {}\nMounts: {}\n{}",
        partition.filesystem.as_deref().unwrap_or("unavailable"),
        partition.label.as_deref().unwrap_or("unavailable"),
        partition.uuid.as_deref().unwrap_or("unavailable"),
        partition
            .mounts
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        if storage.retained {
            "Last observed layout"
        } else {
            "Observed sysfs extent; not filesystem usage"
        }
    )
}
pub(in crate::file_manager::system) fn inspector(
    ui: &mut Ui,
    storage: &Storage,
    colors: WorkbenchColors,
) {
    ui.heading(if storage.layout == Layout::Flash {
        "Flash partition layout"
    } else {
        "Storage topology"
    });
    ui.label(storage.summary());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
    paint(ui, rect, storage, colors);
    ui.label(format!(
        "Partition inventory: {} · disk {}",
        storage.coverage,
        storage.disk.display()
    ));
    for (label, targets) in [
        ("Backing block devices", &storage.dependencies),
        ("Holders", &storage.holders),
    ] {
        if !targets.is_empty() {
            ui.label(format!(
                "{label}: {}",
                targets
                    .iter()
                    .map(|path| path.file_name().unwrap_or_default().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" → ")
            ));
        }
    }
    ui.separator();
}
