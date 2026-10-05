//! Collection navigation retains Image Studio's source guards and unsaved-edit checks.
use super::*;
use std::time::Instant;

pub(super) struct Collection {
    paths: Vec<PathBuf>,
    index: usize,
    slideshow: bool,
    interval: f32,
    ready_since: Option<Instant>,
}

impl Viewers {
    pub(super) fn pause_slideshow(&mut self) {
        if let Some(collection) = &mut self.gallery {
            collection.slideshow = false;
        }
    }
    pub(in crate::file_manager) fn open_collection(&mut self, path: PathBuf, paths: Vec<PathBuf>) {
        if self.active() {
            return;
        }
        self.gallery = paths
            .iter()
            .position(|p| p == &path)
            .map(|index| Collection {
                paths,
                index,
                slideshow: false,
                interval: 4.0,
                ready_since: None,
            });
        self.open(path);
    }

    pub(super) fn gallery_controls(&mut self, ui: &mut Ui) {
        let Some(collection) = &mut self.gallery else {
            return;
        };
        let ready = self.loading.is_none() && matches!(self.session, Some(ViewerSession::Image(_)));
        if !ready {
            collection.ready_since = None;
        }
        let mut step = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    ready && collection.index > 0,
                    egui::Button::new("Previous photo"),
                )
                .clicked()
            {
                step = Some(false);
            }
            ui.label(format!(
                "{} / {}",
                collection.index + 1,
                collection.paths.len()
            ));
            if ui
                .add_enabled(
                    ready && collection.index + 1 < collection.paths.len(),
                    egui::Button::new("Next photo"),
                )
                .clicked()
            {
                step = Some(true);
            }
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(if collection.slideshow {
                        "Pause slideshow"
                    } else {
                        "Start slideshow"
                    }),
                )
                .clicked()
            {
                collection.slideshow = !collection.slideshow;
                collection.ready_since = None;
            }
            if ui
                .add(egui::Slider::new(&mut collection.interval, 1.0..=30.0).suffix(" s"))
                .changed()
            {
                collection.ready_since = None;
            }
        });
        if collection.slideshow && ready {
            let since = collection.ready_since.get_or_insert_with(Instant::now);
            if since.elapsed().as_secs_f32() >= collection.interval {
                step = Some(true);
            }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(forward) = step {
            self.step_photo(forward);
        }
    }

    fn step_photo(&mut self, forward: bool) {
        let Some(mut collection) = self.gallery.take() else {
            return;
        };
        let target = if forward {
            (collection.index + 1) % collection.paths.len()
        } else {
            collection.index.saturating_sub(1)
        };
        if !self.request_close() {
            self.confirm_close = false;
            self.error =
                Some("Save or finish your edits before changing photos. Slideshow paused.".into());
            collection.slideshow = false;
        } else {
            collection.index = target;
            collection.ready_since = None;
            self.open(collection.paths[target].clone());
        }
        self.gallery = Some(collection);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_collection() -> (tempfile::TempDir, Viewers) {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a.png", "b.png"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 80, 120, 255]))
                    .save(&path)
                    .unwrap();
                path
            })
            .collect();
        let config = AppConfig::default();
        let loaded = load(&paths[0], &config).unwrap();
        let viewer = session(loaded, paths[0].clone(), &config);
        (
            dir,
            Viewers {
                path: Some(paths[0].clone()),
                session: Some(viewer),
                gallery: Some(Collection {
                    paths,
                    index: 0,
                    slideshow: true,
                    interval: 1.0,
                    ready_since: None,
                }),
                ..Viewers::default()
            },
        )
    }

    #[test]
    fn next_photo_opens_in_order_and_slideshow_wraps() {
        let (_dir, mut viewers) = ready_collection();
        viewers.step_photo(true);
        assert_eq!(viewers.gallery.as_ref().unwrap().index, 1);
        assert!(viewers.gallery.as_ref().unwrap().slideshow);
        let ctx = Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while viewers.loading.is_some() {
            viewers.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            viewers.path.as_ref(),
            Some(&viewers.gallery.as_ref().unwrap().paths[1])
        );
        viewers.step_photo(true);
        assert_eq!(viewers.gallery.as_ref().unwrap().index, 0);
    }

    #[test]
    fn unsaved_edits_pause_slideshow_without_discarding_or_changing_photo() {
        let (_dir, mut viewers) = ready_collection();
        let original = viewers.path.clone();
        if let Some(ViewerSession::Image(state)) = &mut viewers.session {
            state.pixel_revision += 1;
        }
        viewers.step_photo(true);
        assert_eq!(viewers.path, original);
        assert_eq!(viewers.gallery.as_ref().unwrap().index, 0);
        assert!(!viewers.gallery.as_ref().unwrap().slideshow);
        assert!(viewers.loading.is_none());
        assert!(matches!(viewers.session, Some(ViewerSession::Image(_))));
        assert!(!viewers.confirm_close);
        assert!(viewers.error.as_ref().unwrap().contains("Save"));
    }
}
