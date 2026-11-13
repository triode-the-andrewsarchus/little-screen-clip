//! Native vector callbacks sharing the desktop host's current OpenGL context.
//!
//! Hosts install one runtime per egui context during native creation, and
//! explicitly destroy it in their current-context exit hook. Widgets submit
//! thread-safe drawing input; the GPU renderer never leaves its UI thread.

pub use femtovg;

use egui_glow::glow::{self, HasContext};
use femtovg::{Canvas, LayerEffects, renderer::OpenGl};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_void},
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Weak,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
};

thread_local! {
    static RUNTIMES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const CONTEXT_MISMATCH: u8 = 1;
const OFFSCREEN_HOST: u8 = 2;
const LAYER_FAILURE: u8 = 3;
const GL_FAILURE: u8 = 4;
const RETIRED: u8 = 5;

fn key() -> egui::Id {
    egui::Id::new("clip_ui_native_vector_runtime")
}

#[derive(Clone)]
struct Handle {
    id: u64,
    failure: Arc<AtomicU8>,
}
impl Handle {
    fn fail(&self, failure: u8) {
        let _ = self
            .failure
            .compare_exchange(0, failure, Ordering::Relaxed, Ordering::Relaxed);
    }

    fn error(&self) -> Option<&'static str> {
        match self.failure.load(Ordering::Relaxed) {
            0 => None,
            CONTEXT_MISMATCH => Some("The vector renderer belongs to a different OpenGL context."),
            OFFSCREEN_HOST => {
                Some("This host's offscreen framebuffer is not supported by native vectors.")
            }
            LAYER_FAILURE => Some("The native vector layer could not be allocated."),
            GL_FAILURE => Some("OpenGL reported an error while painting native vectors."),
            _ => Some("The native vector renderer has been retired."),
        }
    }
}

struct Entry {
    canvas: Option<Canvas<OpenGl>>,
    context: Weak<glow::Context>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(canvas) = self.canvas.take() {
            // A panic or missed host exit hook gives no current-context
            // guarantee. Do not issue GL deletion calls on an unrelated or
            // destroyed context. The host context teardown reclaims its GPU
            // objects; leaking this Rust owner is the last-resort failure path.
            std::mem::forget(canvas);
            tracing::error!("Native vectors dropped without current-context cleanup");
        }
    }
}

/// Owns one host's UI-thread vector renderer.
///
/// Drawing callbacks check the actual egui painter's context identity. Multiple
/// hosts on the same thread retain independent renderers and teardown owners.
/// This owner is deliberately neither `Send` nor `Sync`.
pub struct Runtime {
    handle: Handle,
    ctx: egui::Context,
    _thread: PhantomData<Rc<()>>,
}
impl Runtime {
    /// Installs native vectors for an egui host.
    ///
    /// # Errors
    ///
    /// Returns an error if this egui context already has an owner or OpenGL
    /// renderer initialization fails.
    ///
    /// # Safety
    ///
    /// `gl` must be the calling thread's current native OpenGL context, and
    /// `loader` must resolve functions for that same context. The host must call
    /// [`Self::destroy`] while that context is current, before destroying it.
    pub unsafe fn install(
        ctx: &egui::Context,
        gl: &Arc<glow::Context>,
        loader: &dyn Fn(&CStr) -> *const c_void,
    ) -> Result<Self, String> {
        if ctx.data(|data| data.get_temp::<Handle>(key()).is_some()) {
            return Err("This egui context already owns a native vector renderer.".into());
        }
        // SAFETY: the host supplies its current-context loader under the
        // install contract. All resulting GL resources stay on this thread.
        let renderer = unsafe { OpenGl::new_from_function_cstr(loader) }
            .map_err(|error| format!("Cannot initialize native vectors: {error}"))?;
        let mut canvas = Canvas::new(renderer).map_err(|error| error.to_string())?;
        canvas.set_clip_mask_budget(16 * 1024 * 1024);
        let handle = Handle {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            failure: Arc::new(AtomicU8::new(0)),
        };
        RUNTIMES.with(|slots| {
            slots.borrow_mut().insert(
                handle.id,
                Entry {
                    canvas: Some(canvas),
                    context: Arc::downgrade(gl),
                },
            );
        });
        ctx.data_mut(|data| data.insert_temp(key(), handle.clone()));
        Ok(Self {
            handle,
            ctx: ctx.clone(),
            _thread: PhantomData,
        })
    }

    /// Returns the first rendering failure, including failures from callbacks.
    pub fn error(&self) -> Option<&'static str> {
        self.handle.error()
    }

    /// Releases the renderer before the host destroys its native context.
    ///
    /// # Errors
    ///
    /// Returns an error without deleting GPU objects if `gl` is a different
    /// context. Calling this method again after successful cleanup is harmless.
    ///
    /// # Safety
    ///
    /// `gl` must be current on the installing thread throughout this call.
    pub unsafe fn destroy(&mut self, gl: &glow::Context) -> Result<(), String> {
        RUNTIMES.with(|slots| -> Result<(), String> {
            let mut slots = slots.borrow_mut();
            if let Some(entry) = slots.get(&self.handle.id) {
                let matches = entry
                    .context
                    .upgrade()
                    .is_some_and(|owner| std::ptr::eq(gl, owner.as_ref()));
                if !matches {
                    return Err(
                        "Cannot destroy native vectors on a different OpenGL context.".into(),
                    );
                }
            }
            if let Some(mut entry) = slots.remove(&self.handle.id) {
                // Taking the canvas bypasses Entry's no-current-context fallback.
                // SAFETY: the caller retains the owning current context, whose
                // identity was checked above, until Canvas's destructor returns.
                drop(entry.canvas.take());
            }
            Ok(())
        })?;
        self.detach();
        Ok(())
    }

    fn detach(&self) {
        self.ctx.data_mut(|data| {
            if data
                .get_temp::<Handle>(key())
                .is_some_and(|handle| handle.id == self.handle.id)
            {
                data.remove::<Handle>(key());
            }
        });
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.detach();
        self.handle.fail(RETIRED);
        RUNTIMES.with(|slots| {
            slots.borrow_mut().remove(&self.handle.id);
        });
    }
}

/// Queues vector artwork clipped to a widget's logical screen rectangle.
///
/// `draw` runs on the host's current-context UI thread. Its canvas starts in
/// egui logical screen coordinates, with DPI scaling and widget clipping
/// installed. Drawing must balance its own saved states and nested layers.
/// Source parsing and expensive preparation belong on bounded workers.
///
/// # Errors
///
/// Returns an error when the host has no renderer or a prior callback failed.
/// A new callback failure is available on the following UI frame.
pub fn paint(
    ui: &egui::Ui,
    rect: egui::Rect,
    draw: impl Fn(&mut Canvas<OpenGl>) + Send + Sync + 'static,
) -> Result<(), String> {
    let handle = ui
        .ctx()
        .data(|data| data.get_temp::<Handle>(key()))
        .ok_or("Native vectors are unavailable in this desktop host.")?;
    if let Some(error) = handle.error() {
        return Err(error.into());
    }
    let ctx = ui.ctx().clone();
    ui.painter_at(rect)
        .add(egui::Shape::Callback(egui::PaintCallback {
            rect: ui.ctx().screen_rect(),
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                if handle.error().is_some() {
                    return;
                }
                RUNTIMES.with(|slots| {
                    let mut slots = slots.borrow_mut();
                    let Some(entry) = slots.get_mut(&handle.id) else {
                        handle.fail(RETIRED);
                        return;
                    };
                    if !entry
                        .context
                        .upgrade()
                        .is_some_and(|owner| Arc::ptr_eq(&owner, painter.gl()))
                    {
                        handle.fail(CONTEXT_MISMATCH);
                        return;
                    }
                    // FemtoVG's external-framebuffer adapter owns its target. Never
                    // hand it a borrowed egui FBO; the current native glow host uses
                    // the window target. Refuse other targets explicitly for now.
                    if painter.intermediate_fbo().is_some() {
                        handle.fail(OFFSCREEN_HOST);
                        return;
                    }
                    if let Some(canvas) = entry.canvas.as_mut() {
                        paint_frame(canvas, &info, rect, &draw, &handle);
                    }
                    // SAFETY: this callback is running on the owning current
                    // context (verified above). egui restores its program, VAO,
                    // blend and viewport; we must restore the framebuffer and
                    // disable stencil testing left by vector fills/clips.
                    unsafe {
                        painter
                            .gl()
                            .bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
                        painter.gl().disable(glow::STENCIL_TEST);
                        if painter.gl().get_error() != glow::NO_ERROR {
                            handle.fail(GL_FAILURE);
                        }
                    }
                });
                if handle.error().is_some() {
                    ctx.request_repaint();
                }
            })),
        }));
    Ok(())
}

fn paint_frame(
    canvas: &mut Canvas<OpenGl>,
    info: &egui::PaintCallbackInfo,
    rect: egui::Rect,
    draw: &impl Fn(&mut Canvas<OpenGl>),
    handle: &Handle,
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
    // The private layer supplies stencil without changing host surface options.
    let captured = canvas.begin_layer(&LayerEffects::new());
    if captured {
        draw(canvas);
    } else {
        handle.fail(LAYER_FAILURE);
    }
    canvas.end_layer();
    canvas.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lifecycle tests need only the registered delivery identity. Real GPU
    // creation/painting/deletion is exercised by vector_context_acceptance.py.
    fn owner(ctx: &egui::Context) -> Runtime {
        let handle = Handle {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            failure: Arc::new(AtomicU8::new(0)),
        };
        ctx.data_mut(|data| data.insert_temp(key(), handle.clone()));
        Runtime {
            handle,
            ctx: ctx.clone(),
            _thread: PhantomData,
        }
    }

    #[test]
    fn retiring_old_owner_cannot_detach_or_poison_its_replacement() {
        let ctx = egui::Context::default();
        let old = owner(&ctx);
        old.detach();
        let replacement = owner(&ctx);
        drop(old);
        let active = ctx.data(|data| data.get_temp::<Handle>(key())).unwrap();
        assert_eq!(active.id, replacement.handle.id);
        assert!(active.error().is_none());
        let other_ctx = egui::Context::default();
        let other = owner(&other_ctx);
        replacement.handle.fail(GL_FAILURE);
        assert!(other.error().is_none());
        drop(replacement);
        assert!(other_ctx.data(|data| data.get_temp::<Handle>(key()).is_some()));
    }

    #[test]
    fn failed_context_queues_no_native_callback_and_preserves_first_failure() {
        let ctx = egui::Context::default();
        let runtime = owner(&ctx);
        runtime.handle.fail(CONTEXT_MISMATCH);
        runtime.handle.fail(GL_FAILURE);
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let error = paint(ui, ui.available_rect_before_wrap(), |_| {
                    panic!("a rejected callback must never draw");
                })
                .unwrap_err();
                assert!(error.contains("different OpenGL context"));
            });
        });
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| matches!(shape.shape, egui::Shape::Callback(_)))
        );
        drop(runtime);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(
                    paint(ui, ui.available_rect_before_wrap(), |_| {})
                        .unwrap_err()
                        .contains("unavailable")
                );
            });
        });
    }
}
