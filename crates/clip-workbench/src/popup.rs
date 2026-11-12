//! Screen-relative sizing for the fixed-size workbench popup.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Vec2, ViewportBuilder};
use serde::Deserialize;

const SCREEN_FRACTION: f32 = 0.92;
const FALLBACK_SIZE: Vec2 = Vec2::new(960.0, 540.0);

#[derive(Deserialize)]
struct Output {
    logical: Option<LogicalOutput>,
}

#[derive(Deserialize)]
struct LogicalOutput {
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct NiriWindow {
    id: u64,
    app_id: Option<String>,
    pid: Option<u32>,
    layout: Option<NiriWindowLayout>,
}

#[derive(Deserialize)]
struct NiriWindowLayout {
    window_size: [u32; 2],
}

fn instance_lock_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "little-clip-{}",
                rustix::process::geteuid().as_raw()
            ))
        })
        .join("clip-workbench.lock")
}

/// Hold the file for the lifetime of the popup. A second invocation raises it.
pub(crate) fn claim_instance() -> Option<File> {
    let path = instance_lock_path();
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        eprintln!("Cannot create workbench runtime directory: {err}");
        return None;
    }
    let file = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(err) => {
            eprintln!("Cannot open workbench instance lock: {err}");
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            focus_existing();
            None
        }
        Err(err) => {
            eprintln!("Cannot lock workbench instance: {err}");
            None
        }
    }
}

fn window_id(
    windows: Vec<NiriWindow>,
    pid: Option<u32>,
    expected_size: Option<Vec2>,
) -> Option<u64> {
    windows
        .into_iter()
        .find(|window| {
            window.app_id.as_deref() == Some("little-clip-workbench")
                && pid.is_none_or(|pid| window.pid == Some(pid))
                && expected_size.is_none_or(|size| {
                    window.layout.as_ref().is_some_and(|layout| {
                        (layout.window_size[0] as f32 - size.x).abs() <= 2.0
                            && (layout.window_size[1] as f32 - size.y).abs() <= 2.0
                    })
                })
        })
        .map(|window| window.id)
}

#[derive(Deserialize)]
enum NiriResponse {
    Windows(Vec<NiriWindow>),
    FocusedOutput(Option<Output>),
    Handled,
}

fn niri_request(request: &serde_json::Value, limit: usize) -> Result<NiriResponse, String> {
    let deadline = Instant::now() + Duration::from_millis(250);
    let path = std::env::var_os("NIRI_SOCKET").ok_or("Niri IPC is unavailable")?;
    let address =
        rustix::net::SocketAddrUnix::new(PathBuf::from(path)).map_err(|error| error.to_string())?;
    // A full Unix-socket backlog must fail promptly rather than block popup
    // creation. No subprocess or visible probe window is needed.
    let socket = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC | rustix::net::SocketFlags::NONBLOCK,
        None,
    )
    .map_err(|error| error.to_string())?;
    rustix::net::connect(&socket, &address).map_err(|error| error.to_string())?;
    let stream = UnixStream::from(socket);
    stream
        .set_nonblocking(false)
        .map_err(|error| error.to_string())?;
    niri_exchange(stream, request, limit, deadline)
}

fn niri_exchange(
    mut stream: UnixStream,
    request: &serde_json::Value,
    limit: usize,
    deadline: Instant,
) -> Result<NiriResponse, String> {
    let remaining = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or("Niri IPC timed out".to_owned())
    };
    stream
        .set_write_timeout(Some(remaining()?))
        .map_err(|error| error.to_string())?;
    let mut encoded = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|error| error.to_string())?;
    let mut reply = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        stream
            .set_read_timeout(Some(remaining()?))
            .map_err(|error| error.to_string())?;
        // Bound memory even if a peer never terminates its reply line.
        let capacity = buffer.len().min(limit.saturating_sub(reply.len()));
        if capacity == 0 {
            return Err("Niri IPC response exceeds its size limit".into());
        }
        let count = match stream.read(&mut buffer[..capacity]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(|error| error.to_string())?,
        };
        if count == 0 {
            return Err("Niri IPC disconnected before completing its response".into());
        }
        if let Some(end) = buffer[..count].iter().position(|byte| *byte == b'\n') {
            reply.extend_from_slice(&buffer[..end]);
            let reply: Result<NiriResponse, String> =
                serde_json::from_slice(&reply).map_err(|error| error.to_string())?;
            return reply;
        }
        reply.extend_from_slice(&buffer[..count]);
    }
}

fn niri_window_id(pid: Option<u32>, expected_size: Option<Vec2>) -> Option<u64> {
    let NiriResponse::Windows(windows) =
        niri_request(&serde_json::json!("Windows"), 1024 * 1024).ok()?
    else {
        return None;
    };
    window_id(windows, pid, expected_size)
}

fn niri_action(action: &str, id: u64) {
    let request = serde_json::json!({"Action": {action: {"id": id}}});
    match niri_request(&request, 4096) {
        Ok(NiriResponse::Handled) => {}
        Ok(_) => tracing::warn!(%action, "Unexpected Niri popup action response"),
        Err(error) => tracing::warn!(%error, %action, "Niri popup action failed"),
    }
}

fn wait_for_window(pid: Option<u32>, expected_size: Option<Vec2>) -> Option<u64> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Some(id) = niri_window_id(pid, expected_size) {
            return Some(id);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    None
}

fn focus_existing() {
    if std::env::var_os("NIRI_SOCKET").is_some()
        && let Some(id) = wait_for_window(None, None)
    {
        niri_action("FocusWindow", id);
    }
}

pub(crate) fn center_when_mapped() {
    if std::env::var_os("NIRI_SOCKET").is_some() {
        let expected_size = discover_size().unwrap_or(FALLBACK_SIZE);
        std::thread::spawn(move || {
            if let Some(id) = wait_for_window(Some(std::process::id()), Some(expected_size)) {
                niri_action("CenterWindow", id);
            }
        });
    }
}

fn size_from_output(output: Option<Output>) -> Result<Vec2, String> {
    let logical = output
        .and_then(|output| output.logical)
        .filter(|logical| logical.width > 0 && logical.height > 0)
        .ok_or_else(|| "no active logical output".to_owned())?;
    // Niri already accounts for scale and rotation in these dimensions.
    Ok(Vec2::new(
        (logical.width as f32 * SCREEN_FRACTION).floor().max(1.0),
        (logical.height as f32 * SCREEN_FRACTION).floor().max(1.0),
    ))
}

fn discover_size() -> Result<Vec2, String> {
    let NiriResponse::FocusedOutput(output) =
        niri_request(&serde_json::json!("FocusedOutput"), 64 * 1024)?
    else {
        return Err("Unexpected Niri output response".into());
    };
    size_from_output(output)
}

fn fixed_viewport(viewport: ViewportBuilder, size: Vec2) -> ViewportBuilder {
    viewport
        .with_inner_size(size)
        .with_min_inner_size(size)
        .with_max_inner_size(size)
        .with_resizable(false)
        .with_decorations(false)
        .with_maximized(false)
        .with_maximize_button(false)
}

pub(crate) fn window_builder(viewport: ViewportBuilder) -> ViewportBuilder {
    let size = discover_size().unwrap_or_else(|err| {
        tracing::warn!(%err, "Using fallback workbench popup size");
        FALLBACK_SIZE
    });
    tracing::info!(
        width = size.x,
        height = size.y,
        "Opening fixed workbench popup"
    );
    // This hook runs after eframe's initial size selection, before native creation.
    fixed_viewport(viewport, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ipc_frames_requests_and_decodes_split_replies() {
        use std::io::{BufRead, BufReader};
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&line).unwrap(),
                serde_json::json!({"Action": {"FocusWindow": {"id": 42}}})
            );
            server.write_all(br#"{"Ok":"Han"#).unwrap();
            server.write_all(b"dled\"}\n").unwrap();
        });
        assert!(matches!(
            niri_exchange(
                client,
                &serde_json::json!({"Action": {"FocusWindow": {"id": 42}}}),
                4096,
                Instant::now() + Duration::from_secs(1)
            ),
            Ok(NiriResponse::Handled)
        ));
        worker.join().unwrap();
    }

    #[test]
    fn native_ipc_reports_errors_bad_json_and_unterminated_or_oversized_replies() {
        for (reply, limit, expected) in [
            (b"{\"Err\":\"denied\"}\n".as_slice(), 4096, "denied"),
            (b"invalid\n".as_slice(), 4096, "expected"),
            (b"{\"Ok\":\"Handled\"}".as_slice(), 4096, "disconnected"),
            (b"xxxxxxxxxxxxxxxx".as_slice(), 8, "size limit"),
        ] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let worker = std::thread::spawn(move || {
                // Consume the request so closing the peer does not reset unread data.
                let mut request = [0; 64];
                let _ = server.read(&mut request);
                server.write_all(reply).unwrap();
            });
            let error = niri_exchange(
                client,
                &serde_json::json!("Windows"),
                limit,
                Instant::now() + Duration::from_secs(1),
            )
            .err()
            .unwrap();
            assert!(error.contains(expected), "{error}");
            worker.join().unwrap();
        }
    }

    #[test]
    fn native_ipc_has_a_total_deadline_even_when_bytes_keep_arriving() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            for _ in 0..50 {
                if server.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let started = Instant::now();
        assert!(
            niri_exchange(
                client,
                &serde_json::json!("Windows"),
                4096,
                started + Duration::from_millis(60)
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        worker.join().unwrap();
    }

    #[test]
    fn native_ipc_decodes_window_and_output_responses() {
        let windows: Result<NiriResponse, String> = serde_json::from_str(r#"{"Ok":{"Windows":[{"id":42,"app_id":"little-clip-workbench","pid":123,"layout":{"window_size":[800,600]}}]}}"#).unwrap();
        let NiriResponse::Windows(windows) = windows.unwrap() else {
            panic!("wrong response")
        };
        assert_eq!(
            window_id(windows, Some(123), Some(Vec2::new(800.0, 600.0))),
            Some(42)
        );
        let output: Result<NiriResponse, String> = serde_json::from_str(
            r#"{"Ok":{"FocusedOutput":{"logical":{"width":1280,"height":720}}}}"#,
        )
        .unwrap();
        let NiriResponse::FocusedOutput(output) = output.unwrap() else {
            panic!("wrong response")
        };
        assert_eq!(size_from_output(output).unwrap(), Vec2::new(1177.0, 662.0));
    }

    #[test]
    fn waits_for_scaled_window_to_reach_final_logical_size() {
        let expected = Vec2::new(2355.0, 1324.0);
        let startup = br#"[{"id":42,"app_id":"little-clip-workbench","pid":123,"layout":{"window_size":[4710,2648]}}]"#;
        let mapped = br#"[{"id":42,"app_id":"little-clip-workbench","pid":123,"layout":{"window_size":[2355,1324]}}]"#;
        assert_eq!(
            window_id(
                serde_json::from_slice(startup).unwrap(),
                Some(123),
                Some(expected)
            ),
            None
        );
        assert_eq!(
            window_id(
                serde_json::from_slice(mapped).unwrap(),
                Some(123),
                Some(expected)
            ),
            Some(42)
        );
        assert_eq!(
            window_id(
                serde_json::from_slice(mapped).unwrap(),
                Some(456),
                Some(expected)
            ),
            None
        );
    }

    #[test]
    fn uses_logical_dimensions_without_applying_output_scale_twice() {
        let size = size_from_output(
            serde_json::from_slice(br#"{"modes":[{"width":3840,"height":2160}],"logical":{"width":2560,"height":1440,"scale":1.5}}"#).unwrap(),
        )
        .unwrap();
        assert_eq!(size, Vec2::new(2355.0, 1324.0));
    }

    #[test]
    fn retains_output_proportions_and_fits_small_screens() {
        for (width, height) in [(1280, 720), (1920, 1200), (3440, 1440), (640, 480)] {
            let json = format!(r#"{{"logical":{{"width":{width},"height":{height}}}}}"#);
            let size = size_from_output(serde_json::from_str(&json).unwrap()).unwrap();
            assert!(size.x < width as f32 && size.y < height as f32);
            assert!((size.x / size.y - width as f32 / height as f32).abs() < 0.01);
        }
    }

    #[test]
    fn rejects_missing_disabled_and_invalid_outputs() {
        for json in [
            "null",
            r#"{"logical":null}"#,
            r#"{"logical":{"width":0,"height":1080}}"#,
            r#"{"logical":{"width":1920,"height":0}}"#,
            "invalid JSON",
        ] {
            assert!(
                serde_json::from_str(json)
                    .map_err(|error| error.to_string())
                    .and_then(size_from_output)
                    .is_err()
            );
        }
    }

    #[test]
    fn fixed_constraints_override_previous_geometry_before_opening() {
        let size = Vec2::new(1177.0, 662.0);
        let viewport = fixed_viewport(
            ViewportBuilder::default()
                .with_inner_size([3000.0, 2000.0])
                .with_resizable(true)
                .with_maximized(true),
            size,
        );
        assert_eq!(viewport.inner_size, Some(size));
        assert_eq!(viewport.min_inner_size, Some(size));
        assert_eq!(viewport.max_inner_size, Some(size));
        assert_eq!(viewport.resizable, Some(false));
        assert_eq!(viewport.decorations, Some(false));
        assert_eq!(viewport.maximized, Some(false));
        assert_eq!(viewport.maximize_button, Some(false));
    }
}
