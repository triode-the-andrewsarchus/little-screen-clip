//! Development gate for sharing eframe's current GL context with native vectors.
use eframe::{egui, glow::HasContext};
use femtovg::{Canvas, Color, FillRule, LayerEffects, Paint, Path, renderer::OpenGl};
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

thread_local! {
    // GPU objects stay on their creating UI thread, never inside a Send callback.
    static CANVAS: RefCell<Option<Canvas<OpenGl>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Metrics {
    frames: AtomicU64,
    gl_errors: AtomicU64,
    layer_failures: AtomicU64,
    released: AtomicBool,
}

struct Probe {
    metrics: Arc<Metrics>,
    error: Option<String>,
    zoom: f32,
    pan: egui::Vec2,
    clicks: u32,
}

impl Probe {
    fn new(cc: &eframe::CreationContext<'_>, metrics: Arc<Metrics>) -> Self {
        cc.egui_ctx.set_visuals(
            if std::env::var("LITTLE_CLIP_VECTOR_PROBE_APPEARANCE").as_deref() == Ok("light") {
                egui::Visuals::light()
            } else {
                let mut visuals = egui::Visuals::dark();
                visuals.override_text_color = Some(egui::Color32::WHITE);
                visuals
            },
        );
        cc.egui_ctx.set_pixels_per_point(1.4);
        let initialized = (|| {
            if cc.gl.is_none() {
                return Err("eframe did not supply an OpenGL context".into());
            }
            let loader = cc
                .get_proc_address
                .ok_or("eframe did not supply its GL function loader")?;
            // SAFETY: eframe calls its application creator with this window's
            // context current. Its loader supplies functions for that context.
            // The renderer is used only by this thread's paint callbacks and is
            // destroyed in on_exit before eframe destroys the owning context.
            let renderer = unsafe { OpenGl::new_from_function_cstr(loader) }
                .map_err(|error| format!("Cannot initialize native vectors: {error}"))?;
            let mut canvas = Canvas::new(renderer).map_err(|error| error.to_string())?;
            canvas.set_clip_mask_budget(16 * 1024 * 1024);
            CANVAS.with(|slot| *slot.borrow_mut() = Some(canvas));
            Ok::<(), String>(())
        })();
        Self {
            metrics,
            error: initialized.err(),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            clicks: 0,
        }
    }

    fn vector_ui(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());
        if response.dragged() {
            self.pan += ui.input(|input| input.pointer.delta());
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.raw_scroll_delta.y);
            self.zoom = (self.zoom * (scroll * 0.003).exp()).clamp(0.25, 32.0);
        }
        let metrics = self.metrics.clone();
        let zoom = self.zoom;
        let pan = self.pan;
        ui.painter_at(rect)
            .add(egui::Shape::Callback(egui::PaintCallback {
                // FemtoVG uses full-frame coordinates. The existing egui clip
                // rectangle still bounds this widget; surrounding UI is untouched.
                rect: ui.ctx().screen_rect(),
                callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                    CANVAS.with(|slot| {
                        if let Some(canvas) = slot.borrow_mut().as_mut() {
                            paint_vectors(canvas, &info, rect, zoom, pan, &metrics);
                        }
                    });
                    // SAFETY: the callback runs on eframe's current GL context.
                    // Offscreen clip/layer targets must not escape into egui or
                    // the next frame. egui restores its own program/VAO/blend
                    // state, but its callback contract requires us to restore
                    // the framebuffer explicitly.
                    unsafe {
                        painter.gl().bind_framebuffer(
                            eframe::glow::FRAMEBUFFER,
                            painter.intermediate_fbo(),
                        );
                        painter.gl().disable(eframe::glow::STENCIL_TEST);
                    }
                    // SAFETY: egui invokes this callback with its owning context
                    // current. This checks that both function tables used that same
                    // context without invalid GL operations; it creates no handles.
                    if unsafe { painter.gl().get_error() } != 0 {
                        metrics.gl_errors.fetch_add(1, Ordering::Relaxed);
                    }
                })),
            }));
    }
}

fn paint_vectors(
    canvas: &mut Canvas<OpenGl>,
    info: &egui::PaintCallbackInfo,
    rect: egui::Rect,
    zoom: f32,
    pan: egui::Vec2,
    metrics: &Metrics,
) {
    canvas.set_size(
        info.screen_size_px[0],
        info.screen_size_px[1],
        info.pixels_per_point,
    );
    canvas.reset();
    canvas.scale(info.pixels_per_point, info.pixels_per_point);
    let clip = rect.intersect(info.clip_rect);
    canvas.scissor(clip.left(), clip.top(), clip.width(), clip.height());
    // Capture has its own stencil attachment. This does not require changing
    // eframe's window framebuffer or clearing any of egui's surrounding UI.
    if !canvas.begin_layer(&LayerEffects::new()) {
        metrics.layer_failures.fetch_add(1, Ordering::Relaxed);
        canvas.end_layer();
        return;
    }
    let mut viewport = Path::new();
    viewport.rounded_rect(
        clip.left() + 6.0,
        clip.top() + 6.0,
        (clip.width() - 12.0).max(1.0),
        (clip.height() - 12.0).max(1.0),
        18.0,
    );
    canvas.clip_path(&viewport, FillRule::NonZero);
    let mut paper = Path::new();
    paper.rect(rect.left(), rect.top(), rect.width(), rect.height());
    canvas.fill_path(&paper, &Paint::color(Color::white()));
    canvas.translate(rect.center().x + pan.x, rect.center().y + pan.y);
    canvas.scale(zoom, zoom);
    canvas.translate(-10.0, -10.0);
    let mut shape = Path::new();
    shape.rounded_rect(0.0, 0.0, 320.0, 240.0, 30.0);
    shape.circle(160.0, 120.0, 42.0);
    canvas.fill_path(
        &shape,
        &Paint::color(Color::rgb(178, 242, 187)).with_fill_rule(FillRule::EvenOdd),
    );
    canvas.stroke_path(
        &shape,
        &Paint::color(Color::rgb(30, 30, 30)).with_line_width(2.0),
    );
    let mut curve = Path::new();
    curve.move_to(-110.0, 120.0);
    curve.bezier_to(-60.0, -90.0, 130.0, 360.0, 350.0, 80.0);
    canvas.stroke_path(
        &curve,
        &Paint::color(Color::rgb(25, 113, 194)).with_line_width(3.0),
    );
    canvas.end_layer();
    canvas.flush();
    metrics.frames.fetch_add(1, Ordering::Relaxed);
}

impl eframe::App for Probe {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        // Exercise repeated interleaving of both renderers, including while
        // the compositor asynchronously changes the native surface size.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::TopBottomPanel::top("probe_toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Vector context probe");
                if ui.button("Magnify").clicked() {
                    self.zoom = (self.zoom * 2.0).min(32.0);
                }
                if ui.button("Reset").clicked() {
                    self.zoom = 1.0;
                    self.pan = egui::Vec2::ZERO;
                }
                ui.label(format!("Zoom {:.0}%", self.zoom * 100.0));
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::RED, error);
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| self.vector_ui(ui));
        // This layer paints after the GPU callback. Its text and interaction
        // verify that egui's GL state is restored, rather than only testing an
        // isolated FemtoVG window.
        egui::Area::new(egui::Id::new("probe_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(30.0, 80.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    if ui
                        .button(format!("Overlay clicks {}", self.clicks))
                        .clicked()
                    {
                        self.clicks += 1;
                    }
                    ui.label(format!(
                        "Vector frames {}",
                        self.metrics.frames.load(Ordering::Relaxed)
                    ));
                });
            });
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        let released = CANVAS.with(|slot| slot.borrow_mut().take().is_some());
        self.metrics.released.store(released, Ordering::Relaxed);
        if let Some(gl) = gl {
            // SAFETY: eframe's exit hook retains the owning current GL context;
            // the renderer above is dropped before this final validation read.
            if unsafe { gl.get_error() } != 0 {
                self.metrics.gl_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let metrics = Arc::new(Metrics::default());
    let owned_metrics = metrics.clone();
    eframe::run_native(
        "Vector context probe",
        eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport: egui::ViewportBuilder::default()
                .with_app_id("little-clip-vector-probe")
                .with_inner_size([1100.0, 720.0]),
            ..Default::default()
        },
        Box::new(move |cc| Box::new(Probe::new(cc, owned_metrics))),
    )?;
    let frames = metrics.frames.load(Ordering::Relaxed);
    let errors = metrics.gl_errors.load(Ordering::Relaxed);
    let layers = metrics.layer_failures.load(Ordering::Relaxed);
    let released = metrics.released.load(Ordering::Relaxed);
    println!(
        "VECTOR_PROBE frames={frames} gl_errors={errors} layer_failures={layers} released={released}"
    );
    if frames == 0 || errors != 0 || layers != 0 || !released {
        return Err("native vector context validation failed".into());
    }
    Ok(())
}
