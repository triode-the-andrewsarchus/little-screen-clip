//! Explicit editor handoffs render checked session copies off the UI thread.
use super::*;

pub(super) struct AudioEditorJob {
    ticket: JobTicket,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<String, String>, JobError>>,
}
impl Drop for AudioEditorJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

pub(super) fn launch_rendered_handoffs(
    mut rendered: Vec<(
        session_edit::RenderedSessionFile,
        crate::file_source::ReadCheck,
    )>,
    cancel: &AtomicBool,
    launch: impl FnOnce(&[PathBuf]) -> Result<String, String>,
) -> Result<String, String> {
    use std::os::fd::AsRawFd;
    if rendered.is_empty() {
        return Err("No audio editor inputs".into());
    }
    let paths = rendered
        .iter()
        .map(|(copy, _)| {
            let descriptor = PathBuf::from(format!(
                "/proc/self/fd/{}",
                copy.staging.as_file().as_raw_fd()
            ));
            crate::file_source::FileSource::capture(&descriptor)
                .map(|source| source.path().to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Validate the entire batch after preparing all copies. An earlier input
    // may have changed while later tracks were rendering.
    for (copy, check) in &rendered {
        check.validate(cancel)?;
        copy.observed.check()?;
    }
    if cancel.load(Ordering::Acquire) {
        return Err("Audio editor handoff cancelled".into());
    }
    let message = launch(&paths)?;
    // Editors may forward paths and exit before another process opens them.
    for (copy, _) in &mut rendered {
        copy.staging.disable_cleanup(true);
    }
    Ok(if paths.len() == 1 {
        format!("{message}; opened a session copy")
    } else {
        format!("{message}; opened {} session copies", paths.len())
    })
}

impl AudioPreviewState {
    pub(super) fn editor_render_filter(&self) -> (String, export::AudioRenderMode) {
        if let Some((graph, output)) = self.session_crossfade_filter_complex() {
            (
                format!("{graph};{output}anull[out]"),
                export::AudioRenderMode::SessionGraph,
            )
        } else {
            (
                self.session_edit_filter().unwrap_or_else(|| "anull".into()),
                export::AudioRenderMode::Session,
            )
        }
    }

    /// Renders the current session for an external editor without blocking the UI.
    pub fn launch_external_audio_editor(
        &mut self,
        ctx: &egui::Context,
        _close_requested: &mut bool,
    ) {
        if self.editor_job.is_some() {
            return;
        }
        let source = self.path.clone();
        let expected = self.file_source.clone();
        let check = self.read_check.clone();
        let editor = if self.audio_editor.is_empty() {
            "audacity".into()
        } else {
            self.audio_editor.clone()
        };
        let (filter, mode) = self.editor_render_filter();
        self.queue_editor_handoff(ctx, move |cancel| {
            let rendered = session_edit::render_session_file(
                &source,
                expected.as_ref(),
                &check,
                filter,
                mode,
                cancel,
            )?;
            launch_rendered_handoffs(vec![(rendered, check)], cancel, |paths| {
                crate::shell::EscapeHatchLauncher::spawn_with_fallback(&editor, &editor, &paths[0])
            })
        });
    }

    pub(super) fn queue_editor_handoff(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(&AtomicBool) -> Result<String, String> + Send + 'static,
    ) {
        if self.editor_job.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::MAX, move || work(&worker_cancel));
        self.editor_job = Some(AudioEditorJob {
            ticket,
            cancel,
            receiver,
        });
        self.status_toast = Some((
            "Preparing audio editor session…".into(),
            ctx.input(|input| input.time),
        ));
    }

    pub(super) fn poll_audio_editor(&mut self, ctx: &egui::Context) {
        let Some(job) = self.editor_job.take() else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(format!("Audio editor worker failed: {error}")),
            Err(TryRecvError::Disconnected) => Err("Audio editor worker disconnected".into()),
            Err(TryRecvError::Empty) => {
                self.editor_job = Some(job);
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
        };
        match result {
            Ok(message) => {
                self.stop();
                self.stop_group_playback();
                for stem in &mut self.stem_separation.stems {
                    stem.preview.stop();
                }
                self.status_toast = Some((message.into(), ctx.input(|input| input.time)));
            }
            Err(error) => {
                self.status_toast = Some((
                    format!("Could not open audio editor: {error}").into(),
                    ctx.input(|input| input.time),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_preview::tests::write_stereo_pcm_fixture;

    #[test]
    fn editor_submission_rejects_expired_observations_before_reading() {
        let mut state =
            AudioPreviewState::new(PathBuf::from("/missing-editor-input.wav"), "missing".into());
        state.cancel_session_analysis();
        state.read_check =
            crate::file_source::ReadCheck::new(|_| Err("source context expired".into()));
        state.launch_external_audio_editor(&egui::Context::default(), &mut false);
        crate::file_source::read_check_tests::assert_deferred_rejected(
            &state.editor_job.as_ref().unwrap().receiver,
        );
    }

    #[test]
    fn handoff_owns_its_copy_and_cleans_failed_or_cancelled_preparations() {
        for case in [
            "accepted",
            "launch failed",
            "cancelled",
            "source changed",
            "context expired",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let input = directory.path().join("input.wav");
            write_stereo_pcm_fixture(&input, &[8192; 800], &[-16384; 800], 8000);
            let cancel = AtomicBool::new(false);
            let rendered = session_edit::render_session_file(
                &input,
                None,
                &Default::default(),
                "volume=0.5".into(),
                export::AudioRenderMode::Session,
                &cancel,
            )
            .unwrap();
            let second = session_edit::render_session_file(
                &input,
                None,
                &Default::default(),
                "volume=0.5".into(),
                export::AudioRenderMode::Session,
                &cancel,
            )
            .unwrap();
            let second_copy =
                crate::file_source::FileSource::capture(second.staging.path()).unwrap();
            let copy = crate::file_source::FileSource::capture(rendered.staging.path()).unwrap();
            assert_ne!(copy.path(), input);
            if case == "source changed" {
                std::fs::write(&input, b"replaced after rendering").unwrap();
            }
            if case == "cancelled" {
                cancel.store(true, Ordering::Release);
            }
            let check = if case == "context expired" {
                crate::file_source::ReadCheck::new(|_| Err("expired observation".into()))
            } else {
                Default::default()
            };
            let result = launch_rendered_handoffs(
                vec![(rendered, check.clone()), (second, check)],
                &cancel,
                |paths| {
                    assert_eq!(paths.len(), 2);
                    assert_eq!(paths[1], second_copy.path());
                    let path = &paths[0];
                    assert!(
                        matches!(case, "accepted" | "launch failed"),
                        "rejected source reached launcher"
                    );
                    assert_eq!(path, copy.path());
                    let analysis =
                        little_clip_core::audio::compute_full_audio_analysis(path, 50).unwrap();
                    assert_eq!(analysis.total_frames, 800);
                    assert!((analysis.ch1_samples[400] - 0.125).abs() < 0.0001);
                    if case == "launch failed" {
                        Err("no editor installed".into())
                    } else {
                        Ok("Launched fake editor".into())
                    }
                },
            );
            if case == "accepted" {
                assert!(result.unwrap().contains("2 session copies"));
                // A launcher can hand the pathname to another process and exit.
                // Neither closing the viewer nor changing its original removes it.
                std::fs::remove_file(&input).unwrap();
                copy.check().unwrap();
                std::fs::remove_file(copy.path()).unwrap();
                second_copy.check().unwrap();
                std::fs::remove_file(second_copy.path()).unwrap();
            } else {
                assert!(result.is_err(), "{case}");
                assert!(
                    !second_copy.path().exists(),
                    "rejected batch leaked second copy: {case}"
                );
                assert!(
                    !copy.path().exists(),
                    "rejected handoff leaked its copy: {case}"
                );
            }
        }
    }
}
