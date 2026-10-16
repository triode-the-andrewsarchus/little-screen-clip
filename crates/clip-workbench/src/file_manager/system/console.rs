//! Explicit, single-shot VCS reads; passive previews never invoke this module's worker.
use super::{
    Document,
    identity::{Namespace, ObjectContext},
    worker::{self, Budget},
};
use crate::file_manager::preview_job::Task;
use little_clip_core::{
    BackgroundJobs,
    kernel_interface::{self, Kind},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const MAX_CELLS: usize = 16_384;
static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 1));

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
enum Format {
    Glyphs,
    Unicode,
    Attributes,
}
fn format(kind: Kind) -> Option<Format> {
    match kind {
        Kind::Character {
            major: 7,
            minor: 0..=63,
        } => Some(Format::Glyphs),
        Kind::Character {
            major: 7,
            minor: 64..=127,
        } => Some(Format::Unicode),
        Kind::Character {
            major: 7,
            minor: 128..=191,
        } => Some(Format::Attributes),
        _ => None,
    }
}
pub(super) fn describe(document: &mut Document) {
    let Some(format) = format(document.kind) else {
        return;
    };
    let Kind::Character { minor, .. } = document.kind else {
        return;
    };
    let vt = minor & 63;
    let name = match format {
        Format::Glyphs => "Glyph screen data",
        Format::Unicode => "Unicode screen data",
        Format::Attributes => "Glyph/attribute screen data",
    };
    document.title = format!(
        "{name} · {}",
        if vt == 0 {
            "active virtual console".into()
        } else {
            format!("VT {vt}")
        }
    );
    for (label, value) in [
        (
            "Console screen interface",
            format!(
                "{name} · {}",
                if vt == 0 {
                    "active virtual console".into()
                } else {
                    format!("VT {vt}")
                }
            ),
        ),
        (
            "Console screen access",
            "Unverified; capture requires an explicit request".into(),
        ),
    ] {
        let mut fact = super::Fact::text(label, value, document.path.clone());
        fact.interface = true;
        document.facts.push(fact);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    #[serde(with = "super::path_serde")]
    path: PathBuf,
    kind: Kind,
    identity: Vec<u64>,
    context: Option<ObjectContext>,
    namespace: Namespace,
}
impl Request {
    fn from(document: &Document) -> Self {
        Self {
            path: document.path.clone(),
            kind: document.kind,
            identity: document.identity.clone(),
            context: document.interface_context,
            namespace: document.namespace.clone(),
        }
    }
    fn matches(&self, document: &Document) -> bool {
        self.path == document.path
            && self.kind == document.kind
            && self.identity == document.identity
            && self.context == document.interface_context
            && self.namespace == document.namespace
    }
    fn verify(&self, file: &std::fs::File) -> Result<(), String> {
        if !self.namespace.known()
            || self.context.is_none()
            || self.namespace != Namespace::current()
            || kernel_interface::classify_handle(file).map_err(|e| e.to_string())? != self.kind
            || ObjectContext::pinned(file).ok() != self.context
            || super::identity::interface_fingerprint(
                self.kind,
                &file.metadata().map_err(|e| e.to_string())?,
            ) != self.identity
        {
            return Err(
                "Console interface changed or its context is unavailable; refresh the inspector"
                    .into(),
            );
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    text: String,
    dimensions: Option<(u8, u8)>,
    cursor: Option<(u8, u8)>,
    attributes: Vec<u8>,
    truncated: bool,
    description: String,
    captured_ms: u64,
}
fn glyph(byte: u8) -> char {
    if (32..=126).contains(&byte) {
        byte as char
    } else {
        '\u{fffd}'
    }
}
fn decode(bytes: &[u8], format: Format) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot {
        text: String::new(),
        dimensions: None,
        cursor: None,
        attributes: Vec::new(),
        truncated: false,
        description: String::new(),
        captured_ms: super::model::now_ms(),
    };
    let characters = match format {
        Format::Glyphs => {
            snapshot.description = "Glyph codes shown as an ASCII approximation; loaded console-font mapping is unavailable. Screen geometry is not exposed by this interface.".into();
            bytes.iter().copied().map(glyph).collect::<Vec<_>>()
        }
        Format::Unicode => {
            if bytes.len() % 4 != 0 {
                return Err("Incomplete native-endian Unicode console cell".into());
            }
            snapshot.description =
                "Unicode cells; screen geometry is not exposed by this interface.".into();
            bytes
                .chunks_exact(4)
                .map(|cell| {
                    char::from_u32(u32::from_ne_bytes(cell.try_into().unwrap()))
                        .filter(|c| !c.is_control())
                        .unwrap_or('\u{fffd}')
                })
                .collect()
        }
        Format::Attributes => attribute_cells(bytes, &mut snapshot)?,
    };
    snapshot.truncated = characters.len() > MAX_CELLS;
    let count = characters.len().min(MAX_CELLS);
    snapshot.attributes.truncate(count);
    if let Some((rows, columns)) = snapshot.dimensions {
        let expected = rows as usize * columns as usize;
        if count < expected {
            snapshot.truncated = true;
        }
        if characters.len() > expected {
            snapshot.dimensions = None;
            snapshot.cursor = None;
            snapshot.description.push_str(
                " Screen size changed or its header is incomplete; layout is a character stream.",
            );
        }
    }
    for (index, character) in characters.into_iter().take(count).enumerate() {
        if snapshot
            .dimensions
            .is_some_and(|(_, cols)| index > 0 && index % cols as usize == 0)
        {
            snapshot.text.push('\n');
        }
        snapshot.text.push(character);
    }
    Ok(snapshot)
}
fn attribute_cells(bytes: &[u8], snapshot: &mut Snapshot) -> Result<Vec<char>, String> {
    if bytes.len() < 4 || (bytes.len() - 4) % 2 != 0 {
        return Err("Incomplete console header or attribute cell".into());
    }
    let (rows, columns) = (bytes[0], bytes[1]);
    if rows == 0 || columns == 0 {
        return Err("Invalid zero-sized console header".into());
    }
    snapshot.description = "Native-endian glyph/attribute cells; glyphs use an ASCII approximation, not the loaded console font or palette.".into();
    if rows < 255 && columns < 255 {
        snapshot.dimensions = Some((rows, columns));
        if bytes[2] < columns && bytes[3] < rows {
            snapshot.cursor = Some((bytes[2], bytes[3]));
        }
    } else {
        snapshot
            .description
            .push_str(" Header dimensions may be clamped; layout is shown as a character stream.");
    }
    let mut chars = Vec::new();
    for cell in bytes[4..].chunks_exact(2) {
        let word = u16::from_ne_bytes(cell.try_into().unwrap());
        chars.push(glyph(word as u8));
        snapshot.attributes.push((word >> 8) as u8);
    }
    Ok(chars)
}

fn capture(request: &Request) -> Result<Snapshot, String> {
    let format = format(request.kind).ok_or("Not a supported VCS interface")?;
    let pinned = kernel_interface::pin(&request.path, true).map_err(|e| e.to_string())?;
    request.verify(&pinned)?;
    let Kind::Character { minor, .. } = request.kind else {
        return Err("Not a console node".into());
    };
    let object = PathBuf::from(format!("/sys/dev/char/7:{minor}"));
    let class = PathBuf::from("/sys/class/vc")
        .canonicalize()
        .map_err(|_| "Virtual-console class unavailable")?;
    if kernel_interface::classify(&object).ok()
        != Some(Kind::Kernel {
            filesystem: kernel_interface::Filesystem::Sysfs,
        })
        || object.join("subsystem").canonicalize().ok() != Some(class)
    {
        return Err("Registered virtual-console interface is unavailable".into());
    }
    let limit = match format {
        Format::Glyphs => MAX_CELLS + 1,
        Format::Unicode => (MAX_CELLS + 1) * 4,
        Format::Attributes => (MAX_CELLS + 1) * 2 + 4,
    };
    // This is the sole explicit device-opening exception: only verified major-7 VCS modes.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                "Screen contents restricted".into()
            } else {
                format!("Console snapshot unavailable: {error}")
            }
        })?;
    request.verify(&file)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Console snapshot read unavailable: {e}"))?;
    request.verify(&file)?;
    let current = kernel_interface::pin(&request.path, true).map_err(|e| e.to_string())?;
    request.verify(&current)?;
    decode(&bytes, format)
}
pub(crate) fn run(json: &str) -> Result<(), String> {
    if json.len() > 32 * 1024 {
        return Err("Console request exceeds metadata budget".into());
    }
    let request: Request = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let result = capture(&request);
    let bytes = serde_json::to_vec(&result).map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("Console response exceeds memory budget".into());
    }
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|e| e.to_string())
}
fn collect(request: Request, token: &AtomicBool) -> Result<Snapshot, String> {
    if token.load(Ordering::Relaxed) {
        return Err("Console capture cancelled".into());
    }
    let json = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    if json.len() > 32 * 1024 {
        return Err("Console request exceeds metadata budget".into());
    }
    let mut command = Command::new("/proc/self/exe");
    command
        .args(["--internal-system-console-snapshot", &json])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let id = worker::spawn_in(Budget::Optional, command)?;
    let bytes = worker::wait_in(Budget::Optional, id, token)?;
    if token.load(Ordering::Relaxed) {
        return Err("Console capture cancelled".into());
    }
    let current = kernel_interface::pin(&request.path, true).map_err(|e| e.to_string())?;
    request.verify(&current)?;
    serde_json::from_slice::<Result<Snapshot, String>>(&bytes).map_err(|e| e.to_string())?
}
#[derive(Default)]
pub(super) struct Capture {
    pending: Option<(Request, Task<Result<Snapshot, String>>)>,
    snapshot: Option<Snapshot>,
    error: Option<String>,
}
impl Capture {
    pub fn show(&mut self, ui: &mut egui::Ui, document: &Document) {
        if format(document.kind).is_none() {
            return;
        }
        if let Some((request, task)) = &self.pending {
            if let Some(result) = task.poll() {
                let matches = request.matches(document);
                self.pending = None;
                self.snapshot = None;
                match result {
                    Ok(Ok(snapshot)) if matches => {
                        self.snapshot = Some(snapshot);
                        self.error = None;
                    }
                    Ok(Ok(_)) => self.error = Some("Console capture superseded".into()),
                    Ok(Err(error)) => self.error = Some(error),
                    Err(error) => {
                        self.error = Some(format!("Console capture unavailable: {error:?}"))
                    }
                }
            }
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new("Capture console snapshot"),
                )
                .clicked()
            {
                let request = Request::from(document);
                let worker = request.clone();
                self.error = None;
                self.pending = Some((
                    request,
                    Task::start(&JOBS, Duration::from_secs(3), move |token| {
                        collect(worker, token)
                    })
                    .with_response_deadline(super::RESPONSE_DEADLINE),
                ));
            }
            if self.pending.is_some() {
                ui.spinner();
                ui.ctx().request_repaint_after(Duration::from_millis(40));
            }
        });
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if let Some(snapshot) = &self.snapshot {
            ui.label(format!("Session snapshot · captured {} ms ago · current viewed screen, not scrollback · read may span updates", super::model::now_ms().saturating_sub(snapshot.captured_ms)));
            ui.label(&snapshot.description);
            if let Some((rows, columns)) = snapshot.dimensions {
                ui.label(format!("{columns} columns × {rows} rows"));
                if let Some((x, y)) = snapshot.cursor {
                    ui.label(format!("Cursor column {x}, row {y} (zero-based)"));
                }
            }
            if snapshot.truncated {
                ui.label("Partial snapshot: at most 16,384 cells retained");
            }
            egui::ScrollArea::both().max_height(280.0).show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&snapshot.text).monospace())
                        .wrap(snapshot.dimensions.is_none()),
                );
            });
        }
    }
}

#[cfg(test)]
fn console_fixture() -> Document {
    let mut document = worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
    document.kind = Kind::Character {
        major: 7,
        minor: 129,
    };
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_documented_vcs_minor_ranges_are_eligible() {
        for (minor, expected) in [
            (0, Format::Glyphs),
            (63, Format::Glyphs),
            (64, Format::Unicode),
            (127, Format::Unicode),
            (128, Format::Attributes),
            (191, Format::Attributes),
        ] {
            assert_eq!(format(Kind::Character { major: 7, minor }), Some(expected));
        }
        for kind in [
            Kind::Content,
            Kind::Character {
                major: 7,
                minor: 192,
            },
            Kind::Character {
                major: 7,
                minor: 255,
            },
            Kind::Character { major: 4, minor: 1 },
        ] {
            assert_eq!(format(kind), None);
        }
    }
    #[test]
    fn attribute_snapshot_preserves_reported_grid_cursor_and_native_endian_cells() {
        let mut bytes = vec![2, 3, 1, 1];
        for byte in b"abcdef" {
            bytes.extend_from_slice(&(u16::from(*byte) | 0x1f00).to_ne_bytes());
        }
        let snapshot = decode(&bytes, Format::Attributes).unwrap();
        assert_eq!(snapshot.dimensions, Some((2, 3)));
        assert_eq!(snapshot.cursor, Some((1, 1)));
        assert_eq!(snapshot.text, "abc\ndef");
        assert_eq!(snapshot.attributes, vec![31; 6]);
        assert!(!snapshot.truncated);
    }
    #[test]
    fn malformed_or_clamped_geometry_does_not_invent_a_grid() {
        for bytes in [
            &[][..],
            &[1, 2, 0][..],
            &[0, 2, 0, 0][..],
            &[1, 2, 0, 0, 65][..],
        ] {
            assert!(decode(bytes, Format::Attributes).is_err());
        }
        let mut bytes = vec![255, 255, 0, 0];
        bytes.extend_from_slice(&65_u16.to_ne_bytes());
        let snapshot = decode(&bytes, Format::Attributes).unwrap();
        assert_eq!(snapshot.dimensions, None);
        assert!(snapshot.description.contains("clamped"));
    }
    #[test]
    fn unicode_and_glyph_streams_do_not_guess_geometry_or_interpret_controls() {
        let bytes = [65_u32, 0x1f600, 0xd800, 10]
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect::<Vec<_>>();
        let snapshot = decode(&bytes, Format::Unicode).unwrap();
        assert_eq!(snapshot.text, "A😀��");
        assert_eq!(snapshot.dimensions, None);
        assert!(decode(&bytes[..3], Format::Unicode).is_err());
        assert_eq!(decode(b"A\x1b\xff", Format::Glyphs).unwrap().text, "A��");
    }
    #[test]
    fn oversized_and_incomplete_screens_are_explicitly_partial() {
        let snapshot = decode(&vec![65; MAX_CELLS + 1], Format::Glyphs).unwrap();
        assert!(snapshot.truncated);
        assert_eq!(snapshot.text.len(), MAX_CELLS);
        let mut bytes = vec![2, 2, 0, 0];
        bytes.extend_from_slice(&65_u16.to_ne_bytes());
        assert!(decode(&bytes, Format::Attributes).unwrap().truncated);
    }
    #[test]
    fn ordinary_file_cannot_be_opened_as_a_console_by_forging_the_request_kind() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("vcsa1");
        std::fs::write(&path, "ordinary fixture").unwrap();
        let document = worker::interface(&path).unwrap();
        let mut request = Request::from(&document);
        request.kind = Kind::Character {
            major: 7,
            minor: 129,
        };
        assert!(capture(&request).unwrap_err().contains("changed"));
        request.context = None;
        assert!(capture(&request).is_err());
    }
    #[test]
    fn passive_vcs_metadata_describes_vt_without_reading_content() {
        let mut document = console_fixture();
        describe(&mut document);
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Console screen interface"
                    && f.value.as_ref().unwrap().contains("VT 1"))
        );
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Console screen access"
                    && f.value.as_ref().unwrap().contains("Unverified"))
        );
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    #[test]
    fn synthetic_session_snapshot_renders_without_starting_capture_or_exporting() {
        let document = console_fixture();
        let mut bytes = vec![1, 3, 0, 0];
        for byte in b"abc" {
            bytes.extend_from_slice(&u16::from(*byte).to_ne_bytes());
        }
        let mut capture = Capture {
            snapshot: Some(decode(&bytes, Format::Attributes).unwrap()),
            ..Default::default()
        };
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| capture.show(ui, &document));
        });
        assert!(capture.pending.is_none());
        assert!(output.platform_output.copied_text.is_empty());
        assert!(!output.shapes.is_empty());
        assert_eq!(capture.snapshot.as_ref().unwrap().text, "abc");
    }
}
