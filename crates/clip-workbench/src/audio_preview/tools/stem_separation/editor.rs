//! Batch editor handoffs use independent checked session copies.
use super::*;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;

struct StemEditorInput {
    name: String,
    source: AudioEditSnapshot,
    filter: (String, export::AudioRenderMode),
}

fn render_stem_handoff(
    inputs: Vec<StemEditorInput>,
    cancel: &AtomicBool,
    launch: impl FnOnce(&[PathBuf], &[String]) -> Result<String, String>,
) -> Result<String, String> {
    let names = inputs
        .iter()
        .map(|input| input.name.clone())
        .collect::<Vec<_>>();
    let copies = inputs
        .into_iter()
        .map(|input| {
            let copy = session_edit::render_session_file(
                &input.source.path,
                input.source.file_source.as_ref(),
                &input.source.read_check,
                input.filter.0,
                input.filter.1,
                cancel,
            )?;
            Ok((copy, input.source.read_check))
        })
        .collect::<Result<Vec<_>, String>>()?;
    crate::audio_preview::editor::launch_rendered_handoffs(copies, cancel, |paths| {
        launch(paths, &names)
    })
}

fn spawn_stem_editor(editor: &str, paths: &[PathBuf], names: &[String]) -> Result<String, String> {
    let parent = little_clip_core::kernel_interface::pin_content_directory(
        paths
            .first()
            .and_then(|path| path.parent())
            .ok_or("No stem editor inputs")?,
    )
    .map_err(|error| error.to_string())?;
    let mut workspace = tempfile::Builder::new()
        .prefix("stem-editor-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(format!("/proc/self/fd/{}", parent.as_raw_fd()))
        .map_err(|error| error.to_string())?;
    let directory = little_clip_core::kernel_interface::pin_content_directory(workspace.path())
        .map_err(|error| error.to_string())?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let physical = std::fs::read_link(&anchor).map_err(|error| error.to_string())?;
    let aliases = paths
        .iter()
        .zip(names)
        .map(|(path, name)| {
            let filename = format!("{name}.wav");
            if Path::new(&filename).file_name() != Some(std::ffi::OsStr::new(&filename)) {
                return Err("Invalid stem name".to_owned());
            }
            // Private aliases keep vocals/drums/bass/other track names, while the
            // external editor sees only independent rendered copies.
            std::os::unix::fs::symlink(path, anchor.join(&filename))
                .map_err(|error| error.to_string())?;
            Ok(physical.join(filename))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut command = std::process::Command::new(editor);
    if editor_is_audacity(editor) {
        prepare_isolated_audacity_command(&mut command, &physical, &aliases)?;
    } else {
        command.args(&aliases);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not launch {editor}: {error}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    // The receiver may forward the LOF/aliases to an existing process.
    workspace.disable_cleanup(true);
    Ok(format!("Opened stems in {editor}"))
}

impl AudioPreviewState {
    pub(crate) fn launch_stems_in_external_editor(
        &mut self,
        ctx: &egui::Context,
        _close_requested: &mut bool,
    ) {
        let editor = if self.audio_editor.is_empty() {
            "audacity".into()
        } else {
            self.audio_editor.clone()
        };
        let inputs = self
            .stem_separation
            .stems
            .iter()
            .map(|stem| StemEditorInput {
                name: stem.name.clone(),
                source: stem.preview.edit_snapshot(),
                filter: stem.preview.editor_render_filter(),
            })
            .collect();
        self.queue_editor_handoff(ctx, move |cancel| {
            render_stem_handoff(inputs, cancel, |paths, names| {
                spawn_stem_editor(&editor, paths, names)
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_preview::tests::{empty_audio_analysis, write_stereo_pcm_fixture};

    #[test]
    fn stem_editor_renders_all_edits_and_retains_independent_copies() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("original.wav");
        write_stereo_pcm_fixture(&path, &[8192; 800], &[-16384; 800], 8000);
        let mut state = AudioPreviewState::from_completed_source(
            path.clone(),
            "stem".into(),
            None,
            Arc::new(empty_audio_analysis()),
        );
        state.file_source = Some(crate::file_source::FileSource::capture(&path).unwrap());
        state.track_gain_db = -6.0206;
        let inputs = ["vocals", "drums"]
            .into_iter()
            .map(|name| StemEditorInput {
                name: name.into(),
                source: state.edit_snapshot(),
                filter: state.editor_render_filter(),
            })
            .collect();
        let mut retained = Vec::new();
        let result = render_stem_handoff(inputs, &AtomicBool::new(false), |paths, names| {
            assert_eq!(names, &["vocals", "drums"]);
            assert_eq!(paths.len(), 2);
            for path in paths {
                assert_ne!(*path, state.path);
                let audio = little_clip_core::audio::compute_full_audio_analysis(path, 50).unwrap();
                assert_eq!(audio.total_frames, 800);
                assert!((audio.ch1_samples[400] - 0.125).abs() < 0.0001);
            }
            retained = paths.to_vec();
            Ok("fake editor".into())
        })
        .unwrap();
        assert!(result.contains("2 session copies"));
        drop(state);
        drop(directory);
        for path in retained {
            assert!(path.is_file());
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn failed_editor_launch_cleans_private_aliases_and_rejects_unsafe_names() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("copy.wav");
        std::fs::write(&source, b"retained copy").unwrap();
        for name in ["vocals", "../outside"] {
            let result =
                spawn_stem_editor("/missing-stem-editor", &[source.clone()], &[name.into()]);
            assert!(result.is_err());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
            assert_eq!(std::fs::read(&source).unwrap(), b"retained copy");
        }
    }

    #[test]
    fn batch_handoff_rechecks_early_stems_after_later_rendering() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.wav");
        write_stereo_pcm_fixture(&source, &[8192; 800], &[-16384; 800], 8000);
        let mut state = AudioPreviewState::from_completed_source(
            source,
            "stem".into(),
            None,
            Arc::new(empty_audio_analysis()),
        );
        let expired = Arc::new(AtomicBool::new(false));
        let first = expired.clone();
        state.read_check = crate::file_source::ReadCheck::new(move |_| {
            if first.load(Ordering::Acquire) {
                Err("earlier stem expired".into())
            } else {
                Ok(())
            }
        });
        let earlier = StemEditorInput {
            name: "vocals".into(),
            source: state.edit_snapshot(),
            filter: state.editor_render_filter(),
        };
        state.read_check = crate::file_source::ReadCheck::new(move |_| {
            expired.store(true, Ordering::Release);
            Ok(())
        });
        let later = StemEditorInput {
            name: "drums".into(),
            source: state.edit_snapshot(),
            filter: state.editor_render_filter(),
        };
        let result = render_stem_handoff(vec![earlier, later], &AtomicBool::new(false), |_, _| {
            panic!("obsolete batch reached editor")
        });
        assert_eq!(result.err().as_deref(), Some("earlier stem expired"));
    }

    #[test]
    fn stem_editor_rejects_expired_input_before_launch() {
        let mut state = AudioPreviewState::from_completed_source(
            PathBuf::from("/missing-stem.wav"),
            "stem".into(),
            None,
            Arc::new(empty_audio_analysis()),
        );
        state.read_check =
            crate::file_source::ReadCheck::new(|_| Err("stem observation expired".into()));
        let result = render_stem_handoff(
            vec![StemEditorInput {
                name: "vocals".into(),
                source: state.edit_snapshot(),
                filter: state.editor_render_filter(),
            }],
            &AtomicBool::new(false),
            |_, _| panic!("expired stem reached editor"),
        );
        assert_eq!(result.err().as_deref(), Some("stem observation expired"));
    }
}
