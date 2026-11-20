//! Optional regional terrain download planning and its explicit size review.
use super::*;
use crate::file_manager::geography_downloader::viewfinder::{self, Plan};

#[derive(Default)]
pub(super) struct Regional {
    pub viewport: [f32; 2],
    job: Option<Task<Result<Plan, String>>>,
    plan: Option<Plan>,
    error: Option<String>,
    coverage: Option<([i32; 4], Coverage)>,
}

enum Coverage {
    Checking(Task<bool>),
    Checked(bool),
}

impl Regional {
    pub fn invalidate_coverage(&mut self) {
        self.coverage = None;
    }

    fn coverage(
        &mut self,
        ui: &egui::Ui,
        assets: Option<&Arc<BasemapAssets>>,
        [west, south, east, north]: [f64; 4],
    ) -> Option<bool> {
        let Some(assets) = assets else {
            return Some(false);
        };
        let cells = [
            west.floor() as i32,
            south.floor() as i32,
            east.ceil() as i32,
            north.ceil() as i32,
        ];
        if self.coverage.as_ref().is_none_or(|(key, _)| *key != cells) {
            let assets = assets.clone();
            let job = Task::start(
                asset_workers(),
                std::time::Duration::from_secs(20),
                move |cancel| assets.has_local_1_arcsecond_coverage(cells, cancel),
            );
            self.coverage = Some((cells, Coverage::Checking(job)));
        }
        let (_, state) = self.coverage.as_mut()?;
        if let Coverage::Checking(job) = state {
            if let Some(result) = job.poll() {
                *state = Coverage::Checked(result.unwrap_or(false));
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(80));
            }
        }
        match *state {
            Coverage::Checked(covered) => Some(covered),
            Coverage::Checking(_) => None,
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        camera: GlobeCamera,
        assets: Option<&Arc<BasemapAssets>>,
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
        ui.hyperlink_to(
            "Viewfinder 1″ terrain (about 30 m)",
            "https://viewfinderpanoramas.org/dem1d.html",
        );
        if let Some(bounds) = bounds(camera, self.viewport).filter(|b| viewfinder::region_fits(*b))
        {
            match self.coverage(ui, assets, bounds) {
                Some(true) => {
                    ui.label("1″ terrain is already installed for this view.");
                }
                Some(false)
                    if self.job.is_none()
                        && ui
                            .button("Find 1″ terrain downloads for this view…")
                            .clicked() =>
                {
                    self.plan = None;
                    self.error = None;
                    let config = config.clone();
                    self.job = Some(Task::start(
                        asset_workers(),
                        std::time::Duration::from_secs(180),
                        move |cancel| viewfinder::plan(&config, bounds, cancel),
                    ));
                }
                None => {
                    ui.label("Checking installed 1″ terrain…");
                }
                _ => {}
            }
        } else {
            self.invalidate_coverage();
            ui.label("Zoom in on a region in 2D Map to find 1″ terrain downloads.");
        }
        if let Some(plan) = &self.plan {
            let count = plan.archives.len();
            if count == 0 {
                ui.label(format!(
                    "Viewfinder archives for the estimated region are already installed ({}).",
                    plan.already_installed
                ));
            } else {
                ui.label(format!("{count} archives · {} download · {} uncompressed terrain · up to {} additional space during installation", bytes(plan.download_bytes), bytes(plan.installed_bytes), bytes(plan.peak_bytes)));
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
                let remaining = plan.free_bytes;
                let allowed = plan.peak_bytes <= remaining;
                if !allowed {
                    ui.label(format!(
                        "This estimate exceeds the {} available on the geography filesystem.",
                        bytes(remaining)
                    ));
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
fn bounds(mut camera: GlobeCamera, size: [f32; 2]) -> Option<[f64; 4]> {
    if camera.globe_blend() > 0.0 || size.iter().any(|v| *v <= 0.0) {
        return None;
    }
    camera.constrain_view(size[0], size[1]);
    let scale = camera.scale_px(size[0], size[1]);
    let half_lat = (size[1] as f64 / (2.0 * scale)).to_degrees();
    let half_lon = (size[0] as f64
        / (2.0 * scale * camera.center_lat.to_radians().cos().abs().max(0.25)))
    .to_degrees();
    Some([
        camera.center_lon - half_lon,
        (camera.center_lat - half_lat).max(-90.0),
        camera.center_lon + half_lon,
        (camera.center_lat + half_lat).min(90.0),
    ])
}
