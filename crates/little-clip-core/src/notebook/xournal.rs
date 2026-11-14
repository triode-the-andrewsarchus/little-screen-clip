//! Bounded read-only Xournal document indexing. No editor implementation is imported.
//!
//! The XML vocabulary is qualified against the original Xournal format and
//! current Xournal++ fixtures; their provenance is retained with the test data.
use super::check_cancel;
use std::{
    io::{Read, Seek},
    ops::Range,
    sync::atomic::AtomicBool,
};

mod container;
mod render;
pub use render::render;

pub(super) fn has_container_prefix(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
        || bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(&[0xef, 0xbb, 0xbf])
        || bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'<')
}

pub(super) fn has_signature(
    input: &mut (impl Read + Seek),
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let (bytes, _) = container::xml(input, cancel)?;
    Ok(
        crate::xml_content::root(&bytes[..bytes.len().min(64 * 1024)])
            .is_some_and(|root| root.name().as_ref() == b"xournal"),
    )
}

/// Page geometry in 72-dpi document coordinates, preserving authored order.
#[derive(Debug)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    xml: Range<usize>,
}

/// Bounded XML snapshot and lazy page/attachment references; contains no decoded ink.
#[derive(Debug)]
pub struct Document {
    pub pages: Vec<Page>,
    pub version: Option<u32>,
    xml: String,
    pub packaged: bool,
}

impl Document {
    /// Borrows one authored page fragment using a checked 1-based page number.
    pub fn page_xml(&self, page: usize) -> Result<&str, String> {
        let page = page
            .checked_sub(1)
            .and_then(|index| self.pages.get(index))
            .ok_or("Xournal page is outside the notebook")?;
        self.xml
            .get(page.xml.clone())
            .ok_or_else(|| "Invalid Xournal page range".into())
    }
}

/// Reads a checked XML page index from gzip, plain XML or an Xournal ZIP package.
///
/// Call on a guarded input in a disposable worker; this function never resolves
/// background filenames, images, entities or URLs.
///
/// # Errors
/// Rejects unsupported containers/versions, malformed XML and page/size budgets.
pub fn inspect(input: &mut (impl Read + Seek), cancel: &AtomicBool) -> Result<Document, String> {
    check_cancel(cancel)?;
    let (bytes, packaged) = container::xml(input, cancel)?;
    crate::xml_content::validate(&bytes, 1_000_000, 24)?;
    let xml = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    let tree = parse(&xml)?;
    let root = tree.root_element();
    if !root.has_tag_name("xournal") {
        return Err("Unrecognized Xournal document root".into());
    }
    let version = root
        .attribute("fileversion")
        .map(str::parse::<u32>)
        .transpose()
        .map_err(|_| "Invalid Xournal file version")?;
    if version.is_some_and(|version| version > 5) {
        return Err("Unsupported Xournal file version".into());
    }
    let mut pages = Vec::new();
    for node in root.children().filter(|node| node.has_tag_name("page")) {
        check_cancel(cancel)?;
        if pages.len() == 4096 {
            return Err("Xournal exceeds the 4096-page preview limit".into());
        }
        pages.push(Page {
            width: dimension(node, "width")?,
            height: dimension(node, "height")?,
            xml: node.range(),
        });
    }
    if pages.is_empty() {
        return Err("Xournal document contains no pages".into());
    }
    Ok(Document {
        pages,
        version,
        xml,
        packaged,
    })
}

fn parse(xml: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 500_000,
        },
    )
    .map_err(|error| format!("Invalid Xournal XML: {error}"))
}

fn dimension(node: roxmltree::Node<'_, '_>, key: &str) -> Result<f64, String> {
    let value: f64 = node
        .attribute(key)
        .ok_or_else(|| format!("Missing Xournal page {key}"))?
        .parse()
        .map_err(|_| format!("Invalid Xournal page {key}"))?;
    if !value.is_finite() || !(0.1..=16384.0).contains(&value) {
        return Err(format!("Xournal page {key} exceeds the dimension limit"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/examples/notebooks/xournal")
            .join(name)
    }

    #[test]
    fn real_legacy_gzip_and_packaged_sources_preserve_the_page_index() {
        for (name, count, packaged) in [
            ("legacy-stroke.xoj", 1, false),
            ("load/pages.xopp", 11, false),
            ("load/strokes.xopp", 1, false),
            ("packaged/stroke.xopp", 1, true),
            ("packaged/pdf-background.xopp", 2, true),
        ] {
            let mut file = std::fs::File::open(fixture(name)).unwrap();
            let document = inspect(&mut file, &AtomicBool::new(false)).unwrap();
            assert_eq!(document.pages.len(), count, "{name}");
            assert_eq!(document.packaged, packaged);
            let mut previous = 0;
            for page in &document.pages {
                assert!(page.width > 0.0 && page.height > 0.0);
                assert!(page.xml.start > previous);
                assert!(document.xml[page.xml.clone()].starts_with("<page"));
                previous = page.xml.end;
            }
        }
    }

    #[test]
    fn plain_xml_versions_and_resource_budgets_are_checked() {
        let xml = |body: &str| Cursor::new(body.as_bytes().to_vec());
        let cancel = AtomicBool::new(false);
        let valid =
            "<xournal fileversion='5'><page width='100' height='200'><layer/></page></xournal>";
        assert_eq!(inspect(&mut xml(valid), &cancel).unwrap().version, Some(5));
        for bad in [
            "<another><page width='100' height='200'/></another>",
            "<xournal/>",
            "<xournal><page width='NaN' height='200'/></xournal>",
            "<xournal><page width='100' height='200'>",
            "<xournal fileversion='999'><page width='100' height='200'/></xournal>",
            "<!DOCTYPE xournal [<!ENTITY x 'payload'>]><xournal><page width='100' height='200'/></xournal>",
        ] {
            assert!(inspect(&mut xml(bad), &cancel).is_err(), "{bad}");
        }
        assert!(inspect(&mut xml(valid), &AtomicBool::new(true)).is_err());
        let deep = format!(
            "<xournal>{}{}{}",
            "<a>".repeat(30),
            "</a>".repeat(30),
            "</xournal>"
        );
        assert!(
            inspect(&mut xml(&deep), &cancel)
                .unwrap_err()
                .contains("deep")
        );
    }
}
