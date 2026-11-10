//! Cheap format hints shared by strips, row admission and studio dispatch.
//!
//! These hints do not establish content identity, readability or edit support.
//! Workers validate ordinary sources and decode them before publishing prepared
//! documents. Ambiguous sampler names require positive content identification;
//! they retain their ordinary routing until a worker supplies that evidence.
use std::path::Path;
/// Cheap semantic-document path hints; workers establish actual content support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Font,
    Notebook,
    Book,
    Mail,
    Calendar,
    Database,
    Geography,
    Toolpath,
    Multisample,
    Subtitle,
}
impl Format {
    /// Returns the shared filename hint without reading or admitting the source.
    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        let lower;
        let extension = if extension.bytes().any(|b| b.is_ascii_uppercase()) {
            lower = extension.to_ascii_lowercase();
            lower.as_str()
        } else {
            extension
        };
        match extension {
            "ttf" | "ttc" | "otf" | "otc" | "woff" | "woff2" => Some(Self::Font),
            "ipynb" => Some(Self::Notebook),
            "epub" | "cbz" => Some(Self::Book),
            "eml" => Some(Self::Mail),
            "ics" => Some(Self::Calendar),
            "sqlite" | "sqlite3" | "db" | "db3" => Some(Self::Database),
            "gpx" | "geojson" => Some(Self::Geography),
            "gcode" => Some(Self::Toolpath),
            "sfz" | "sf2" | "nki" | "dspreset" | "dslibrary" | "multisample" | "xrni" => {
                Some(Self::Multisample)
            }
            _ => crate::subtitles::is_subtitle_path(path).then_some(Self::Subtitle),
        }
    }
}
/// Returns whether a path uses a Gaussian-capable scene container extension.
///
/// GLB may instead contain meshes; loading classifies it on the worker thread.
pub fn is_splat_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            ["ply", "splat", "sog", "spz", "glb"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// Returns whether a path has the conventional MIDI filename suffix.
pub fn is_midi(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classification_and_preview_source_share_the_same_format_inventory() {
        for (name, format) in [
            ("font.OTC", Format::Font),
            ("a.IPYNB", Format::Notebook),
            ("a.EPUB", Format::Book),
            ("a.CBZ", Format::Book),
            ("a.EML", Format::Mail),
            ("a.ICS", Format::Calendar),
            ("a.DB3", Format::Database),
            ("a.GPX", Format::Geography),
            ("a.GEOJSON", Format::Geography),
            ("a.GCODE", Format::Toolpath),
            ("a.SF2", Format::Multisample),
            ("a.SFZ", Format::Multisample),
            ("a.NKI", Format::Multisample),
            ("a.DSPRESET", Format::Multisample),
            ("a.DSLIBRARY", Format::Multisample),
            ("a.MULTISAMPLE", Format::Multisample),
            ("a.VTT", Format::Subtitle),
        ] {
            assert_eq!(Format::for_path(Path::new(name)), Some(format));
        }
        assert_eq!(Format::for_path(Path::new("ordinary.txt")), None);
        assert_eq!(Format::for_path(Path::new("ordinary.wav")), None);
        for path in [
            "ordinary.xml",
            "ordinary.exs",
            "ordinary.xpm",
            "ordinary.preset",
            "ordinary.iso",
        ] {
            assert_eq!(Format::for_path(Path::new(path)), None);
        }
    }
}
