//! Guarded built-in content inputs and tool outputs; scripts keep their explicit execution policy.
use super::{EXTERNAL_COMMAND_TIMEOUT, artifacts::Workspace, types::PortValue};
use little_clip_core::item::ClipItem;
use little_clip_core::{CommandRequest, file_source::FileSource, media_backend::AudioFilter};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

pub(super) struct Input {
    pub(super) file: File,
    observed: FileSource,
}
impl Input {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let file = little_clip_core::kernel_interface::open_content(path, true)?;
        let observed = FileSource::capture_file(&file).map_err(io::Error::other)?;
        Ok(Self { file, observed })
    }
    pub(super) fn check(&self) -> io::Result<()> {
        self.observed
            .check_file(&self.file)
            .map_err(io::Error::other)
    }
    fn argument(&self, request: CommandRequest) -> io::Result<CommandRequest> {
        request.arg_owned_content_file(self.file.try_clone()?)
    }
}

// A built-in transform owns all admitted inputs and its unpublished output
// together. The shared publication point revalidates every input before install.
struct RenderFiles {
    inputs: Vec<Input>,
    output: tempfile::NamedTempFile,
}
impl RenderFiles {
    fn new(workspace: &Workspace, paths: &[&Path]) -> io::Result<Self> {
        let inputs = paths
            .iter()
            .map(|path| Input::open(path))
            .collect::<io::Result<Vec<_>>>()?;
        let output = workspace.stage_output()?;
        Ok(Self { inputs, output })
    }
    fn publish(self, workspace: &Workspace, destination: &Path) -> io::Result<()> {
        for input in &self.inputs {
            input.check()?;
        }
        workspace.install_output(self.output, destination)
    }
}

fn run(request: CommandRequest) -> Result<String, String> {
    let output = request
        .timeout(EXTERNAL_COMMAND_TIMEOUT)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Tool failed (exit {:?}): {}",
            output.status.code(),
            output.stderr_lossy()
        ));
    }
    Ok(output.stdout_lossy())
}

pub(super) fn checksum(path: &Path) -> Result<String, String> {
    let input = Input::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let output = run(input
        .argument(CommandRequest::new("sha256sum"))
        .map_err(|error| error.to_string())?)?;
    input.check().map_err(|error| error.to_string())?;
    Ok(output.split_whitespace().next().unwrap_or("").to_owned())
}

pub(super) fn image_bytes(path: &Path) -> Result<Vec<u8>, String> {
    const LIMIT: u64 = 128 * 1024 * 1024;
    let input = Input::open(path).map_err(|error| error.to_string())?;
    if input
        .file
        .metadata()
        .map_err(|error| error.to_string())?
        .len()
        > LIMIT
    {
        return Err("Pipeline image exceeds the 128 MiB input limit".into());
    }
    let mut bytes = Vec::new();
    (&input.file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("Pipeline image grew beyond its input limit".into());
    }
    input.check().map_err(|error| error.to_string())?;
    Ok(bytes)
}

pub(super) fn magick(
    workspace: &Workspace,
    paths: &[&Path],
    arguments: &[&str],
    destination: &Path,
) -> Result<(), String> {
    let files = RenderFiles::new(workspace, paths).map_err(|error| error.to_string())?;
    let format = destination
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or("Missing image output format")?;
    let mut last_error = String::new();
    for program in ["magick", "convert"] {
        let mut request = CommandRequest::new(program);
        for (input, path) in files.inputs.iter().zip(paths) {
            input.check().map_err(|error| error.to_string())?;
            if matches!(
                little_clip_core::item::guess_image_mime(path).as_str(),
                "image/x-tga" | "image/x-targa"
            ) {
                let (next, fd) = request
                    .inherit_handle(&input.file)
                    .map_err(|error| error.to_string())?;
                request = next.arg(format!("tga:/proc/self/fd/{fd}"));
            } else {
                request = input.argument(request).map_err(|error| error.to_string())?;
            }
        }
        let (request, fd) = request
            .args(arguments.iter().copied())
            .inherit_handle(files.output.as_file())
            .map_err(|error| error.to_string())?;
        match run(request.arg(format!("{format}:/proc/self/fd/{fd}"))) {
            Ok(_) => {
                return files
                    .publish(workspace, destination)
                    .map_err(|error| error.to_string());
            }
            Err(error) => {
                last_error = error;
                files
                    .output
                    .as_file()
                    .set_len(0)
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Err(last_error)
}

pub(super) fn audio(
    workspace: &Workspace,
    paths: &[&Path],
    filter: AudioFilter,
    destination: &Path,
) -> Result<(), String> {
    let files = RenderFiles::new(workspace, paths).map_err(|error| error.to_string())?;
    render_audio(&files, filter)?;
    files
        .publish(workspace, destination)
        .map_err(|error| error.to_string())
}

pub(super) fn audio_gain(
    workspace: &Workspace,
    source: &Path,
    gain_db: &str,
    destination: &Path,
) -> Result<(), String> {
    let gain_db = gain_db
        .parse::<f32>()
        .map_err(|_| "Enter a numeric gain in decibels")?;
    if gain_db.is_nan() || gain_db == f32::INFINITY {
        return Err("Enter a valid gain in decibels".into());
    }
    let files = RenderFiles::new(workspace, &[source]).map_err(|error| error.to_string())?;
    let result = little_clip_core::audio::render_wav(
        files.inputs[0]
            .file
            .try_clone()
            .map_err(|error| error.to_string())?,
        source,
        files
            .output
            .as_file()
            .try_clone()
            .map_err(|error| error.to_string())?,
        little_clip_core::audio::WavRenderOptions {
            gain_db,
            ..Default::default()
        },
        &std::sync::atomic::AtomicBool::new(false),
    );
    if let Err(error) = result {
        use std::io::Seek;
        tracing::debug!(%error, "Native audio gain deferred to the codec backend");
        files.inputs[0].check().map_err(|error| error.to_string())?;
        let mut output = files.output.as_file();
        output.set_len(0).map_err(|error| error.to_string())?;
        output.rewind().map_err(|error| error.to_string())?;
        render_audio(&files, AudioFilter::Chain(format!("volume={gain_db}dB")))?;
    }
    files
        .publish(workspace, destination)
        .map_err(|error| error.to_string())
}

fn render_audio(files: &RenderFiles, filter: AudioFilter) -> Result<(), String> {
    use little_clip_core::media_backend::{AudioInput, AudioRender};
    let mut settings = AudioRender::new(
        files
            .inputs
            .iter()
            .map(|_| AudioInput {
                formats: little_clip_core::media_backend::AUDIO_FORMATS.into(),
                stream: None,
            })
            .collect(),
        "wav",
    );
    settings.codec = Some("pcm_s16le".into());
    settings.filter = Some(filter);
    let mut request = little_clip_core::media_backend::render_audio_to_request(
        &settings,
        files.output.as_file(),
    )?;
    for input in &files.inputs {
        request = input.argument(request).map_err(|error| error.to_string())?;
    }
    run(request)?;
    Ok(())
}

impl PortValue {
    /// Materializes the port value into a new `ClipItem`.
    ///
    /// File batches retain reference semantics even for one image. Explicit image
    /// outputs use guarded, bounded hydration and fall back to a file reference
    /// when the image cannot be decoded within the input budget.
    pub fn into_clip_item(self, title_hint: &str) -> ClipItem {
        match self {
            PortValue::Text(text) => ClipItem::from_text(text),
            PortValue::FilePath(path) if !little_clip_core::item::is_image_path(&path) => {
                ClipItem::from_single_file(&path)
            }
            PortValue::ImagePath(path) | PortValue::FilePath(path) => {
                if let Ok(bytes) = image_bytes(&path) {
                    let mime = little_clip_core::item::guess_image_mime(&path);
                    if let Some(mut item) = ClipItem::from_image_bytes(bytes, mime) {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| title_hint.to_string());
                        item.title = file_name;
                        return item;
                    }
                }
                ClipItem::from_files(vec![path])
            }
            PortValue::AudioPath(path) => ClipItem::from_audio(path.clone())
                .unwrap_or_else(|| ClipItem::from_single_file(&path)),
            PortValue::SpectralPath(path) => ClipItem::from_single_file(&path),
            PortValue::FileBatch(paths) => {
                if paths.is_empty() {
                    ClipItem::from_text(title_hint.to_string())
                } else {
                    ClipItem::from_files(paths)
                }
            }
            PortValue::Automation(pts) => {
                let json = serde_json::to_string(&pts).unwrap_or_else(|_| "[]".to_string());
                ClipItem::from_text(json)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_fixture(path: &Path, format: image::ImageFormat) {
        image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 32, 64, 255]))
            .save_with_format(path, format)
            .unwrap();
    }

    #[test]
    fn image_tools_preserve_dimensions_and_tga_hints_through_descriptor_inputs() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        let workspace = Workspace::new().unwrap();
        for (name, format) in [
            (
                std::ffi::OsString::from_vec(b"image_\xff.png".to_vec()),
                image::ImageFormat::Png,
            ),
            ("image.tga".into(), image::ImageFormat::Tga),
        ] {
            let source = root.path().join(name);
            image_fixture(&source, format);
            let destination = workspace
                .path()
                .join(format!("{}.png", uuid::Uuid::new_v4()));
            magick(
                &workspace,
                &[&source],
                &["-bordercolor", "red", "-border", "2"],
                &destination,
            )
            .unwrap();
            assert_eq!(image::image_dimensions(destination).unwrap(), (7, 6));
        }
        let source = root.path().join("source.png");
        image_fixture(&source, image::ImageFormat::Png);
        let joined = workspace.path().join("joined.webp");
        magick(&workspace, &[&source, &source], &["+append"], &joined).unwrap();
        assert_eq!(image::image_dimensions(joined).unwrap(), (6, 2));
    }

    #[test]
    fn native_gain_preserves_stereo_timing_and_scales_pcm() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.wav");
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 8000, 2);
        buffer.push_samples(&vec![1234; 16_000]);
        buffer.write_wav(&source).unwrap();
        let workspace = Workspace::new().unwrap();
        let output = workspace.path().join("native-gain.wav");
        audio_gain(&workspace, &source, "-6.0206", &output).unwrap();
        let metadata = little_clip_core::audio::read_audio_metadata(&output).unwrap();
        assert_eq!(
            (
                metadata.sample_rate,
                metadata.channels,
                metadata.duration_ms
            ),
            (8000, 2, 1000)
        );
        let (peaks, _) = little_clip_core::audio::compute_preview_waveform(
            File::open(&output).unwrap(),
            &output,
            1.0,
            8,
            None,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            peaks
                .iter()
                .all(|peak| (*peak - 617.0 / 32768.0).abs() <= 1.0 / 32768.0)
        );
        let invalid = workspace.path().join("invalid.wav");
        assert!(audio_gain(&workspace, &source, "NaN", &invalid).is_err());
        assert!(!invalid.exists());
    }

    #[test]
    fn ffmpeg_tools_finalize_stereo_wave_outputs_and_concat_inputs() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.wav");
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 8000, 2);
        buffer.push_samples(&vec![1234; 16_000]);
        buffer.write_wav(&source).unwrap();
        let workspace = Workspace::new().unwrap();
        let adjusted = workspace.path().join("adjusted.wav");
        audio(
            &workspace,
            &[&source],
            AudioFilter::Chain("volume=0.5".into()),
            &adjusted,
        )
        .unwrap();
        let metadata = little_clip_core::audio::read_audio_metadata(&adjusted).unwrap();
        assert_eq!(metadata.sample_rate, 8000);
        assert_eq!(metadata.channels, 2);
        assert_eq!(metadata.duration_ms, 1000);
        let joined = workspace.path().join("joined.wav");
        audio(
            &workspace,
            &[&source, &source],
            AudioFilter::Graph {
                spec: "[0:a][1:a]concat=n=2:v=0:a=1[out]".into(),
                output: "out".into(),
            },
            &joined,
        )
        .unwrap();
        assert_eq!(
            little_clip_core::audio::read_audio_metadata(&joined)
                .unwrap()
                .duration_ms,
            2000
        );
    }

    #[test]
    fn kernel_content_is_rejected_before_tools_or_image_reads() {
        let workspace = Workspace::new().unwrap();
        let destination = workspace.path().join("forbidden.png");
        for source in [Path::new("/proc/version"), Path::new("/dev/null")] {
            assert!(checksum(source).is_err());
            assert!(image_bytes(source).is_err());
            assert!(magick(&workspace, &[source], &[], &destination).is_err());
            assert!(
                audio(
                    &workspace,
                    &[source],
                    AudioFilter::Chain("anull".into()),
                    &destination
                )
                .is_err()
            );
            assert!(audio_gain(&workspace, source, "0", &destination).is_err());
        }
        assert!(!destination.exists());
    }

    #[test]
    fn inherited_input_does_not_follow_rebound_aliases_and_rejects_modified_bytes() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.txt");
        let second = root.path().join("second.txt");
        let alias = root.path().join("alias.txt");
        std::fs::write(&first, b"abc").unwrap();
        std::fs::write(&second, b"different bytes").unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();
        let input = Input::open(&alias).unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&second, &alias).unwrap();
        let output = run(input.argument(CommandRequest::new("sha256sum")).unwrap()).unwrap();
        assert!(
            output.starts_with("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        input.check().unwrap();
        std::fs::write(&first, b"changed in place").unwrap();
        assert!(input.check().is_err());
    }
}
