//! One classification shared by semantic strips, row admission and studio dispatch.
use std::path::Path;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
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
            _ => little_clip_core::subtitles::is_subtitle_path(path).then_some(Self::Subtitle),
        }
    }
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
