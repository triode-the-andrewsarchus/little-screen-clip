//! Guarded machine-embroidery decoding and stitch-order previews.

mod raster;

pub use raster::{Timeline, TimelineLayout};

use crate::{CommandRequest, file_source::read_file_snapshot_with_source};
use std::{
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMANDS: usize = 500_000;
const RECORD_BYTES: usize = 12;
const COLORS: [[u8; 3]; 6] = [
    [19, 123, 117],
    [181, 55, 84],
    [36, 70, 143],
    [146, 96, 16],
    [112, 60, 134],
    [40, 92, 55],
];
const BUNDLE: &[u8] = include_bytes!("../../../vendor/pystitch/pystitch-1.0.1-py3-none-any.whl");

/// Embroidery filename hints. Generic extensions such as DAT and CSV are
/// intentionally left to their regular file handlers.
pub fn supports_extension(extension: &str) -> bool {
    matches!(
        extension,
        "10o"
            | "dst"
            | "dsb"
            | "dsz"
            | "emd"
            | "exp"
            | "exy"
            | "fxy"
            | "hus"
            | "inb"
            | "jef"
            | "jef+"
            | "jpx"
            | "ksm"
            | "mit"
            | "pcq"
            | "pcs"
            | "pec"
            | "pes"
            | "phb"
            | "phc"
            | "pmv"
            | "ptn"
            | "sew"
            | "shv"
            | "spx"
            | "stc"
            | "stx"
            | "tap"
            | "tbf"
            | "u01"
            | "vip"
            | "vp3"
            | "xxx"
            | "zhs"
            | "zxy"
    )
}

enum Event {
    Sew([i32; 2], Option<[u8; 3]>),
    Break,
    ThreadChange,
}

#[derive(Default)]
struct StitchBuilder {
    stitches: Vec<Stitch>,
    previous: Option<[i32; 2]>,
    thread: usize,
}

impl StitchBuilder {
    fn add(&mut self, event: Event) {
        match event {
            Event::Sew(point, color) => {
                self.stitches.push(Stitch {
                    from: self.previous.unwrap_or(point),
                    to: point,
                    color: color.unwrap_or(COLORS[self.thread % COLORS.len()]),
                });
                self.previous = Some(point);
            }
            Event::Break => self.previous = None,
            Event::ThreadChange => {
                self.previous = None;
                self.thread += 1;
            }
        }
    }
}

#[derive(Debug)]
struct Stitch {
    from: [i32; 2],
    to: [i32; 2],
    color: [u8; 3],
}

/// The visible sewn geometry in order, with travel and trim gaps preserved.
#[derive(Debug)]
pub struct Design {
    stitches: Vec<Stitch>,
    min: [i32; 2],
    max: [i32; 2],
}

impl Design {
    /// Reads a bounded regular-file snapshot and decodes its stitch stream.
    pub fn read(path: &Path, cancelled: &AtomicBool) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|ext| supports_extension(ext))
            .ok_or("Unsupported embroidery format")?;
        check_cancelled(cancelled)?;
        let (bytes, source) =
            read_file_snapshot_with_source(path, MAX_BYTES).map_err(|error| error.to_string())?;
        check_cancelled(cancelled)?;
        let mut builder = StitchBuilder::default();
        if matches!(extension.as_str(), "vip" | "jef+")
            || (extension == "jef" && bytes.get(4..8) == Some(&1_u32.to_le_bytes()))
        {
            read_native(&extension, &bytes, &mut builder)?;
        } else {
            let reader = if extension == "ptn" {
                "jef"
            } else {
                &extension
            };
            read_bundled(reader, bytes, cancelled, &mut builder)?;
        }
        let stitches = builder.stitches;
        source.check()?;
        check_cancelled(cancelled)?;

        let first = stitches.first().ok_or("No stitches in design")?;
        let (mut min, mut max) = (first.from, first.from);
        for stitch in &stitches {
            for axis in 0..2 {
                min[axis] = min[axis].min(stitch.from[axis]).min(stitch.to[axis]);
                max[axis] = max[axis].max(stitch.from[axis]).max(stitch.to[axis]);
            }
        }
        Ok(Self { stitches, min, max })
    }

    /// Number of needle penetrations represented in the preview.
    pub fn stitch_count(&self) -> usize {
        self.stitches.len()
    }

    /// Size of visible stitch geometry in millimeters.
    pub fn size_mm(&self) -> [f32; 2] {
        std::array::from_fn(|axis| {
            (i64::from(self.max[axis]) - i64::from(self.min[axis])) as f32 / 10.0
        })
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Embroidery preview cancelled".into());
    }
    Ok(())
}

fn read_bundled(
    extension: &str,
    bytes: Vec<u8>,
    cancelled: &AtomicBool,
    builder: &mut StitchBuilder,
) -> Result<(), String> {
    let mut bundle = tempfile::tempfile().map_err(|error| error.to_string())?;
    bundle
        .write_all(BUNDLE)
        .map_err(|error| error.to_string())?;
    let command = CommandRequest::new("python3")
        .arg("-I")
        .arg("-B")
        .arg("-S")
        .arg("-c")
        .arg(include_str!("embroidery/decode.py"))
        .arg(extension)
        .arg_owned_content_file(bundle)
        .map_err(|error| error.to_string())?
        .arg(MAX_COMMANDS.to_string())
        .stdin_bytes(bytes)
        .resource_limit(rustix::process::Resource::As, 256 * 1024 * 1024)
        .resource_limit(rustix::process::Resource::Cpu, 3)
        .resource_limit(rustix::process::Resource::Fsize, 0)
        .timeout(Duration::from_secs(5))
        .max_output_bytes(MAX_COMMANDS * RECORD_BYTES)
        .own_process_group()
        .die_with_parent();
    let output = command
        .run_cancellable(cancelled)
        .map_err(|error| format!("Embroidery preview helper (Python 3): {error}"))?;
    if !output.status.success() || output.stdout_truncated {
        return Err(format!(
            "Could not decode {} stitches within the preview limits",
            extension.to_ascii_uppercase()
        ));
    }
    let (records, remainder) = output.stdout.as_chunks::<RECORD_BYTES>();
    if !remainder.is_empty() {
        return Err("Invalid embroidery preview data".into());
    }
    for record in records {
        let (xy, _) = record.as_chunks::<4>();
        let point = [i32::from_le_bytes(xy[0]), i32::from_le_bytes(xy[1])];
        let event = match record[11] {
            0 => Event::Sew(point, Some([record[8], record[9], record[10]])),
            1 => Event::Sew(point, None),
            2 => Event::Break,
            3 => Event::ThreadChange,
            _ => return Err("Invalid embroidery preview command".into()),
        };
        builder.add(event);
    }
    Ok(())
}

fn read_native(extension: &str, bytes: &[u8], builder: &mut StitchBuilder) -> Result<(), String> {
    use oxideav_embroidery::{Command, hus, jef};

    let design = if extension == "vip" {
        let file = hus::parse_vip(bytes).map_err(|error| error.to_string())?;
        if file.stitch_count as usize > MAX_COMMANDS {
            return Err("Embroidery stitch budget exceeded".into());
        }
        hus::decode_design(&file)
    } else {
        // Bound the command allocation even if the declared count is wrong.
        let offset = bytes
            .first_chunk::<4>()
            .map_or(0, |offset| u32::from_le_bytes(*offset));
        if !jef::probe(bytes) || bytes.len().saturating_sub(offset as usize) > MAX_COMMANDS * 2 {
            return Err("Invalid or oversized JEF+ stitch stream".into());
        }
        jef::decode(bytes).map(|file| file.design)
    }
    .map_err(|error| error.to_string())?;

    let (mut x, mut y) = (0i32, 0i32);
    for command in design.commands {
        let (dx, dy) = command.delta();
        x = x.saturating_add(dx);
        y = y.saturating_sub(dy);
        let event = match command {
            Command::Stitch { .. } => {
                let color = design
                    .threads
                    .get(builder.thread)
                    .and_then(|entry| entry.rgb);
                Event::Sew([x, y], color)
            }
            Command::ColorChange { .. } => Event::ThreadChange,
            Command::Jump { .. } | Command::Trim { .. } => Event::Break,
            Command::End => break,
            _ => continue,
        };
        builder.add(event);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_format::Format;

    #[test]
    fn machine_files_decode_in_stitch_order_and_keep_their_file_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/embroidery");
        let cancelled = AtomicBool::new(false);
        for extension in [
            "dst", "exp", "hus", "jef", "jef+", "pec", "pes", "ptn", "tbf", "u01", "vip", "vp3",
            "xxx",
        ] {
            let path = root.join(format!("Wildflower.{extension}"));
            assert_eq!(Format::for_path(&path), Some(Format::Embroidery));
            let design = Design::read(&path, &cancelled)
                .unwrap_or_else(|error| panic!("{extension}: {error}"));
            assert_eq!(design.stitch_count(), 553, "{extension}");
            let [width, height] = design.size_mm();
            assert!((20.0..30.0).contains(&width), "{extension}: {width}");
            assert!((40.0..50.0).contains(&height), "{extension}: {height}");
            assert_ne!(
                design.stitches[0].color, design.stitches[300].color,
                "{extension}"
            );
        }
        for extension in ["pes", "tbf", "vp3", "xxx"] {
            let design =
                Design::read(&root.join(format!("Wildflower.{extension}")), &cancelled).unwrap();
            assert_eq!(design.stitches[0].color, [34, 118, 90], "{extension}");
            assert_eq!(design.stitches[300].color, [174, 64, 94], "{extension}");
            assert_eq!(design.stitches[552].color, [227, 166, 50], "{extension}");
        }
        let path = root.join("Wildflower.pes");
        assert!(matches!(
            crate::ClipItem::from_text(path.to_string_lossy().as_ref()).payload,
            crate::ClipPayload::FileBatch(paths) if paths == [path]
        ));
    }

    #[test]
    fn pen_travel_and_color_changes_leave_unsewn_gaps() {
        let mut builder = StitchBuilder::default();
        builder.add(Event::Sew([10, 15], None));
        builder.add(Event::Sew([30, 40], None));
        builder.add(Event::Break);
        builder.add(Event::Sew([65, 80], None));
        builder.add(Event::ThreadChange);
        builder.add(Event::Sew([120, 150], None));
        let [start, sewn, after_jump, after_change] = builder.stitches.as_slice() else {
            panic!("Expected four sewn records");
        };
        assert_eq!(sewn.from, start.to);
        assert_eq!(after_jump.from, after_jump.to);
        assert_eq!(after_change.from, after_change.to);
        assert_eq!(after_jump.color, start.color);
        assert_ne!(after_change.color, start.color);
    }

    #[test]
    fn malformed_protected_and_cancelled_sources_do_not_render() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bad.PES");
        std::fs::write(&path, b"this is not a design").unwrap();
        assert!(Design::read(&path, &AtomicBool::new(false)).is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/proc/uptime", &path).unwrap();
        assert!(Design::read(&path, &AtomicBool::new(false)).is_err());
        assert_eq!(
            Design::read(&path, &AtomicBool::new(true)).unwrap_err(),
            "Embroidery preview cancelled"
        );
        assert_eq!(Format::for_path(Path::new("binary.dat")), None);
    }

    #[test]
    fn native_vip_preview_reads_compressed_stitches() {
        use oxideav_embroidery::{Command, Design as Native, hus};
        let design = Native {
            commands: vec![
                Command::Stitch { dx: 0, dy: 0 },
                Command::Stitch { dx: 80, dy: 120 },
                Command::End,
            ],
            ..Native::default()
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sewn.VIP");
        std::fs::write(
            &path,
            hus::encode_vip(&design, &Default::default()).unwrap(),
        )
        .unwrap();
        let decoded = Design::read(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(decoded.size_mm(), [8.0, 12.0]);
        assert_eq!(decoded.stitches[1].to, [80, -120]);
    }
}
