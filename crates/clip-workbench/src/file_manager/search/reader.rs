//! Disposable source-reader process. No source payload is read on the UI thread.
mod inventory;
mod media;
mod svg;
mod video;
use super::{
    policy::{self, Root},
    session::SourceSession,
};
use crate::file_manager::filesystem::{self, Directory, Entry};
use base64::{Engine, engine::general_purpose::STANDARD};
use little_clip_core::process::CommandRequest;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

// Decoder admission, independent of extension classification. In particular,
// disguised HLS/concat/playlist inputs cannot fetch URLs or unrelated files.
const AUDIO_DEMUXERS: &str =
    "wav,mp3,flac,ogg,aac,aiff,asf,ape,wv,tta,ac3,eac3,dts,amr,au,caf,mov,mp4,m4a,3gp,3g2,mj2";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SourceRoot {
    #[serde(with = "crate::file_manager::path_serde")]
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    #[serde(default)]
    pub mount: Option<u64>,
    #[serde(default)]
    pub session: Option<SourceSession>,
}
impl SourceRoot {
    fn capture(path: &Path) -> Result<Self, String> {
        let root = Root::open(path)?;
        Ok(Self::from_root(&root))
    }
    pub(super) fn from_root(root: &Root) -> Self {
        Self {
            path: root.path.clone(),
            device: root.device,
            inode: root.inode,
            mount: Some(root.mount),
            session: SourceSession::capture(root),
        }
    }
    fn validate(&self) -> Result<Root, String> {
        let expected_mount = self
            .mount
            .ok_or("Indexed source needs a current mount observation before content access")?;
        let root = Root::open(&self.path)?;
        if (root.device, root.inode, root.mount) != (self.device, self.inode, expected_mount) {
            return Err("Indexed root was replaced or remounted".into());
        }
        if self.session.is_some() && self.session != SourceSession::capture(&root) {
            return Err("Indexed source session changed".into());
        }
        Ok(root)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Request {
    Restore {
        fds: [i32; 3],
        max_bytes: u64,
        max_vectors: u64,
        page_size: u32,
    },
    ImageHash {
        entry: Entry,
        cached: Option<u64>,
    },
    Probe(#[serde(with = "crate::file_manager::path_serde")] PathBuf),
    List {
        #[serde(default)]
        output: Option<i32>,
        root: SourceRoot,
        #[serde(with = "crate::file_manager::path_serde")]
        path: PathBuf,
        hidden: bool,
        #[serde(with = "crate::file_manager::path_serde::list")]
        exclusions: Vec<PathBuf>,
    },
    Watch {
        root: SourceRoot,
        #[serde(with = "crate::file_manager::path_serde")]
        path: PathBuf,
        fd: i32,
    },
    Extract {
        root: SourceRoot,
        entry: Entry,
        cursor: usize,
    },
    Header {
        root: SourceRoot,
        entry: Entry,
    },
    Verify {
        root: SourceRoot,
        entry: Entry,
    },
    Fingerprint {
        root: SourceRoot,
        entry: Entry,
    },
    VerifyBatch {
        root: SourceRoot,
        entries: Vec<Entry>,
    },
    Absent {
        root: SourceRoot,
        #[serde(with = "crate::file_manager::path_serde")]
        path: PathBuf,
    },
}
#[derive(Serialize, Deserialize)]
pub(super) enum Reply {
    Restored(u64),
    ImageHash(u64),
    Watch(i32),
    Root(SourceRoot),
    Directory(Directory),
    DirectoryWritten,
    Prepared(Prepared),
    Header(String),
    Verified,
    Fingerprint(Option<[u8; 32]>),
    VerifiedBatch(Vec<usize>),
}
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Prepared {
    pub text: String,
    /// Encoded PNG or WAV; never a path to source material.
    pub data: Option<String>,
    pub modality: String,
    pub location: String,
    pub next: Option<usize>,
    pub coverage: String,
    #[serde(default)]
    pub vision_tokens: Option<u32>,
}

pub(super) fn call(request: &Request, cancel: &AtomicBool) -> Result<Reply, String> {
    if matches!(request, Request::List { output: None, .. }) {
        inventory::call(request, cancel)
    } else {
        call_with(request, CommandRequest::new("/proc/self/exe"), cancel)
    }
}

pub(super) fn watch(
    root: &SourceRoot,
    path: &Path,
    handle: &std::fs::File,
    cancel: &AtomicBool,
) -> Result<i32, String> {
    let (command, fd) = CommandRequest::new("/proc/self/exe")
        .inherit_handle(handle)
        .map_err(|e| e.to_string())?;
    match call_with(
        &Request::Watch {
            root: root.clone(),
            path: path.into(),
            fd,
        },
        command,
        cancel,
    )? {
        Reply::Watch(wd) => Ok(wd),
        _ => Err("Invalid watch registration reply".into()),
    }
}

pub(super) fn prepare_restore(
    handles: [&std::fs::File; 3],
    max_bytes: u64,
    max_vectors: u64,
    page_size: u32,
    cancel: &AtomicBool,
) -> Result<u64, String> {
    let mut command = CommandRequest::new("/proc/self/exe");
    let mut fds = [0; 3];
    for (index, handle) in handles.into_iter().enumerate() {
        let (next, fd) = command.inherit_handle(handle).map_err(|e| e.to_string())?;
        command = next;
        fds[index] = fd;
    }
    match call_with(
        &Request::Restore {
            fds,
            max_bytes,
            max_vectors,
            page_size,
        },
        command,
        cancel,
    )? {
        Reply::Restored(vectors) => Ok(vectors),
        _ => Err("Invalid index restore reply".into()),
    }
}

fn restore_helper(
    fds: [i32; 3],
    max_bytes: u64,
    max_vectors: u64,
    page_size: u32,
) -> Result<Reply, String> {
    use std::os::fd::BorrowedFd;
    if fds.iter().any(|fd| *fd < 3) || fds[0] == fds[1] || fds[1] == fds[2] || fds[0] == fds[2] {
        return Err("Invalid inherited restore descriptors".into());
    }
    let handles = fds
        .map(|fd| {
            // SAFETY: these descriptors are inherited from owned handles. Cloning
            // validates them without taking ownership of the original numbers.
            unsafe { BorrowedFd::borrow_raw(fd) }
                .try_clone_to_owned()
                .map(std::fs::File::from)
                .map_err(|e| e.to_string())
        })
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    super::store::prepare_in_helper(
        &handles[0],
        &handles[1],
        &handles[2],
        max_bytes,
        max_vectors,
        page_size,
    )
    .map(Reply::Restored)
}

fn call_with(
    request: &Request,
    command: CommandRequest,
    cancel: &AtomicBool,
) -> Result<Reply, String> {
    let timeout = if matches!(request, Request::Restore { .. }) {
        300
    } else {
        90
    };
    let output = command
        .arg("--internal-file-search-reader")
        .stdin_bytes(serde_json::to_vec(request).map_err(|e| e.to_string())?)
        .own_process_group()
        .timeout(Duration::from_secs(timeout))
        .max_output_bytes(24 * 1024 * 1024)
        .run_cancellable(cancel)
        .map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(format!("Search reader failed: {}", output.stderr_lossy()));
    }
    serde_json::from_slice::<Result<Reply, String>>(&output.stdout)
        .map_err(|e| format!("Invalid search reader reply: {e}"))?
}

pub(crate) fn run() -> Result<(), String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Search reader request exceeds limit".into());
    }
    let request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    reader_limits(&request)?;
    let result = execute(request);
    serde_json::to_writer(std::io::stdout().lock(), &result).map_err(|e| e.to_string())
}

// Called only in the disposable helper, never by in-process test execution.
fn reader_limits(request: &Request) -> Result<(), String> {
    let (cpu, hard) = if matches!(request, Request::Restore { .. }) {
        (240, 250)
    } else {
        (60, 70)
    };
    for (resource, soft, hard) in [
        (libc::RLIMIT_AS, 2_147_483_648, 2_147_483_648),
        (libc::RLIMIT_CPU, cpu, hard),
    ] {
        let limit = libc::rlimit {
            rlim_cur: soft,
            rlim_max: hard,
        };
        // SAFETY: this affects only the owned reader subprocess and its children.
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

fn execute(request: Request) -> Result<Reply, String> {
    match request {
        Request::Restore {
            fds,
            max_bytes,
            max_vectors,
            page_size,
        } => restore_helper(fds, max_bytes, max_vectors, page_size),
        Request::ImageHash { entry, cached } => image_hash(&entry, cached),
        Request::Probe(path) => SourceRoot::capture(&path).map(Reply::Root),
        Request::VerifyBatch { root, entries } => verify_batch(&root, &entries),
        Request::Absent { root, path } => absence(&root, &path),
        Request::List {
            root,
            path,
            hidden,
            exclusions,
            output,
        } => inventory::reply(list(&root, &path, hidden, &exclusions)?, output),
        Request::Watch { root, path, fd } => attach_watch(&root, &path, fd).map(Reply::Watch),
        Request::Verify { root, entry } => verify_target(&root, &entry),
        Request::Fingerprint { root, entry } => fingerprint(&root, &entry),
        Request::Extract {
            root,
            entry,
            cursor,
        } => {
            let origin = root.validate()?;
            let mut file = source(&origin, &entry)?;
            let prepared = extract(&entry, &mut file, cursor)?;
            verify(&origin, &entry, &file)?;
            Ok(Reply::Prepared(prepared))
        }
        Request::Header { root, entry } => {
            let origin = root.validate()?;
            let file = source(&origin, &entry)?;
            let mut bytes = Vec::new();
            (&file)
                .take(8192)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            verify(&origin, &entry, &file)?;
            // Keep only the valid UTF-8 prefix. An invalid/truncated body must
            // not damage a complete header, and replacement characters must
            // never manufacture a field inside a malformed header.
            let valid = std::str::from_utf8(&bytes).map_or_else(
                |error| std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
                |text| text,
            );
            Ok(Reply::Header(valid.into()))
        }
    }
}

fn fingerprint(root: &SourceRoot, entry: &Entry) -> Result<Reply, String> {
    use sha2::{Digest, Sha256};
    let origin = root.validate()?;
    let mut file = source(&origin, entry)?;
    if entry.size_bytes > 64 * 1024 * 1024 {
        return Ok(Reply::Fingerprint(None));
    }
    let mut digest = Sha256::new();
    let mut remaining = entry.size_bytes;
    let mut buffer = [0u8; 65536];
    while remaining > 0 {
        let length = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..length])
            .map_err(|e| e.to_string())?;
        digest.update(&buffer[..length]);
        remaining -= length as u64;
    }
    // The same pinned source and its path must still have the captured revision.
    verify(&origin, entry, &file)?;
    Ok(Reply::Fingerprint(Some(digest.finalize().into())))
}

fn attach_watch(root: &SourceRoot, path: &Path, fd: i32) -> Result<i32, String> {
    let origin = root.validate()?;
    if !path.starts_with(&root.path) {
        return Err("Watch is outside the enrolled root".into());
    }
    let folder = origin.child(path)?;
    if folder.path != path || folder.device != root.device {
        return Err("Watch crosses an alias or mount".into());
    }
    let label =
        std::ffi::CString::new(format!("/proc/self/fd/{}", folder.directory.as_raw_fd())).unwrap();
    let mask = libc::IN_ONLYDIR
        | libc::IN_MODIFY
        | libc::IN_CLOSE_WRITE
        | libc::IN_ATTRIB
        | libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_DELETE_SELF
        | libc::IN_MOVE_SELF;
    // SAFETY: fd is an explicitly inherited inotify instance, label names the
    // held ordinary directory, and the kernel copies its nul-terminated bytes.
    let wd = unsafe { libc::inotify_add_watch(fd, label.as_ptr(), mask) };
    if wd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if let Err(error) = folder.revalidate().and_then(|_| origin.revalidate()) {
        // SAFETY: remove only the registration just returned by this instance.
        unsafe {
            libc::inotify_rm_watch(fd, wd);
        }
        return Err(error);
    }
    Ok(wd)
}

fn image_hash(entry: &Entry, cached: Option<u64>) -> Result<Reply, String> {
    if !entry.is_photo() || entry.symlink {
        return Err("Choose an ordinary image file".into());
    }
    let file = little_clip_core::kernel_interface::open_content(&entry.path, false)
        .map_err(|e| e.to_string())?;
    if entry.identity
        != Some(filesystem::identity(
            &file.metadata().map_err(|e| e.to_string())?,
        ))
    {
        return Err("Image changed since enumeration".into());
    }
    let hash = match cached {
        Some(hash) => hash,
        None => crate::file_manager::image_strip::perceptual(
            &entry.path,
            entry.revision,
            &AtomicBool::new(false),
        )?,
    };
    filesystem::check_revision(&entry.path, entry.revision)?;
    if entry.identity
        != Some(filesystem::identity(
            &file.metadata().map_err(|e| e.to_string())?,
        ))
    {
        return Err("Image changed during analysis".into());
    }
    Ok(Reply::ImageHash(hash))
}

fn verify_target(root: &SourceRoot, entry: &Entry) -> Result<Reply, String> {
    let origin = root.validate()?;
    if entry.directory {
        if entry.symlink || !entry.path.starts_with(&root.path) {
            return Err("Search directory is outside its source".into());
        }
        let folder = origin.child(&entry.path)?;
        if folder.path != entry.path
            || entry.identity.map(|id| id.object()) != Some((folder.device, folder.inode))
        {
            return Err("Search directory changed since enumeration".into());
        }
        origin.revalidate()?;
    } else {
        let file = source(&origin, entry)?;
        verify(&origin, entry, &file)?;
    }
    Ok(Reply::Verified)
}

fn verify_batch(root: &SourceRoot, entries: &[Entry]) -> Result<Reply, String> {
    if entries.len() > 1000 {
        return Err("Validation batch exceeds result cap".into());
    }
    let origin = root.validate()?;
    let accepted = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let file = source(&origin, entry).ok()?;
            verify(&origin, entry, &file).ok()?;
            Some(index)
        })
        .collect();
    origin.revalidate()?;
    Ok(Reply::VerifiedBatch(accepted))
}

fn absence(root: &SourceRoot, path: &Path) -> Result<Reply, String> {
    let origin = root.validate()?;
    if !path.starts_with(&root.path) || path == root.path.as_path() {
        return Err("Invalid absence-check scope".into());
    }
    origin.confirms_absence(path)?;
    origin.revalidate()?;
    Ok(Reply::Verified)
}
fn list(
    root: &SourceRoot,
    path: &Path,
    hidden: bool,
    exclusions: &[PathBuf],
) -> Result<Directory, String> {
    if policy::excluded_by_user(path, exclusions) {
        return Err("Directory is excluded by search preferences".into());
    }
    let origin = root.validate()?;
    if !path.starts_with(&root.path) {
        return Err("Directory is outside the enrolled root".into());
    }
    let folder = origin.child(path)?;
    if folder.path != path || folder.device != root.device {
        return Err("Search does not cross aliases or nested mounts".into());
    }
    let mut directory = filesystem::read_pinned_directory(
        path,
        &folder.directory,
        &AtomicBool::new(false),
        100_000,
    )?;
    directory = admit_inventory(&origin, directory, hidden, exclusions);
    folder.revalidate()?;
    origin.revalidate()?;
    Ok(directory)
}

fn admit_inventory(
    origin: &Root,
    mut directory: Directory,
    hidden: bool,
    exclusions: &[PathBuf],
) -> Directory {
    directory.skipped += directory
        .entries
        .iter()
        .filter(|entry| entry.warning.is_some() && !entry.symlink)
        .count();
    for entry in std::mem::take(&mut directory.entries) {
        if !eligible(&entry, origin, hidden) || policy::excluded_by_user(&entry.path, exclusions) {
            continue;
        }
        match origin.admit(&entry.path) {
            Ok(Some(pinned)) => {
                let identity = pinned
                    .metadata()
                    .ok()
                    .map(|metadata| filesystem::identity(&metadata));
                if identity == entry.identity {
                    directory.entries.push(entry);
                } else {
                    directory.skipped += 1;
                }
            }
            Ok(None) => {} // Nested mounts/aliases need their own explicit root.
            Err(_) => directory.skipped += 1,
        }
    }
    directory
}

fn eligible(entry: &Entry, root: &Root, hidden: bool) -> bool {
    !entry.symlink
        && entry.system.is_none()
        && entry.warning.is_none()
        && !policy::excluded_from_root(&entry.path, &root.path, hidden)
        && entry
            .identity
            .is_some_and(|id| id.object().0 == root.device)
}

fn source(root: &Root, entry: &Entry) -> Result<std::fs::File, String> {
    if !entry.path.starts_with(&root.path) || !entry.regular_file || entry.symlink {
        return Err("Unsupported search source".into());
    }
    let file = root.open_content(&entry.path)?;
    if policy::check_handle(&file)? != entry.path {
        return Err("Search source moved or crosses a directory alias".into());
    }
    if entry.identity
        != Some(filesystem::identity(
            &file.metadata().map_err(|e| e.to_string())?,
        ))
    {
        return Err("Search source changed since enumeration".into());
    }
    Ok(file)
}
fn verify(root: &Root, entry: &Entry, file: &std::fs::File) -> Result<(), String> {
    if policy::check_handle(file)? != entry.path {
        return Err("Search source moved while reading".into());
    }
    if entry.identity
        != Some(filesystem::identity(
            &file.metadata().map_err(|e| e.to_string())?,
        ))
    {
        return Err("Search source changed while reading".into());
    }
    let current = root.open_content(&entry.path)?;
    if entry.identity
        != Some(filesystem::identity(
            &current.metadata().map_err(|e| e.to_string())?,
        ))
    {
        return Err("Search source changed while verifying its current path".into());
    }
    filesystem::check_revision(&entry.path, entry.revision)?;
    root.revalidate()
}

fn extract(entry: &Entry, file: &mut std::fs::File, cursor: usize) -> Result<Prepared, String> {
    let held = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    if little_clip_core::pdf::is_pdf_path(&entry.path) {
        if entry.size_bytes > 512 * 1024 * 1024 {
            return Ok(Prepared {
                coverage: "name-only: PDF exceeds 512 MiB source budget".into(),
                ..Default::default()
            });
        }
        return pdf(&held, cursor);
    }
    if entry.is_photo() {
        if entry.size_bytes > 256 * 1024 * 1024 {
            return Ok(Prepared {
                coverage: "name-only: image exceeds 256 MiB source budget".into(),
                ..Default::default()
            });
        }
        return photo(entry, file, &held);
    }
    if little_clip_core::audio::is_audio_path(&entry.path) {
        return audio(&held, cursor, entry.size_bytes);
    }
    if entry.is_video() {
        return video::prepare(&held, cursor, entry.size_bytes);
    }
    text(entry, file, cursor)
}
fn text(entry: &Entry, file: &mut std::fs::File, cursor: usize) -> Result<Prepared, String> {
    let extension = entry
        .path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let kind = if matches!(
        extension.as_str(),
        "html" | "htm" | "xhtml" | "docx" | "odt" | "pptx" | "xlsx"
    ) {
        extension.as_str()
    } else if crate::file_manager::text_strip::known_text(&entry.path)
        || matches!(extension.as_str(), "rst" | "org")
    {
        "txt"
    } else {
        return Ok(Prepared {
            coverage: "name-only: extractor unavailable".into(),
            ..Default::default()
        });
    };
    if entry.size_bytes > 64 * 1024 * 1024 {
        return Ok(Prepared {
            coverage: "name-only: document exceeds 64 MiB extraction budget".into(),
            ..Default::default()
        });
    }
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/search_model");
    let script = super::runtime::helper_script("documents.py", &bundled)?;
    let output = CommandRequest::new("python3")
        .arg("-I")
        .arg(script)
        .args(["--kind", kind, "--cursor", &cursor.to_string()])
        .arg_content_file(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|e| e.to_string())?
        .timeout(Duration::from_secs(30))
        .max_output_bytes(24 * 1024 * 1024)
        .run()
        .map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err(format!(
            "Document extraction failed: {}",
            output.stderr_lossy()
        ));
    }
    serde_json::from_slice::<Result<Prepared, String>>(&output.stdout)
        .map_err(|e| format!("Invalid document extraction reply: {e}"))?
}

fn pdf(path: &Path, cursor: usize) -> Result<Prepared, String> {
    let info = little_clip_core::pdf::get_pdf_info(path)?;
    let page = cursor + 1;
    if page > info.page_count.min(512) {
        return Err("PDF page is outside extraction budget".into());
    }
    let text = little_clip_core::pdf::extract_pdf_page_text(path, page)?;
    let data = if text.trim().chars().count() < 200 {
        Some(STANDARD.encode(little_clip_core::pdf::render_pdf_page_thumbnail(path, page, 1024)?.0))
    } else {
        None
    };
    Ok(Prepared {
        modality: if data.is_some() { "image" } else { "text" }.into(),
        text,
        data,
        location: format!("page {page}"),
        next: (page < info.page_count.min(512)).then_some(page),
        coverage: if info.page_count > 512 {
            "partial: 512 page limit"
        } else {
            "complete"
        }
        .into(),
        ..Default::default()
    })
}

fn photo(entry: &Entry, file: &mut std::fs::File, held: &Path) -> Result<Prepared, String> {
    use image::ImageDecoder;
    let mime = little_clip_core::item::guess_image_mime(&entry.path);
    let bytes = if little_clip_core::raw_image::is_raw_image_path(&entry.path) {
        let output = CommandRequest::new("exiftool")
            .args(["-b", "-PreviewImage"])
            .arg_content_file(held)
            .map_err(|e| e.to_string())?
            .max_output_bytes(16 * 1024 * 1024)
            .run()
            .map_err(|e| e.to_string())?;
        if !output.status.success() || output.stdout_truncated {
            return Err("RAW embedded preview unavailable".into());
        }
        output.stdout
    } else {
        let mut bytes = Vec::new();
        file.take(256 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        bytes
    };
    if little_clip_core::item::is_svg_bytes(&bytes) {
        return svg::prepare(&bytes);
    }
    let hint = if little_clip_core::raw_image::is_raw_image_path(&entry.path) {
        "image/jpeg"
    } else {
        &mime
    };
    let mut reader =
        little_clip_core::item::image_reader(&bytes, Some(hint)).map_err(|e| e.to_string())?;
    reader.limits(crate::file_manager::media::raster_preview_limits());
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .thumbnail(1024.min(image.width()), 1024.min(image.height()))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(Prepared {
        data: Some(STANDARD.encode(encoded.into_inner())),
        modality: "image".into(),
        location: "image".into(),
        coverage: "complete".into(),
        ..Default::default()
    })
}

fn audio(path: &Path, cursor: usize, bytes: u64) -> Result<Prepared, String> {
    if bytes > 64 * 1024 * 1024 * 1024 {
        return Ok(Prepared {
            coverage: "name-only: audio exceeds 64 GiB source budget".into(),
            ..Default::default()
        });
    }
    let value = media::probe(path, AUDIO_DEMUXERS)?;
    let track = media::audio_track(&value).ok_or("Audio track or duration unavailable")?;
    media::audio_window(path, AUDIO_DEMUXERS, &track, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only_source(folder: &Path) -> (SourceRoot, Entry) {
        let root = SourceRoot::capture(folder).unwrap();
        let mut directory = filesystem::read_directory(folder, &AtomicBool::new(false)).unwrap();
        assert_eq!(directory.entries.len(), 1);
        (root, directory.entries.remove(0))
    }
    #[test]
    fn full_fingerprint_hashes_verified_bytes_and_skips_large_payloads() {
        use sha2::{Digest, Sha256};
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("note.txt");
        std::fs::write(&path, b"abc").unwrap();
        let (root, entry) = only_source(folder.path());
        let Reply::Fingerprint(Some(digest)) = fingerprint(&root, &entry).unwrap() else {
            panic!()
        };
        assert_eq!(digest.as_slice(), Sha256::digest(b"abc").as_slice());
        std::fs::write(&path, b"xyz").unwrap();
        assert!(fingerprint(&root, &entry).is_err());
        std::fs::File::create(&path)
            .unwrap()
            .set_len(64 * 1024 * 1024 + 1)
            .unwrap();
        let entry = filesystem::read_directory(folder.path(), &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        assert!(matches!(
            fingerprint(&root, &entry).unwrap(),
            Reply::Fingerprint(None)
        ));
    }

    #[test]
    #[ignore = "run explicitly inside an isolated user/mount namespace"]
    fn bind_mount_boundaries_block_inventory_extraction_and_absence_checks() {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::env::var("CLIP_SEARCH_PRIVATE_MOUNT_TEST").as_deref(),
            Ok("1")
        );
        let fixture = PathBuf::from(
            std::env::var_os("CLIP_SEARCH_PRIVATE_MOUNT_FIXTURE")
                .expect("The namespace runner owns and cleans the fixture"),
        );
        assert!(fixture.is_dir());
        assert!(std::fs::read_dir(&fixture).unwrap().next().is_none());
        let enrolled = fixture.join("enrolled");
        let outside = fixture.join("outside");
        let nested = enrolled.join("mounted");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(enrolled.join("local.txt"), "ordinary content").unwrap();
        std::fs::write(outside.join("canary.txt"), "must not enter enclosing root").unwrap();
        let mount = |source: &Path, target: &Path| {
            assert!(
                std::process::Command::new("mount")
                    .arg("--bind")
                    .arg(source)
                    .arg(target)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        mount(&outside, &nested);
        let file_mount = enrolled.join("mounted-file.txt");
        std::fs::write(&file_mount, "original local file").unwrap();
        mount(&outside.join("canary.txt"), &file_mount);
        assert_eq!(
            std::fs::metadata(&enrolled).unwrap().dev(),
            std::fs::metadata(&nested).unwrap().dev(),
            "This must exercise same-filesystem mounts"
        );
        let root = SourceRoot::capture(&enrolled).unwrap();
        let Reply::Directory(directory) = execute(Request::List {
            output: None,
            root: root.clone(),
            path: enrolled.clone(),
            hidden: false,
            exclusions: vec![],
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            directory.skipped, 0,
            "Excluded mounts are not failed enumeration"
        );
        assert_eq!(
            directory
                .entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["local.txt"]
        );
        let entry = filesystem::read_directory(&nested, &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        assert!(
            execute(Request::Extract {
                root: root.clone(),
                entry: entry.clone(),
                cursor: 0
            })
            .is_err()
        );
        assert!(
            execute(Request::Verify {
                root: root.clone(),
                entry: entry.clone()
            })
            .is_err()
        );
        assert!(
            execute(Request::Absent {
                root: root.clone(),
                path: nested.join("missing.txt")
            })
            .is_err()
        );
        assert!(
            execute(Request::Absent {
                root: root.clone(),
                path: enrolled.join("missing.txt")
            })
            .is_ok()
        );
        // An explicitly enrolled nested mount is allowed to index its own content.
        let mounted_file = filesystem::read_directory(&enrolled, &AtomicBool::new(false))
            .unwrap()
            .entries
            .into_iter()
            .find(|entry| entry.path == file_mount)
            .unwrap();
        assert!(
            execute(Request::Extract {
                root: root.clone(),
                entry: mounted_file,
                cursor: 0
            })
            .is_err()
        );
        assert!(
            execute(Request::Absent {
                root: root.clone(),
                path: enrolled.join("../outside/missing.txt")
            })
            .is_err()
        );
        // Watch admission uses the same scoped resolution, before registration.
        let descriptor = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        assert!(descriptor >= 0);
        let watch = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(descriptor) };
        assert!(attach_watch(&root, &nested, watch.as_raw_fd()).is_err());
        assert!(attach_watch(&root, &enrolled, watch.as_raw_fd()).is_ok());
        let chosen = SourceRoot::capture(&nested).unwrap();
        // Same backing inode through two independently mounted source views
        // must not overwrite one another's file record or source context.
        let cache = fixture.join("scope-cache");
        let mut store = super::super::store::Store::open(&cache).unwrap();
        let mut files = Vec::new();
        for origin in [&outside, &nested] {
            let source = SourceRoot::capture(origin).unwrap();
            let root_id = store.enroll_source(&source).unwrap();
            let generation = store.begin_scan(root_id).unwrap();
            let entry = filesystem::read_directory(origin, &AtomicBool::new(false))
                .unwrap()
                .entries
                .into_iter()
                .find(|entry| entry.name == "canary.txt")
                .unwrap();
            let (file, changed) = store
                .observe(
                    root_id,
                    generation,
                    &entry,
                    &std::fs::metadata(&entry.path).unwrap(),
                    "model",
                )
                .unwrap();
            assert!(changed);
            files.push((file, entry.identity.unwrap().object()));
        }
        assert_eq!(files[0].1, files[1].1);
        assert_ne!(files[0].0, files[1].0);
        let Reply::Prepared(prepared) = execute(Request::Extract {
            root: chosen,
            entry,
            cursor: 0,
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(prepared.text, "must not enter enclosing root");
        // Cross out and back into this filesystem: final device IDs are insufficient.
        let returned = nested.join("returned");
        std::fs::create_dir(&returned).unwrap();
        mount(&enrolled, &returned);
        assert!(
            execute(Request::List {
                output: None,
                root: root.clone(),
                path: returned.clone(),
                hidden: false,
                exclusions: vec![]
            })
            .is_err()
        );
        assert!(
            execute(Request::Absent {
                root,
                path: returned.join("missing.txt")
            })
            .is_err()
        );
        // A held root must reject a replacement bind mount of its very same inode.
        let held = Root::open(&enrolled).unwrap();
        let original = SourceRoot::capture(&enrolled).unwrap();
        mount(&enrolled, &enrolled);
        let now = Root::open(&enrolled).unwrap();
        assert_eq!((held.device, held.inode), (now.device, now.inode));
        assert_ne!(held.mount, now.mount);
        assert!(held.revalidate().is_err());
        assert!(
            original.validate().is_err(),
            "A new request must retain the captured mount too"
        );
        // The parent runner cleans its fixture after this namespace exits, so
        // recursive cleanup never encounters the deliberate private bind loop.
    }

    #[test]
    fn enumeration_excludes_aliases_secrets_and_generated_subtrees() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("notes.txt"), "hello").unwrap();
        std::fs::write(folder.path().join("private.key"), "secret").unwrap();
        std::fs::create_dir(folder.path().join(".git")).unwrap();
        std::os::unix::fs::symlink("notes.txt", folder.path().join("alias.txt")).unwrap();
        let root = SourceRoot::capture(folder.path()).unwrap();
        let Reply::Directory(dir) = execute(Request::List {
            output: None,
            path: root.path.clone(),
            root,
            hidden: true,
            exclusions: Vec::new(),
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(dir.entries.len(), 1);
        assert_eq!(dir.entries[0].name, "notes.txt");
    }
    #[test]
    fn inventory_exclusions_skip_exact_files_and_subtrees_without_hiding_siblings() {
        let folder = tempfile::tempdir().unwrap();
        for name in ["kept.txt", "ignored.txt"] {
            std::fs::write(folder.path().join(name), "content").unwrap();
        }
        for name in ["private", "private-ish"] {
            std::fs::create_dir(folder.path().join(name)).unwrap();
        }
        let root = SourceRoot::capture(folder.path()).unwrap();
        let request = Request::List {
            output: None,
            root: root.clone(),
            path: root.path.clone(),
            hidden: true,
            exclusions: vec![
                folder.path().join("ignored.txt"),
                folder.path().join("private"),
            ],
        };
        let request = serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
        let Reply::Directory(directory) = execute(request).unwrap() else {
            panic!()
        };
        assert_eq!(
            directory
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["private-ish", "kept.txt"]
        );
        assert!(
            execute(Request::List {
                output: None,
                root: root.clone(),
                path: root.path.clone(),
                hidden: true,
                exclusions: vec![root.path.clone()],
            })
            .is_err()
        );
        assert_eq!(
            std::fs::read(folder.path().join("ignored.txt")).unwrap(),
            b"content"
        );
    }

    #[test]
    fn headers_preserve_valid_prefix_and_reject_replaced_or_aliased_sources() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("post.md");
        let mut original = b"---\ndate: 2021-11-18\n---\n".to_vec();
        original.extend_from_slice(&[0xff; 9000]);
        std::fs::write(&path, &original).unwrap();
        let (root, entry) = only_source(folder.path());
        let request = Request::Header { root, entry };
        let Reply::Header(header) = execute(request.clone()).unwrap() else {
            panic!()
        };
        assert_eq!(header, "---\ndate: 2021-11-18\n---\n");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::write(&path, "---\ndate: 2025-01-01\n---\n").unwrap();
        assert!(execute(request.clone()).is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/proc/version", &path).unwrap();
        assert!(execute(request).is_err());
    }

    #[test]
    fn text_segments_preserve_utf8_and_reject_replaced_sources() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.txt");
        let text = "é🦇 hello ".repeat(250);
        std::fs::write(&path, &text).unwrap();
        let (root, entry) = only_source(folder.path());
        let mut result = String::new();
        let mut cursor = 0;
        loop {
            let Reply::Prepared(part) = execute(Request::Extract {
                root: root.clone(),
                entry: entry.clone(),
                cursor,
            })
            .unwrap() else {
                panic!()
            };
            result.push_str(&part.text);
            match part.next {
                Some(next) => cursor = next,
                None => break,
            }
        }
        assert_eq!(result, text);
        std::fs::write(&path, "replacement").unwrap();
        assert!(execute(Request::Verify { root, entry }).is_err());
    }
    #[test]
    fn moved_parent_alias_cannot_reauthorize_an_unchanged_file() {
        let base = tempfile::tempdir().unwrap();
        let root_path = base.path().join("enrolled");
        let parent = root_path.join("documents");
        std::fs::create_dir_all(&parent).unwrap();
        let path = parent.join("notes.txt");
        std::fs::write(&path, "original content").unwrap();
        let root = SourceRoot::capture(&root_path).unwrap();
        let entry = filesystem::read_directory(&parent, &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        let moved = base.path().join("outside");
        std::fs::rename(&parent, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &parent).unwrap();
        assert_eq!(
            entry.identity,
            Some(filesystem::identity(&std::fs::metadata(&path).unwrap()))
        );
        assert!(
            execute(Request::Verify {
                root: root.clone(),
                entry: entry.clone()
            })
            .is_err()
        );
        assert!(
            execute(Request::Extract {
                root,
                entry,
                cursor: 0
            })
            .is_err()
        );
    }

    #[test]
    fn directory_navigation_accepts_content_changes_but_rejects_replacement() {
        let base = tempfile::tempdir().unwrap();
        let path = base.path().join("documents");
        std::fs::create_dir(&path).unwrap();
        let root = SourceRoot::capture(base.path()).unwrap();
        let entry = filesystem::read_directory(base.path(), &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        std::fs::write(path.join("new.txt"), "new content").unwrap();
        assert!(matches!(
            execute(Request::Verify {
                root: root.clone(),
                entry: entry.clone()
            }),
            Ok(Reply::Verified)
        ));
        std::fs::rename(&path, base.path().join("old")).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(execute(Request::Verify { root, entry }).is_err());
    }

    #[test]
    fn unsupported_large_files_and_budget_limits_remain_explicit_name_only_results() {
        let folder = tempfile::tempdir().unwrap();
        for (name, size) in [
            ("large.stl", 600u64 * 1024 * 1024),
            ("large.pdf", 513 * 1024 * 1024),
            ("large.txt", 65 * 1024 * 1024),
            ("large.png", 257 * 1024 * 1024),
            ("large.wav", 65 * 1024 * 1024 * 1024),
        ] {
            std::fs::File::create(folder.path().join(name))
                .unwrap()
                .set_len(size)
                .unwrap();
        }
        let root = SourceRoot::capture(folder.path()).unwrap();
        let directory = filesystem::read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        for entry in directory.entries {
            let Reply::Prepared(prepared) = execute(Request::Extract {
                root: root.clone(),
                entry,
                cursor: 0,
            })
            .unwrap() else {
                panic!()
            };
            assert!(prepared.coverage.starts_with("name-only:"));
            assert!(prepared.text.is_empty() && prepared.data.is_none() && prepared.next.is_none());
        }
    }
    #[test]
    fn ordinary_source_code_uses_the_shared_text_classification_without_execution() {
        let folder = tempfile::tempdir().unwrap();
        let source = "pub fn violet_lighthouse() { println!(\"hello\"); }\n";
        std::fs::write(folder.path().join("example.rs"), source).unwrap();
        let root = SourceRoot::capture(folder.path()).unwrap();
        let directory = filesystem::read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let Reply::Prepared(prepared) = execute(Request::Extract {
            root,
            entry: directory.entries[0].clone(),
            cursor: 0,
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(prepared.text, source);
        assert_eq!(prepared.coverage, "complete");
    }
    #[test]
    fn guarded_image_and_audio_preparation_produce_bounded_model_inputs() {
        let (folder, _) = crate::audio_preview::tests::generate_fixture_wav("sample", 0.1);
        image::RgbImage::from_pixel(80, 40, image::Rgb([20, 100, 220]))
            .save(folder.path().join("photo.png"))
            .unwrap();
        let root = SourceRoot::capture(folder.path()).unwrap();
        let directory = filesystem::read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        for entry in directory.entries {
            let Reply::Prepared(prepared) = execute(Request::Extract {
                root: root.clone(),
                entry,
                cursor: 0,
            })
            .unwrap() else {
                panic!()
            };
            let bytes = STANDARD.decode(prepared.data.unwrap()).unwrap();
            assert!(prepared.next.is_none());
            assert_eq!(prepared.coverage, "complete");
            match prepared.modality.as_str() {
                "image" => {
                    let image = image::load_from_memory(&bytes).unwrap();
                    assert_eq!((image.width(), image.height()), (80, 40));
                }
                "audio" => {
                    assert_eq!(&bytes[..4], b"RIFF");
                    assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
                    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16000);
                    assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
                    assert!(bytes.len() > 44 && bytes.len() < 16000);
                }
                kind => panic!("Unexpected modality {kind}"),
            }
        }
    }
    #[test]
    fn disguised_audio_playlist_cannot_open_a_network_resource() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("sample.mp3"), format!("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1,\nhttp://{}/audio.ts\n#EXT-X-ENDLIST\n", listener.local_addr().unwrap())).unwrap();
        let root = SourceRoot::capture(folder.path()).unwrap();
        let directory = filesystem::read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        assert!(
            execute(Request::Extract {
                root,
                entry: directory.entries[0].clone(),
                cursor: 0
            })
            .is_err()
        );
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
    #[test]
    fn image_hash_does_not_require_enrollment_and_rejects_obsolete_cached_values() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("image.png");
        image::RgbImage::from_pixel(16, 16, image::Rgb([40, 80, 120]))
            .save(&path)
            .unwrap();
        let directory = filesystem::read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let entry = directory.entries[0].clone();
        let Reply::ImageHash(hash) = execute(Request::ImageHash {
            entry: entry.clone(),
            cached: None,
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            hash, 0,
            "A solid image has no horizontal luminance transitions"
        );
        std::fs::write(&path, "replacement").unwrap();
        assert!(
            execute(Request::ImageHash {
                entry,
                cached: Some(hash)
            })
            .is_err()
        );
    }
}
