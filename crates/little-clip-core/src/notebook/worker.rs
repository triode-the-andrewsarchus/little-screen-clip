//! Disposable notebook readers sharing the ordinary-file and command boundaries.
use super::{Info, PageSize};
use crate::{CommandRequest, file_source::ReadCheck};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const FLAG: &str = "--internal-notebook-worker";
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// Inspects an ordinary notebook on a bounded disposable worker.
///
/// # Errors
/// Returns source, cancellation, compatibility or worker-protocol failures.
pub fn inspect(path: &Path, cancel: &AtomicBool) -> Result<Info, String> {
    let bytes = request(path, "info", 0, 0, cancel)?;
    let info: Info = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if info.pages.is_empty()
        || info.pages.len() > 4096
        || info.pages.iter().any(|page| {
            page.width == 0 || page.height == 0 || page.width > 65536 || page.height > 65536
        })
    {
        return Err("Invalid notebook page dimensions or count".into());
    }
    Ok(info)
}

/// Renders one 1-based notebook page with both dimensions bounded by `edge`.
///
/// # Errors
/// Returns source, cancellation, page, compatibility or worker-protocol failures.
pub fn render_page(
    path: &Path,
    page: usize,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), String> {
    if page == 0 || page > 4096 || !(1..=4096).contains(&edge) {
        return Err("Notebook page or preview size is outside the supported range".into());
    }
    let png = request(path, "page", page, edge, cancel)?;
    let size = imagesize::blob_size(&png).map_err(|error| error.to_string())?;
    if !png.starts_with(b"\x89PNG\r\n\x1a\n")
        || size.width == 0
        || size.height == 0
        || size.width > edge as usize
        || size.height > edge as usize
    {
        return Err("Invalid notebook page worker response".into());
    }
    Ok((png, size.width as u32, size.height as u32))
}

pub(super) fn identify(path: &Path, cancel: &AtomicBool) -> Result<bool, String> {
    serde_json::from_slice(&request(path, "identify", 0, 0, cancel)?)
        .map_err(|error| format!("Invalid notebook identification response: {error}"))
}

fn request(
    path: &Path,
    mode: &str,
    page: usize,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Notebook preview cancelled".into());
    }
    let (input, source) = ReadCheck::default().open(path, None, cancel)?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let request = CommandRequest::new(executable.into_os_string())
        .arg(FLAG)
        .arg(mode)
        .arg(page.to_string())
        .arg(edge.to_string());
    let (request, descriptor) = request
        .inherit_handle(&input)
        .map_err(|error| error.to_string())?;
    let output = request
        .arg(descriptor.to_string())
        .timeout(Duration::from_secs(20))
        .resource_limit(rustix::process::Resource::As, 768 * 1024 * 1024)
        .resource_limit(rustix::process::Resource::Cpu, 15)
        .resource_limit(rustix::process::Resource::Fsize, MAX_OUTPUT as u64)
        .max_output_bytes(if mode == "page" {
            MAX_OUTPUT
        } else {
            256 * 1024
        })
        .own_process_group()
        .die_with_parent()
        .run_cancellable(cancel)
        .map_err(|error| error.to_string())?;
    source.check_file(&input)?;
    if cancel.load(Ordering::Acquire) {
        return Err("Notebook preview cancelled".into());
    }
    if !output.status.success() || output.stdout_truncated {
        return Err(format!(
            "Notebook worker failed: {}",
            output.stderr_lossy().chars().take(1024).collect::<String>()
        ));
    }
    Ok(output.stdout)
}

/// Dispatches a notebook helper before application or async-runtime startup.
///
/// Unrelated arguments return false. Helpers accept only inherited ordinary-file
/// descriptors, never filenames or resource URLs; failure exits with status 2.
pub fn run_worker_cli() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return false;
    }
    let args = args.collect::<Vec<_>>();
    if let Err(error) = run(&args) {
        eprintln!("Notebook preview: {error}");
        std::process::exit(2);
    }
    true
}

fn run(args: &[std::ffi::OsString]) -> Result<(), String> {
    let [mode, page, edge, descriptor] = args else {
        return Err("Invalid notebook helper arguments".into());
    };
    let number = |value: &std::ffi::OsString| {
        value
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or("Invalid notebook helper number")
    };
    let (page, edge, descriptor) = (number(page)?, number(edge)?, number(descriptor)?);
    let descriptor = i32::try_from(descriptor).map_err(|_| "Invalid inherited descriptor")?;
    let mut input = crate::process::duplicate_inherited_handle(descriptor)
        .map_err(|error| error.to_string())?;
    if !input
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
        || crate::kernel_interface::classify_handle(&input)
            .map_err(|error| error.to_string())?
            .protected()
    {
        return Err("Notebook input is not ordinary file content".into());
    }
    let output = prepare(&mut input, mode.to_str().unwrap_or(""), page as usize, edge)?;
    std::io::stdout()
        .lock()
        .write_all(&output)
        .map_err(|error| error.to_string())
}

fn prepare(input: &mut File, mode: &str, page: usize, edge: u32) -> Result<Vec<u8>, String> {
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut header = Vec::with_capacity(super::HEADER_BYTES);
    Read::by_ref(input)
        .take(super::HEADER_BYTES as u64)
        .read_to_end(&mut header)
        .map_err(|error| error.to_string())?;
    input.rewind().map_err(|error| error.to_string())?;
    let kind = source_kind(input, &header)?;
    match (mode, page, edge, kind) {
        ("identify", 0, 0, kind) => {
            serde_json::to_vec(&kind.is_some()).map_err(|error| error.to_string())
        }
        (_, _, _, None) => Err("Unrecognized notebook header".into()),
        ("info", 0, 0, Some(kind)) => {
            serde_json::to_vec(&prepare_info(input, kind)?).map_err(|error| error.to_string())
        }
        ("page", 1..=4096, 1..=4096, Some(kind)) => {
            prepare_page(input, kind, page, edge).map(|result| result.0)
        }
        _ => Err("Invalid notebook helper operation".into()),
    }
}

#[derive(Clone, Copy)]
enum SourceKind {
    Supernote,
    Xournal,
    Remarkable,
}

fn source_kind(input: &mut File, header: &[u8]) -> Result<Option<SourceKind>, String> {
    if super::supernote::has_signature(header) {
        return Ok(Some(SourceKind::Supernote));
    }
    if super::remarkable::has_signature(header) {
        return Ok(Some(SourceKind::Remarkable));
    }
    if header.starts_with(b"PK\x03\x04") {
        let has_xournal = {
            let archive = zip::ZipArchive::new(&mut *input).map_err(|error| error.to_string())?;
            archive.file_names().any(|name| name == "content.xml")
        };
        input.rewind().map_err(|error| error.to_string())?;
        if !has_xournal {
            return remarkable_package(input);
        }
    } else if header.get(257..262) == Some(b"ustar") {
        return remarkable_package(input);
    }
    if super::xournal::has_container_prefix(header)
        && super::xournal::has_signature(input, &AtomicBool::new(false))?
    {
        Ok(Some(SourceKind::Xournal))
    } else {
        Ok(None)
    }
}

fn remarkable_package(input: &mut File) -> Result<Option<SourceKind>, String> {
    super::remarkable::archive::has_signature(input, &AtomicBool::new(false))
        .map(|recognized| recognized.then_some(SourceKind::Remarkable))
}

fn prepare_info(input: &mut File, kind: SourceKind) -> Result<Info, String> {
    match kind {
        SourceKind::Supernote => {
            let document = super::supernote::inspect(input)?;
            Ok(Info {
                format: "Supernote".into(),
                warnings: Vec::new(),
                pages: document
                    .pages
                    .into_iter()
                    .map(|page| PageSize {
                        width: page.width,
                        height: page.height,
                    })
                    .collect(),
            })
        }
        SourceKind::Xournal => {
            let document = super::xournal::inspect(input, &AtomicBool::new(false))?;
            Ok(Info {
                format: "Xournal".into(),
                warnings: Vec::new(),
                pages: document
                    .pages
                    .into_iter()
                    .map(|page| PageSize {
                        width: page.width.ceil() as u32,
                        height: page.height.ceil() as u32,
                    })
                    .collect(),
            })
        }
        SourceKind::Remarkable => {
            super::remarkable::preview::inspect(input, &AtomicBool::new(false))
        }
    }
}

fn prepare_page(
    input: &mut File,
    kind: SourceKind,
    page: usize,
    edge: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let cancel = AtomicBool::new(false);
    match kind {
        SourceKind::Supernote => super::supernote::render(input, page, edge, &cancel),
        SourceKind::Remarkable => super::remarkable::preview::render(input, page, edge, &cancel),
        SourceKind::Xournal => {
            let source = crate::file_source::FileSource::capture_file(input)?;
            let result = super::xournal::render(input, source.path(), page, edge, &cancel)?;
            source.check_file(input)?;
            Ok(result)
        }
    }
}
