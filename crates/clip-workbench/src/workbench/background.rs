use super::*;

fn daemon_scene_previews() -> Vec<little_clip_core::ipc::ScenePreview> {
    match crate::send_ipc_request_with_timeout(
        &IpcRequest::GetScenePreviews,
        Some(Duration::from_millis(100)),
    ) {
        Ok(IpcResponse::ScenePreviews(previews)) => previews,
        _ => Vec::new(),
    }
}

/// Shared snapshot identity for background results. Keeping the Arc also makes
/// local swaps copy-on-write, so in-flight readers retain their original state.
#[derive(Clone)]
pub(crate) struct ClipboardSelection {
    state: Arc<SlotManager>,
    selected: Vec<SlotId>,
}

impl ClipboardSelection {
    fn new(app: &WorkbenchApp) -> Self {
        Self {
            state: Arc::clone(&app.state),
            selected: app.selected_slots.clone(),
        }
    }

    pub(super) fn matches(&self, app: &WorkbenchApp) -> bool {
        Arc::ptr_eq(&self.state, &app.state) && self.selected == app.selected_slots
    }
}

pub(crate) struct PaletteOpsJob {
    key: ClipboardSelection,
    pub(super) ticket: JobTicket,
    receiver: Receiver<Result<Vec<OpDescriptor>, JobError>>,
}

type MirrorResult = (ScreenRenderer, Result<egui::ColorImage, String>, bool);

pub(crate) struct MirrorJob {
    key: ClipboardSelection,
    appearance: WorkbenchAppearance,
    pub(super) ticket: JobTicket,
    receiver: Receiver<Result<MirrorResult, JobError>>,
}

impl WorkbenchApp {
    pub(crate) fn poll_palette_ops(&mut self, ctx: &egui::Context) {
        if !self.state_loaded {
            return;
        }
        if self
            .palette_ops_job
            .as_ref()
            .is_some_and(|job| !job.key.matches(self))
            && let Some(job) = self.palette_ops_job.take()
        {
            job.ticket.cancel();
        }
        if let Some(job) = &self.palette_ops_job {
            match job.receiver.try_recv() {
                Ok(result) => {
                    let job = self.palette_ops_job.take().unwrap();
                    match result {
                        Ok(ops) => self.palette_ops = Some((job.key.clone(), ops)),
                        Err(JobError::QueueFull) => {}
                        Err(error) => {
                            self.set_status_message(format!("Could not prepare actions: {error}"));
                            self.palette_ops = Some((job.key.clone(), Vec::new()));
                        }
                    }
                    ctx.request_repaint();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.palette_ops_job = None;
                }
            }
        }
        if self
            .palette_ops
            .as_ref()
            .is_some_and(|(key, _)| key.matches(self))
        {
            return;
        }
        self.palette_ops = None;
        if self.palette_ops_job.is_none() {
            let key = ClipboardSelection::new(self);
            let state = Arc::clone(&key.state);
            let selected = key.selected.clone();
            let repaint = ctx.clone();
            let (ticket, receiver) =
                BackgroundJobs::global().submit(Duration::from_secs(30), move || {
                    let items = selected
                        .iter()
                        .filter_map(|slot| state.get(*slot))
                        .collect::<Vec<_>>();
                    let result = get_applicable_ops(&items);
                    repaint.request_repaint();
                    result
                });
            self.palette_ops_job = Some(PaletteOpsJob {
                key,
                ticket,
                receiver,
            });
        }
        ctx.request_repaint_after(Duration::from_millis(40));
    }

    pub(crate) fn update_mirror_texture(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.mirror_job {
            match job.receiver.try_recv() {
                Ok(result) => {
                    let job = self.mirror_job.take().unwrap();
                    match result {
                        Ok((renderer, image, animated)) => {
                            self.renderer = Some(renderer);
                            if job.key.matches(self)
                                && job.appearance == self.config.workbench.appearance
                            {
                                self.mirror_animated = animated;
                                match image {
                                    Ok(image) => {
                                        self.mirror_texture = Some(ctx.load_texture(
                                            "ambient-mirror",
                                            image,
                                            egui::TextureOptions::LINEAR,
                                        ));
                                        self.mirror_error = None;
                                    }
                                    Err(error) => self.mirror_error = Some(error),
                                }
                            } else {
                                self.dirty_mirror = true;
                            }
                        }
                        Err(JobError::QueueFull) => self.dirty_mirror = true,
                        Err(error) => self.mirror_error = Some(error.to_string()),
                    }
                    ctx.request_repaint();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.mirror_job = None;
                    self.mirror_error = Some("Preview worker stopped".to_owned());
                }
            }
        }
        if self.state_loaded && self.dirty_mirror && self.mirror_job.is_none() {
            let key = ClipboardSelection::new(self);
            let state = Arc::clone(&key.state);
            let selected = key.selected.clone();
            let config = self.config.clone();
            let appearance = config.workbench.appearance;
            let renderer = self.renderer.take();
            let repaint = ctx.clone();
            let (ticket, receiver) =
                BackgroundJobs::global().submit(Duration::from_secs(30), move || {
                    let mut renderer = renderer.unwrap_or_else(|| {
                        ScreenRenderer::new(config.clone())
                            .unwrap_or_else(|_| ScreenRenderer::fallback(config.clone()))
                    });
                    renderer.set_config(config);
                    renderer.set_scene_preview_source(daemon_scene_previews);
                    let animated = state.has_active_animation();
                    let mut render_state = (*state).clone();
                    render_state.selected = selected;
                    let image = renderer
                        .render_pixmap(&render_state)
                        .map(|pixmap| {
                            egui::ColorImage::from_rgba_premultiplied(
                                [SCREEN_WIDTH as usize, SCREEN_HEIGHT as usize],
                                pixmap.data(),
                            )
                        })
                        .map_err(|error| error.to_string());
                    repaint.request_repaint();
                    (renderer, image, animated)
                });
            self.mirror_job = Some(MirrorJob {
                key,
                appearance,
                ticket,
                receiver,
            });
            self.dirty_mirror = false;
        }
        if self.mirror_job.is_some() || !self.state_loaded {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }
}

impl Drop for PaletteOpsJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

impl Drop for MirrorJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench_layout_tests::{
        queued_job_behind_blocker, ready_palette_actions, test_workbench_app, wait_for_mirror,
    };

    #[test]
    fn actions_match_exact_classification_and_search_reuses_the_snapshot() {
        let mut app = test_workbench_app(None, VecDeque::new());
        Arc::make_mut(&mut app.state).push_left(ClipItem::from_text("x".repeat(256 * 1024)));
        let expected = get_applicable_ops(&app.get_selected_items());
        ready_palette_actions(&mut app);
        let (key, actual) = app.palette_ops.as_ref().unwrap();
        assert_eq!(
            actual.iter().map(|op| op.name).collect::<Vec<_>>(),
            expected.iter().map(|op| op.name).collect::<Vec<_>>()
        );
        let snapshot = Arc::clone(&key.state);
        app.search_query = "uppercase".to_owned();
        ready_palette_actions(&mut app);
        assert!(Arc::ptr_eq(
            &snapshot,
            &app.palette_ops.as_ref().unwrap().0.state
        ));
        assert!(
            matches!(&app.filtered_actions()[0], PaletteAction::Op(op) if op.name == "uppercase")
        );
    }

    #[test]
    fn selection_change_discards_a_queued_action_result() {
        let mut app = test_workbench_app(None, VecDeque::new());
        Arc::make_mut(&mut app.state).push_left(ClipItem::from_text("text"));
        let key = ClipboardSelection::new(&app);
        let (_jobs, release, ticket, receiver) = queued_job_behind_blocker(Vec::new);
        app.palette_ops_job = Some(PaletteOpsJob {
            key,
            ticket,
            receiver,
        });
        app.selected_slots = vec![SlotId::RightHand];
        app.poll_palette_ops(&egui::Context::default());
        release.send(()).unwrap();
        ready_palette_actions(&mut app);
        assert!(app.palette_ops.as_ref().unwrap().1.is_empty());
        assert_eq!(
            app.palette_ops.as_ref().unwrap().0.selected,
            [SlotId::RightHand]
        );
    }

    #[test]
    fn mirror_completion_does_not_publish_after_an_appearance_change() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let config = app.config.clone();
        let key = ClipboardSelection::new(&app);
        let (_jobs, release, ticket, receiver) = queued_job_behind_blocker(move || {
            (
                ScreenRenderer::fallback(config),
                Ok(egui::ColorImage::new([1, 1], egui::Color32::RED)),
                false,
            )
        });
        let appearance = app.config.workbench.appearance;
        app.mirror_job = Some(MirrorJob {
            key,
            appearance,
            ticket,
            receiver,
        });
        app.config.workbench.appearance = appearance.toggled();
        release.send(()).unwrap();
        wait_for_mirror(&mut app);
        assert_ne!(app.mirror_texture.as_ref().unwrap().size(), [1, 1]);
        assert_eq!(
            app.renderer.as_ref().unwrap().config.workbench.appearance,
            appearance.toggled()
        );
    }

    #[test]
    fn same_item_preview_hydration_invalidates_the_background_snapshot() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let mut item = ClipItem::from_text("image");
        item.payload = ClipPayload::Image {
            bytes: vec![],
            path: None,
            width: 1,
            height: 1,
            mime: "image/png".into(),
            thumbnail: None,
        };
        Arc::make_mut(&mut app.state).push_left(item);
        let key = ClipboardSelection::new(&app);
        let original_id = app.state.left_hand().unwrap().id;
        let Some(ClipItem {
            payload: ClipPayload::Image { thumbnail, .. },
            ..
        }) = Arc::make_mut(&mut app.state)
            .get_mut(SlotId::LeftHand)
            .unwrap()
        else {
            panic!("image fixture")
        };
        *thumbnail = Some(little_clip_core::ImageThumbnail {
            width: 1,
            height: 1,
            premultiplied_rgba: vec![0, 0, 0, 255],
        });
        assert_eq!(app.state.left_hand().unwrap().id, original_id);
        assert!(!key.matches(&app));
    }

    #[test]
    fn saturated_jobs_retry_without_permanently_hiding_actions_or_mirror() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let jobs = BackgroundJobs::new(1, 1);
        let (ticket, _) = jobs.submit(Duration::MAX, || ());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err(JobError::QueueFull)).unwrap();
        app.palette_ops_job = Some(PaletteOpsJob {
            key: ClipboardSelection::new(&app),
            ticket: ticket.clone(),
            receiver: rx,
        });
        ready_palette_actions(&mut app);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err(JobError::QueueFull)).unwrap();
        app.mirror_job = Some(MirrorJob {
            key: ClipboardSelection::new(&app),
            appearance: app.config.workbench.appearance,
            ticket,
            receiver: rx,
        });
        let ctx = egui::Context::default();
        app.update_mirror_texture(&ctx);
        assert!(app.mirror_job.is_some());
        wait_for_mirror(&mut app);
    }
}
