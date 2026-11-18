//! Optional regional terrain download planning and its explicit size review.
use super::*;
use crate::file_manager::geography_downloader::viewfinder::{self, Plan};

#[derive(Default)]
pub(super) struct Regional {
    pub viewport: [f32; 2],
    job: Option<Task<Result<Plan, String>>>,
    plan: Option<Plan>,
    error: Option<String>,
}
impl Regional {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        camera: GlobeCamera,
        config: &AppConfig,
        downloader: &mut AssetDownloader,
    ) {
        if let Some(job) = &self.job {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
            ui.label("Checking Viewfinder coverage and archive sizes…");
            if let Some(result) = job.poll() {
                self.job = None;
                match result {
                    Ok(Ok(plan)) => self.plan = Some(plan),
                    Ok(Err(error)) => self.error = Some(error),
                    Err(error) => self.error = Some(format!("Terrain planning failed: {error:?}")),
                }
            }
        }
        ui.separator();
        ui.label(
            "Optional 1″ terrain (about 30 m). Large regional downloads; excluded from Get all.",
        );
        ui.hyperlink_to(
            "Viewfinder coverage and source information",
            "https://viewfinderpanoramas.org/dem1d.html",
        );
        if ui
            .add_enabled(
                self.job.is_none() && downloader.active.is_none(),
                egui::Button::new("Estimate 1″ terrain for this view…"),
            )
            .clicked()
        {
            self.plan = None;
            self.error = None;
            match bounds(camera, self.viewport) {
                Ok(bounds) => {
                    let config = config.clone();
                    self.job = Some(Task::start(
                        asset_workers(),
                        std::time::Duration::from_secs(180),
                        move |cancel| viewfinder::plan(&config, bounds, cancel),
                    ));
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(plan) = &self.plan {
            let count = plan.archives.len();
            if count == 0 {
                ui.label(format!(
                    "All terrain archives for this view are already installed ({}).",
                    plan.already_installed
                ));
            } else {
                ui.label(format!("{} archives: {} download; {} uncompressed terrain; up to {} additional space during installation.",count,bytes(plan.download_bytes),bytes(plan.installed_bytes),bytes(plan.peak_bytes)));
                ui.label("Terrain is stored compressed; disk estimates use its uncompressed upper bound.");
                ui.label("Archives include surrounding terrain beyond the current view. Sizes come from server ZIP metadata; estimates are cached.");
                ui.label(format!(
                    "Archives: {}",
                    plan.archives
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                let remaining = plan.budget_remaining;
                let allowed = plan.peak_bytes <= remaining;
                if !allowed {
                    ui.label(format!("Needs more than the remaining {} geography budget. Increase geography.max_assets_bytes or choose a smaller view.",bytes(remaining)));
                }
                if ui
                    .add_enabled(
                        allowed && downloader.active.is_none(),
                        egui::Button::new(format!(
                            "Download {} of 1″ terrain",
                            bytes(plan.download_bytes)
                        )),
                    )
                    .clicked()
                {
                    downloader.start_viewfinder(config, plan.clone());
                    self.plan = None;
                }
            }
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
    }
}
fn bytes(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2} GB", n as f64 / 1e9)
    } else {
        format!("{:.1} MB", n as f64 / 1e6)
    }
}
fn bounds(mut camera: GlobeCamera, size: [f32; 2]) -> Result<[f64; 4], String> {
    if camera.globe_blend() > 0.0 || size.iter().any(|v| *v <= 0.0) {
        return Err("Use 2D Map and zoom into the region you want to download".into());
    }
    camera.constrain_view(size[0], size[1]);
    let scale = camera.scale_px(size[0], size[1]);
    let half_lat = (size[1] as f64 / (2.0 * scale)).to_degrees();
    let half_lon = (size[0] as f64
        / (2.0 * scale * camera.center_lat.to_radians().cos().abs().max(0.25)))
    .to_degrees();
    Ok([
        camera.center_lon - half_lon,
        camera.center_lat - half_lat,
        camera.center_lon + half_lon,
        camera.center_lat + half_lat,
    ])
}
