//! One-run limits are reviewed without changing global preferences.
use super::*;

pub(super) fn review(
    ui: &mut Ui,
    drive: &Drive,
    limits: &mut CatalogLimits,
    status: &DriveStatus,
    requests: &mut Vec<DriveRequest>,
) -> bool {
    ui.label(format!(
        "Partition: {} · {}",
        drive.label,
        display_path(&drive.device)
    ));
    ui.small(format!("Drive ID: {}", drive.id));
    ui.label("Save names, sizes, dates and hierarchy without copying file contents. These limits apply to this capture only. A reached limit is reported as partial coverage.");
    egui::Grid::new("catalog_capture_limits")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Maximum entries");
            ui.add(
                egui::DragValue::new(&mut limits.entries)
                    .speed(1000)
                    .clamp_range(0..=u64::MAX),
            );
            ui.end_row();
            ui.label("Database bytes");
            ui.add(
                egui::DragValue::new(&mut limits.bytes)
                    .speed(1024 * 1024)
                    .clamp_range(128 * 1024..=i64::MAX as u64),
            );
            ui.end_row();
            ui.label("Maximum depth");
            ui.add(egui::DragValue::new(&mut limits.depth).clamp_range(1..=usize::MAX));
            ui.end_row();
            ui.label("Time limit (seconds)");
            ui.add(
                egui::DragValue::new(&mut limits.timeout_seconds)
                    .speed(60)
                    .clamp_range(1..=u64::MAX),
            );
            ui.end_row();
        });
    ui.small(format!(
        "Database budget: {}",
        little_clip_core::file_inventory::format_size(limits.bytes)
    ));
    ui.horizontal(|ui| {
        if ui.button("Use larger limits").clicked() {
            limits.entries = limits.entries.saturating_mul(4);
            limits.bytes = limits.bytes.saturating_mul(4).min(i64::MAX as u64);
            limits.depth = limits.depth.saturating_mul(2);
            limits.timeout_seconds = limits.timeout_seconds.saturating_mul(3);
        }
        if ui.button("Use configured limits").clicked() {
            *limits = CatalogLimits::from(&status.config);
        }
    });
    ui.label(format!("Up to {} independent catalog capture(s) can run together. Backups wait for captures; other drives can continue syncing. Cancel remaining work from Jobs.", status.config.catalog_workers));
    let valid = limits.validate();
    if let Err(error) = valid {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    let start = ui
        .add_enabled(valid.is_ok(), egui::Button::new("Start catalog"))
        .clicked();
    if start {
        requests.push(DriveRequest::CatalogWithLimits {
            drive: drive.id.clone(),
            limits: (*limits).into(),
        });
    }
    !start
}
