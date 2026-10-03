//! Read-only Gaussian scene navigation with bounded asynchronous GPU rendering.

use crate::icons::Icon;
use crate::shell::StudioShell;
use eframe::egui::{self, Vec2};
use little_clip_core::palette::AppPalette;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use little_clip_scene::{OrbitCamera, SplatRenderer, SplatScene};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewRequest {
    camera: OrbitCamera,
    size: [u32; 2],
}

struct Completion {
    renderer: SplatRenderer,
    image: image::RgbaImage,
    points: usize,
}

impl Completion {
    fn render(
        mut renderer: SplatRenderer,
        request: ViewRequest,
        points: usize,
    ) -> Result<Self, String> {
        let image = renderer
            .render(request.camera, request.size[0], request.size[1])
            .map_err(|error| error.to_string())?;
        Ok(Self {
            renderer,
            image,
            points,
        })
    }
}

enum RenderCompletion {
    Splat(Completion),
    Mesh {
        model: Arc<little_clip_core::model3d::Model3d>,
        path: PathBuf,
    },
}

struct RenderJob {
    request: ViewRequest,
    ticket: JobTicket,
    rx: mpsc::Receiver<Result<Result<RenderCompletion, String>, JobError>>,
}

impl Drop for RenderJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

/// One active read-only scene session; it never queues more than one render.
pub struct SplatViewerState {
    title: String,
    mesh: Option<crate::Model3dViewerState>,
    config: little_clip_core::config::Model3dConfig,
    desired: ViewRequest,
    completed: Option<ViewRequest>,
    renderer: Option<SplatRenderer>,
    job: Option<RenderJob>,
    texture: Option<egui::TextureHandle>,
    points: usize,
    error: Option<String>,
}

impl SplatViewerState {
    pub fn from_file(path: PathBuf) -> Self {
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "3D scene".into());
        let request = ViewRequest {
            camera: OrbitCamera::default(),
            size: [640, 360],
        };
        let (ticket, rx) = BackgroundJobs::global().submit(Duration::from_secs(120), move || {
            let scene = match SplatScene::load(&path) {
                Ok(scene) => scene,
                Err(little_clip_scene::SceneError::MeshGlb) => {
                    let model = little_clip_core::model3d::Model3d::load_file(&path)?;
                    return Ok(RenderCompletion::Mesh {
                        model: Arc::new(model),
                        path,
                    });
                }
                Err(error) => return Err(error.to_string()),
            };
            let points = scene.len();
            let renderer =
                SplatRenderer::new(Arc::new(scene)).map_err(|error| error.to_string())?;
            Completion::render(renderer, request, points).map(RenderCompletion::Splat)
        });
        Self {
            title,
            mesh: None,
            config: Default::default(),
            desired: request,
            completed: None,
            renderer: None,
            job: Some(RenderJob {
                request,
                ticket,
                rx,
            }),
            texture: None,
            points: 0,
            error: None,
        }
    }

    pub fn with_config(mut self, config: little_clip_core::config::Model3dConfig) -> Self {
        self.config = config;
        self
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else {
            return;
        };
        let result = match job.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.error = Some("Scene rendering worker disconnected".into());
                ctx.request_repaint();
                return;
            }
        };
        let request = job.request;
        self.job = None;
        ctx.request_repaint();
        match result {
            Ok(Ok(RenderCompletion::Mesh { model, path })) => {
                self.mesh = Some(
                    crate::Model3dViewerState::from_shared_model(model, Some(path))
                        .with_config(self.config.clone()),
                );
            }
            Ok(Ok(RenderCompletion::Splat(completion))) => {
                self.renderer = Some(completion.renderer);
                self.points = completion.points;
                if request == self.desired {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [
                            completion.image.width() as usize,
                            completion.image.height() as usize,
                        ],
                        completion.image.as_raw(),
                    );
                    if let Some(texture) = &mut self.texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ctx.load_texture(
                            "scene-viewport",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    self.completed = Some(request);
                }
            }
            Ok(Err(error)) => self.error = Some(error),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn queue_render(&mut self) {
        if self.job.is_some() || self.error.is_some() || self.completed == Some(self.desired) {
            return;
        }
        let Some(renderer) = self.renderer.take() else {
            return;
        };
        let request = self.desired;
        let points = self.points;
        let (ticket, rx) = BackgroundJobs::global().submit(Duration::from_secs(30), move || {
            Completion::render(renderer, request, points).map(RenderCompletion::Splat)
        });
        self.job = Some(RenderJob {
            request,
            ticket,
            rx,
        });
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette, close: &mut bool) {
        if let Some(mesh) = &mut self.mesh {
            mesh.render_ui(ui, palette, close);
            return;
        }
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            *close = true;
            return;
        }
        egui::Frame::none()
            .fill(crate::hex_to_c32(&palette.colors.canvas))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                let badge = if self.error.is_some() {
                    "Preview unavailable".into()
                } else if self.points == 0 {
                    "Loading 3D scene…".into()
                } else {
                    format!("{} Gaussians", self.points)
                };
                let mut shell = StudioShell::new(&self.title, Icon::Model3d, palette, close)
                    .with_badge(badge, None)
                    .with_footer_hints("Drag: Orbit • Right/Mid: Pan • Scroll: Zoom • Esc: Close")
                    .with_footer_readout(format!("Zoom {:.1}×", self.desired.camera.zoom));
                shell.render_header(ui, |ui| {
                    if ui
                        .add(crate::ui_ext::secondary_button_icon(
                            Some(Icon::ZoomFit),
                            "Reset view",
                            crate::hex_to_c32(&palette.colors.surface_panel),
                            palette,
                        ))
                        .clicked()
                    {
                        self.desired.camera = OrbitCamera::default();
                    }
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    shell.render_footer(ui, &None, 0.0);
                    self.render_canvas(ui);
                });
            });
        self.poll(ui.ctx());
        self.queue_render();
        if self.job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    fn render_canvas(&mut self, ui: &mut egui::Ui) {
        let size = ui.available_size().max(Vec2::splat(1.0));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.desired.camera.yaw -= delta.x * 0.01;
            self.desired.camera.pitch =
                (self.desired.camera.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            let delta =
                response.drag_delta() / rect.height().max(1.0) * 2.0 / self.desired.camera.zoom;
            self.desired.camera.pan[0] -= delta.x;
            self.desired.camera.pan[1] += delta.y;
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.raw_scroll_delta.y);
            self.desired.camera.zoom =
                (self.desired.camera.zoom * (scroll * 0.002).exp()).clamp(0.05, 50.0);
        }
        let scale = ui.ctx().pixels_per_point().max(1.0);
        let reduction = (rect.width() * scale / 1600.0)
            .max(rect.height() * scale / 1200.0)
            .max(1.0);
        self.desired.size = [
            (rect.width() * scale / reduction).round().max(1.0) as u32,
            (rect.height() * scale / reduction).round().max(1.0) as u32,
        ];
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::BLACK);
        if let Some(texture) = &self.texture {
            painter.image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        if let Some(error) = &self.error {
            ui.allocate_ui_at_rect(rect.shrink(16.0), |ui| {
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!("Unable to display 3D scene:\n{error}"),
                );
            });
        } else if self.texture.is_none() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Loading 3D scene…",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn finish_job(viewer: &mut SplatViewerState, ctx: &egui::Context) {
        let started = Instant::now();
        while viewer.job.is_some() {
            viewer.poll(ctx);
            assert!(started.elapsed() < Duration::from_secs(15));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(viewer.error.is_none(), "{:?}", viewer.error);
    }

    #[test]
    fn ordinary_glb_completes_into_the_existing_mesh_viewer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mesh.glb");
        std::fs::write(
            &path,
            include_bytes!("../../little-clip-scene/tests/fixtures/mesh.glb"),
        )
        .unwrap();
        let mut viewer = SplatViewerState::from_file(path.clone());
        finish_job(&mut viewer, &egui::Context::default());
        let mesh = viewer.mesh.as_ref().expect("Mesh fallback must be ready");
        assert_eq!(mesh.path.as_ref(), Some(&path));
        assert_eq!(mesh.mesh.as_ref().unwrap().triangles.len(), 4);
        assert!(viewer.renderer.is_none());
        assert!(viewer.texture.is_none());
    }

    #[test]
    #[ignore = "requires a native compute-capable GPU adapter"]
    fn navigation_rejects_stale_frames_retains_texture_and_stops_when_idle() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("navigation.splat");
        let mut bytes = Vec::new();
        for position in [[-0.5f32, 0.0, 0.0], [0.5, 0.3, 0.2]] {
            for value in position.into_iter().chain([0.1, 0.2, 0.15]) {
                bytes.extend(value.to_le_bytes());
            }
            bytes.extend([255, 30, 20, 240, 255, 128, 128, 128]);
        }
        std::fs::write(&path, bytes).unwrap();
        let ctx = egui::Context::default();
        let mut viewer = SplatViewerState::from_file(path);
        viewer.desired.camera.yaw += 0.4;
        viewer.desired.size = [97, 64];
        finish_job(&mut viewer, &ctx);
        assert!(
            viewer.texture.is_none(),
            "Initial stale frame must be discarded"
        );
        assert!(viewer.renderer.is_some());
        viewer.queue_render();
        finish_job(&mut viewer, &ctx);
        let accepted = viewer.completed.unwrap();
        let texture_id = viewer.texture.as_ref().unwrap().id();
        viewer.desired.camera.pan[0] += 0.2;
        viewer.queue_render();
        viewer.desired.camera.zoom *= 1.5;
        finish_job(&mut viewer, &ctx);
        assert_eq!(viewer.completed, Some(accepted));
        assert_eq!(viewer.texture.as_ref().unwrap().id(), texture_id);
        viewer.queue_render();
        finish_job(&mut viewer, &ctx);
        assert_eq!(viewer.completed, Some(viewer.desired));
        assert_eq!(viewer.texture.as_ref().unwrap().id(), texture_id);
        for _ in 0..100 {
            viewer.queue_render();
            assert!(
                viewer.job.is_none(),
                "Unchanged view must not dispatch GPU work"
            );
        }
    }
}
