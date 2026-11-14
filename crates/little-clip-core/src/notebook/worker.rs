//! Disposable notebook readers sharing the ordinary-file and command boundaries.
use crate::{CommandRequest, file_source::ReadCheck};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const FLAG: &str = "--internal-notebook-worker";
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// Native page dimensions, in authored order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    pub width: u32,
    pub height: u32,
}

/// Read-only notebook summary without decoded ink or source-specific offsets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub format: String,
    pub pages: Vec<PageSize>,
}

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
            page.width == 0 || page.height == 0 || page.width > 16384 || page.height > 16384
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
        .max_output_bytes(if mode == "info" {
            256 * 1024
        } else {
            MAX_OUTPUT
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
    let mut header = [0; 24];
    input
        .read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    input
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    if !super::supernote::has_signature(&header) {
        return Err("Unrecognized notebook header".into());
    }
    match mode {
        "info" if page == 0 && edge == 0 => {
            let document = super::supernote::inspect(input)?;
            let info = Info {
                format: "Supernote".into(),
                pages: document
                    .pages
                    .into_iter()
                    .map(|page| PageSize {
                        width: page.width,
                        height: page.height,
                    })
                    .collect(),
            };
            serde_json::to_vec(&info).map_err(|error| error.to_string())
        }
        "page" if (1..=4096).contains(&page) && (1..=4096).contains(&edge) => {
            super::supernote::render(input, page, edge, &AtomicBool::new(false))
                .map(|result| result.0)
        }
        _ => Err("Invalid notebook helper operation".into()),
    }
}
