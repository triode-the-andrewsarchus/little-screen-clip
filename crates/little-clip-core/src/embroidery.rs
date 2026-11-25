//! Guarded machine-embroidery decoding and stitch-order previews.

mod decode;
mod raster;

pub use raster::{Timeline, TimelineLayout};

use crate::file_source::read_file_snapshot_with_source;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMANDS: usize = 500_000;

/// Embroidery filename hints. Generic extensions such as DAT and CSV are
/// intentionally left to their regular file handlers.
pub fn supports_extension(extension: &str) -> bool {
    decode::reader(extension).is_some()
}

/// A sewn segment in tenths of a millimeter, with positive Y down.
#[derive(Debug)]
pub struct Stitch {
    pub from: [i32; 2],
    pub to: [i32; 2],
    pub color: [u8; 3],
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
            .ok_or("Unsupported embroidery format")?;
        let extension = extension.to_ascii_lowercase();
        let reader = decode::reader(&extension).ok_or("Unsupported embroidery format")?;
        check_cancelled(cancelled)?;
        let (bytes, source) =
            read_file_snapshot_with_source(path, MAX_BYTES).map_err(|error| error.to_string())?;
        let stitches = decode::read(reader, &bytes, cancelled)?;
        source.check()?;
        check_cancelled(cancelled)?;
        Self::from_stitches(stitches)
    }

    fn from_stitches(stitches: Vec<Stitch>) -> Result<Self, String> {
        let first = stitches.first().ok_or("No stitches in design")?;
        let (mut min, mut max) = (first.from, first.from);
        for point in stitches.iter().flat_map(|stitch| [stitch.from, stitch.to]) {
            for axis in 0..2 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
        }
        Ok(Self { stitches, min, max })
    }

    /// Number of needle penetrations represented in the preview.
    pub fn stitch_count(&self) -> usize {
        self.stitches.len()
    }

    /// Sewn segments in needle order. An isolated penetration has equal endpoints.
    pub fn stitches(&self) -> &[Stitch] {
        &self.stitches
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_format::Format;

    #[test]
    fn machine_files_decode_in_stitch_order_and_keep_their_file_references() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/embroidery");
        let cancelled = AtomicBool::new(false);
        for extension in [
            "dst", "exp", "hus", "jef", "jef+", "jpx", "pec", "pes", "ptn", "tbf", "u01", "vip",
            "vp3", "xxx",
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
