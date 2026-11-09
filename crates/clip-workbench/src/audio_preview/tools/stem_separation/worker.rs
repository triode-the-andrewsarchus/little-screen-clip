//! Checked normalization and descriptor-backed inputs/outputs for stem inference.
use super::*;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;

pub(super) struct StemOutput {
    pub(super) name: String,
    pub(super) identity: crate::file_source::FileSource,
    pub(super) check: crate::file_source::ReadCheck,
    pub(super) metadata: Option<little_clip_core::audio::AudioMetadata>,
}

pub(super) struct StemFiles {
    // Cleanup through the retained parent before either descriptor closes.
    _temporary: tempfile::TempDir,
    _parent: std::fs::File,
    directory: std::fs::File,
}

#[derive(Clone)]
pub(super) struct StemWorkerCommand {
    pub(super) program: PathBuf,
    pub(super) prefix_args: Vec<std::ffi::OsString>,
}

impl StemWorkerCommand {
    pub(super) fn configured() -> Self {
        let worker =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/separate_stems.py");
        let venv_python = std::env::var("HOME")
            .map(|home| {
                PathBuf::from(home).join(".local/share/little-screen-clip/nobg-venv/bin/python")
            })
            .ok()
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("python3"));
        Self {
            program: venv_python,
            prefix_args: vec!["-I".into(), worker.into_os_string()],
        }
    }

    fn command(
        &self,
        source: &Path,
        directory: &std::fs::File,
    ) -> Result<std::process::Command, String> {
        let request = little_clip_core::process::CommandRequest::new(self.program.as_os_str())
            .args(self.prefix_args.iter().cloned())
            .arg_content_file(source)
            .map_err(|error| error.to_string())?;
        let (request, output_fd) = request
            .inherit_handle(directory)
            .map_err(|error| error.to_string())?;
        Ok(request
            .arg(format!("/proc/self/fd/{output_fd}"))
            .build_command())
    }
}

pub(super) fn run_stem_worker(
    worker: StemWorkerCommand,
    source: AudioEditSnapshot,
    filter: (String, export::AudioRenderMode),
    workspace: &Path,
    control: &crate::image_preview::ImageToolProcessControl,
) -> Result<Vec<StemOutput>, String> {
    if control.cancel.load(Ordering::Acquire) {
        return Err("Stem separation cancelled".into());
    }
    let rendered = session_edit::render_session_file(
        &source.path,
        source.file_source.as_ref(),
        &source.read_check,
        filter.0,
        filter.1,
        &control.cancel,
    )?;
    let parent = little_clip_core::kernel_interface::create_content_directory(workspace)
        .map_err(|error| error.to_string())?;
    let temporary = tempfile::Builder::new()
        .prefix("stem-job-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(format!("/proc/self/fd/{}", parent.as_raw_fd()))
        .map_err(|error| error.to_string())?;
    let directory = little_clip_core::kernel_interface::pin_content_directory(temporary.path())
        .map_err(|error| error.to_string())?;
    let files = Arc::new(StemFiles {
        _temporary: temporary,
        _parent: parent,
        directory,
    });
    let input = PathBuf::from(format!(
        "/proc/self/fd/{}",
        rendered.staging.as_file().as_raw_fd()
    ));
    let mut command = worker.command(&input, &files.directory)?;
    // The helper receives two inherited descriptors. Its manifest names only
    // direct children of this private job, never arbitrary paths to read later.
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = crate::image_preview::spawn_image_tool_child(command, control)
        .map_err(|error| format!("Could not launch Python stem worker: {error}"))?;
    let pid = child.id();
    let output = child
        .wait_with_output()
        .map_err(|error| format!("Could not wait for Python stem worker: {error}"));
    crate::image_preview::clear_image_tool_pid_if_owned(&control.child_pid, pid);
    if control.cancel.load(Ordering::Acquire) {
        return Err("Stem separation cancelled".into());
    }
    let output = output?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if message.is_empty() {
            "HTDemucs separation failed".into()
        } else {
            message
        });
    }
    source.read_check.validate(&control.cancel)?;
    rendered.observed.check()?;
    let stems = parse_stem_manifest(&output.stdout)?;
    stems
        .into_iter()
        .map(|(name, path)| prepare_output(name, path, files.clone(), &control.cancel))
        .collect()
}

fn prepare_output(
    name: String,
    path: PathBuf,
    files: Arc<StemFiles>,
    cancel: &AtomicBool,
) -> Result<StemOutput, String> {
    let filename = format!("{name}.wav");
    if path != Path::new(&filename) {
        return Err("Stem worker returned an output outside its job directory".into());
    }
    let pinned =
        PathBuf::from(format!("/proc/self/fd/{}", files.directory.as_raw_fd())).join(&filename);
    let identity = crate::file_source::FileSource::capture(&pinned)?;
    let expected = identity.clone();
    let check = crate::file_source::ReadCheck::new(move |cancel| {
        if cancel.load(Ordering::Acquire) {
            return Err("Stem read cancelled".into());
        }
        let _owner = &files;
        if crate::file_source::FileSource::capture(&pinned)? != expected {
            return Err("Separated stem changed".into());
        }
        expected.check()
    });
    let metadata = check.read(cancel, || Ok(read_audio_metadata(identity.path())))?;
    Ok(StemOutput {
        name,
        identity,
        check,
        metadata,
    })
}
