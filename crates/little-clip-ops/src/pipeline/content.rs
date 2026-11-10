//! Guarded built-in content inputs and tool outputs; scripts keep their explicit execution policy.
use super::{EXTERNAL_COMMAND_TIMEOUT, artifacts::Workspace, types::PortValue};
use little_clip_core::item::ClipItem;
use little_clip_core::{CommandRequest, file_source::FileSource};
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
        if FileSource::capture_file(&self.file).map_err(io::Error::other)? != self.observed {
            return Err(io::Error::other("Pipeline input changed while processing"));
        }
        self.observed.check().map_err(io::Error::other)
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
    arguments: &[&str],
    destination: &Path,
) -> Result<(), String> {
    let files = RenderFiles::new(workspace, paths).map_err(|error| error.to_string())?;
    let mut request =
        CommandRequest::new("ffmpeg").args(["-nostdin", "-y", "-protocol_whitelist", "file,pipe"]);
    for input in &files.inputs {
        request = input
            .argument(request.arg("-i"))
            .map_err(|error| error.to_string())?;
    }
    let (request, fd) = request
        .args(arguments.iter().copied())
        .args(["-f", "wav"])
        .inherit_handle(files.output.as_file())
        .map_err(|error| error.to_string())?;
    run(request.arg(format!("/proc/self/fd/{fd}")))?;
    files
        .publish(workspace, destination)
        .map_err(|error| error.to_string())
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
            PortValue::FileBatch(paths) => {
                if paths.is_empty() {
                    ClipItem::from_text(title_hint.to_string())
                } else {
                    ClipItem::from_files(paths)
                }
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
    fn ffmpeg_tools_finalize_stereo_wave_outputs_and_concat_inputs() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.wav");
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 8000, 2);
        buffer.push_samples(&vec![1234; 16_000]);
        buffer.write_wav(&source).unwrap();
        let workspace = Workspace::new().unwrap();
        let adjusted = workspace.path().join("adjusted.wav");
        audio(&workspace, &[&source], &["-af", "volume=0.5"], &adjusted).unwrap();
        let metadata = little_clip_core::audio::read_audio_metadata(&adjusted).unwrap();
        assert_eq!(metadata.sample_rate, 8000);
        assert_eq!(metadata.channels, 2);
        assert_eq!(metadata.duration_ms, 1000);
        let joined = workspace.path().join("joined.wav");
        audio(
            &workspace,
            &[&source, &source],
            &[
                "-filter_complex",
                "[0:0][1:0]concat=n=2:v=0:a=1[out]",
                "-map",
                "[out]",
            ],
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
            assert!(audio(&workspace, &[source], &[], &destination).is_err());
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
