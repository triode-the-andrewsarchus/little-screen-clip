//! EPUB package/spine inspection and inert reflowed chapter content.
use crate::zip_content::{archive, entry};
use roxmltree::{Document as Xml, Node};
use std::{collections::BTreeMap, sync::Arc};

const XML_LIMIT: usize = 2 * 1024 * 1024;

/// One reading-order document in a publication.
#[derive(Clone, Debug)]
pub struct Chapter {
    /// Navigation label, or manifest identifier when no label is supplied.
    pub title: String,
    /// Archive-local resource name; never a filesystem path.
    pub resource: String,
}
/// A read-only publication, retaining its bounded container for lazy chapters.
pub struct Book {
    bytes: Arc<[u8]>,
    /// Dublin Core title.
    pub title: String,
    /// Dublin Core creators.
    pub authors: Vec<String>,
    /// Reading-order documents from the package spine.
    pub chapters: Vec<Chapter>,
    /// Optional archive-local cover resource.
    pub cover: Option<String>,
}
pub use crate::html_preview::Block;
fn xml(bytes: &[u8]) -> Result<Xml<'_>, String> {
    crate::xml_content::validate(bytes, 100_000, 128)?;
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    Xml::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: 100_000,
        },
    )
    .map_err(|e| format!("Invalid EPUB XML: {e}"))
}
fn named<'a>(node: Node<'a, 'a>, name: &'a str) -> impl Iterator<Item = Node<'a, 'a>> {
    node.descendants()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}
fn text(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(Node::is_text)
        .filter(|n| {
            !n.ancestors().any(|p| {
                p.is_element() && matches!(p.tag_name().name(), "script" | "style" | "head")
            })
        })
        .filter_map(|n| n.text())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
/// Resolves an EPUB resource URL inside the container, without fetching any resource.
fn resource(base: &str, href: &str) -> Result<String, String> {
    let root = url::Url::parse("https://epub.invalid/").unwrap();
    let target = root
        .join(base)
        .and_then(|base| base.join(href))
        .map_err(|e| e.to_string())?;
    if target.origin() != root.origin() || target.query().is_some() || href.contains('\\') {
        return Err("EPUB resource is not an archive-local URL".into());
    }
    let path = percent_encoding::percent_decode_str(target.path().trim_start_matches('/'))
        .decode_utf8()
        .map_err(|e| e.to_string())?
        .into_owned();
    if path.split('/').any(|p| p == ".." || p == ".") || path.contains(['\0', '\\']) {
        return Err("Invalid EPUB resource path".into());
    }
    Ok(path)
}
impl Book {
    /// Reads package metadata, navigation labels and the explicit spine order.
    ///
    /// # Errors
    /// Rejects malformed/oversized archives, encrypted publications, invalid XML,
    /// missing spine members and unsupported non-XHTML reading-order documents.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        let mut zip = archive(&bytes)?;
        let (package, package_bytes) = package(&mut zip)?;
        let doc = xml(&package_bytes)?;
        let metadata = named(doc.root(), "metadata")
            .next()
            .ok_or("Missing EPUB metadata")?;
        let title = named(metadata, "title")
            .next()
            .map(text)
            .unwrap_or_default();
        let authors = named(metadata, "creator").map(text).collect();
        let items = manifest(&doc, &package)?;
        check_encryption(&mut zip, &items)?;
        let cover = cover(metadata, &items);
        let labels = labels(&mut zip, &items)?;
        let chapters = spine(&doc, &items, &labels)?;
        for chapter in &chapters {
            zip.by_name(&chapter.resource)
                .map_err(|e| format!("Missing EPUB chapter: {e}"))?;
        }
        drop(zip);
        Ok(Self {
            bytes: bytes.into(),
            title,
            authors,
            chapters,
            cover,
        })
    }
    /// Reads a bounded local image resource without extracting it to disk.
    ///
    /// # Errors
    /// Returns an error for missing or oversized members.
    pub fn image(&self, path: &str) -> Result<Vec<u8>, String> {
        entry(&mut archive(&self.bytes)?, path, 16 * 1024 * 1024)
    }
    /// Prepares the selected chapter as reflowed text and local image references.
    ///
    /// # Errors
    /// Rejects unavailable chapters and malformed or oversized XHTML.
    pub fn chapter(&self, index: usize) -> Result<Vec<Block>, String> {
        let chapter = self.chapters.get(index).ok_or("Unknown EPUB chapter")?;
        let bytes = entry(&mut archive(&self.bytes)?, &chapter.resource, XML_LIMIT)?;
        crate::xml_content::validate(&bytes, 100_000, 128)?;
        crate::html_preview::parse(
            std::str::from_utf8(&bytes).map_err(|e| e.to_string())?,
            |href| resource(&chapter.resource, href).ok(),
        )
    }
}
fn package(zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Result<(String, Vec<u8>), String> {
    if entry(zip, "mimetype", 64)? != b"application/epub+zip" {
        return Err("Not an EPUB publication".into());
    }
    let container = entry(zip, "META-INF/container.xml", XML_LIMIT)?;
    let container = xml(&container)?;
    let package = named(container.root(), "rootfile")
        .find(|n| n.attribute("media-type") == Some("application/oebps-package+xml"))
        .and_then(|n| n.attribute("full-path"))
        .ok_or("Missing EPUB package")?;
    let package = resource("", package)?;
    let package_bytes = entry(zip, &package, XML_LIMIT)?;
    Ok((package, package_bytes))
}
fn check_encryption(
    zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    items: &Items<'_>,
) -> Result<(), String> {
    if !zip
        .file_names()
        .any(|name| name == "META-INF/encryption.xml")
    {
        return Ok(());
    }
    let bytes = entry(zip, "META-INF/encryption.xml", XML_LIMIT)?;
    let encryption = xml(&bytes)?;
    for encrypted in named(encryption.root(), "EncryptedData") {
        let method = named(encrypted, "EncryptionMethod")
            .next()
            .and_then(|n| n.attribute("Algorithm"))
            .unwrap_or("");
        let path = named(encrypted, "CipherReference")
            .next()
            .and_then(|n| n.attribute("URI"))
            .ok_or("Missing EPUB encryption resource")?;
        let path = resource("", path)?;
        let font = items.values().any(|(name, node)| {
            name == &path
                && matches!(
                    node.attribute("media-type"),
                    Some(
                        "font/otf"
                            | "font/ttf"
                            | "font/woff"
                            | "font/woff2"
                            | "application/vnd.ms-opentype"
                            | "application/font-sfnt"
                            | "application/font-woff"
                    )
                )
        });
        if !font
            || !matches!(
                method,
                "http://www.idpf.org/2008/embedding" | "http://ns.adobe.com/pdf/enc#RC"
            )
        {
            return Err("Encrypted EPUB content is not supported".into());
        }
    }
    Ok(())
}
type Items<'a> = BTreeMap<&'a str, (String, Node<'a, 'a>)>;
fn manifest<'a>(doc: &'a Xml<'a>, package: &str) -> Result<Items<'a>, String> {
    let mut items = BTreeMap::new();
    for item in named(doc.root(), "item") {
        let id = item.attribute("id").ok_or("Missing EPUB item identifier")?;
        let href = item.attribute("href").ok_or("Missing EPUB resource URL")?;
        if items.insert(id, (resource(package, href)?, item)).is_some() {
            return Err("Duplicate EPUB manifest identifier".into());
        }
    }
    Ok(items)
}
fn cover(metadata: Node<'_, '_>, items: &Items<'_>) -> Option<String> {
    let cover_id = named(metadata, "meta")
        .find(|n| n.attribute("name") == Some("cover"))
        .and_then(|n| n.attribute("content"));

    items
        .iter()
        .find(|(id, (_, n))| {
            Some(**id) == cover_id
                || n.attribute("properties")
                    .unwrap_or("")
                    .split_whitespace()
                    .any(|p| p == "cover-image")
        })
        .map(|(_, (path, _))| path.clone())
}
fn labels(
    zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    items: &Items<'_>,
) -> Result<BTreeMap<String, String>, String> {
    let mut labels = BTreeMap::new();
    for (path, _) in items.values().filter(|(_, n)| {
        n.attribute("media-type") == Some("application/x-dtbncx+xml")
            || n.attribute("properties")
                .unwrap_or("")
                .split_whitespace()
                .any(|p| p == "nav")
    }) {
        let nav = entry(zip, path, XML_LIMIT)?;
        navigation(&xml(&nav)?, path, &mut labels)?;
    }
    Ok(labels)
}
fn spine(
    doc: &Xml<'_>,
    items: &Items<'_>,
    labels: &BTreeMap<String, String>,
) -> Result<Vec<Chapter>, String> {
    let mut chapters = Vec::new();
    for node in named(doc.root(), "itemref") {
        let id = node
            .attribute("idref")
            .ok_or("Missing EPUB spine reference")?;
        let (path, item) = items.get(id).ok_or("Unknown EPUB spine reference")?;
        if item.attribute("media-type") != Some("application/xhtml+xml") {
            return Err("This EPUB uses a non-XHTML spine document".into());
        }

        chapters.push(Chapter {
            title: labels.get(path).cloned().unwrap_or_else(|| id.into()),
            resource: path.clone(),
        });
        if chapters.len() > 4096 {
            return Err("EPUB has too many chapters".into());
        }
    }
    if chapters.is_empty() {
        return Err("EPUB has no reading-order documents".into());
    }
    Ok(chapters)
}
fn navigation(
    doc: &Xml<'_>,
    base: &str,
    labels: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for link in named(doc.root(), "a") {
        if let Some(href) = link.attribute("href")
            && let Ok(path) = resource(base, href)
        {
            labels.entry(path).or_insert_with(|| text(link));
        }
    }
    for point in named(doc.root(), "navPoint") {
        let label = point.children().find(|n| n.has_tag_name("navLabel"));
        let src = point
            .children()
            .find(|n| n.has_tag_name("content"))
            .and_then(|n| n.attribute("src"));
        if let (Some(label), Some(src)) = (label, src) {
            labels
                .entry(resource(base, src)?)
                .or_insert_with(|| text(label));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/ebooks/journey.epub");
    #[test]
    fn reading_order_labels_metadata_and_local_illustrations() {
        let book = Book::parse(BOOK.to_vec()).unwrap();
        assert_eq!(book.title, "A Small Journey");
        assert_eq!(book.authors, ["Example Author"]);
        assert_eq!(
            book.chapters
                .iter()
                .map(|c| c.title.as_str())
                .collect::<Vec<_>>(),
            ["Departure", "Arrival"]
        );
        let first = book.chapter(0).unwrap();
        assert!(first.contains(&Block::Heading(1, "Departure".into())));
        assert!(first.contains(&Block::Image(
            "Books/images/cover.png".into(),
            "Blue sea illustration".into()
        )));
        assert!(!format!("{first:?}").contains("HIDDEN"));
        assert!(!format!("{first:?}").contains("remote.png"));
        assert!(format!("{first:?}").contains("A & B."));
        assert!(
            book.image(book.cover.as_ref().unwrap())
                .unwrap()
                .starts_with(b"\x89PNG")
        );
        assert!(format!("{:?}", book.chapter(1).unwrap()).contains("harbor"));
        assert!(book.chapter(2).is_err());
    }
    #[test]
    fn epub_two_uses_legacy_ncx_labels_without_a_navigation_document() {
        let book = Book::parse(include_bytes!("../tests/fixtures/ebooks/journey-v2.epub").to_vec())
            .unwrap();
        assert_eq!(
            book.chapters
                .iter()
                .map(|c| c.title.as_str())
                .collect::<Vec<_>>(),
            ["Departure", "Arrival"]
        );
    }
    fn replace_members(changes: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::{Cursor, Read, Write};
        let mut input = archive(BOOK).unwrap();
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..input.len() {
            let mut member = input.by_index(index).unwrap();
            if changes.iter().any(|(name, _)| *name == member.name()) {
                continue;
            }
            let mut bytes = Vec::new();
            member.read_to_end(&mut bytes).unwrap();
            output
                .start_file(member.name(), zip::write::SimpleFileOptions::default())
                .unwrap();
            output.write_all(&bytes).unwrap();
        }
        for (name, bytes) in changes {
            output
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            output.write_all(bytes).unwrap();
        }
        output.finish().unwrap().into_inner()
    }
    #[test]
    fn font_obfuscation_can_be_ignored_but_encrypted_reading_content_is_rejected() {
        let package = entry(&mut archive(BOOK).unwrap(), "Books/main.opf", XML_LIMIT).unwrap();
        let package = String::from_utf8(package).unwrap().replace(
            "</manifest>",
            "<item id='font' href='font.ttf' media-type='font/ttf'/></manifest>",
        );
        let encryption = b"<encryption><EncryptedData><EncryptionMethod Algorithm='http://www.idpf.org/2008/embedding'/><CipherData><CipherReference URI='Books/font.ttf'/></CipherData></EncryptedData></encryption>";
        let bytes = replace_members(&[
            ("Books/main.opf", package.as_bytes()),
            ("META-INF/encryption.xml", encryption),
        ]);
        assert!(Book::parse(bytes).is_ok());
        let encrypted_content = String::from_utf8(encryption.to_vec())
            .unwrap()
            .replace("Books/font.ttf", "Books/text/first.xhtml");
        assert!(
            Book::parse(replace_members(&[(
                "META-INF/encryption.xml",
                encrypted_content.as_bytes()
            )]))
            .is_err()
        );
    }
    #[test]
    fn legacy_html_entities_are_read_without_resolving_external_dtds() {
        let chapter = b"<!DOCTYPE html PUBLIC '-//W3C//DTD XHTML 1.1//EN' 'https://example.invalid/never-fetch.dtd'><html xmlns='http://www.w3.org/1999/xhtml'><body><p>&ldquo;A&nbsp;sun<strong>lit</strong> sea&rdquo;</p></body></html>";
        let book = Book::parse(replace_members(&[("Books/text/first.xhtml", chapter)])).unwrap();
        assert_eq!(
            book.chapter(0).unwrap(),
            [Block::Text("\u{201c}A sunlit sea\u{201d}".into())]
        );
    }
    #[test]
    fn wrong_mime_and_oversized_package_are_rejected_before_parsing() {
        assert!(Book::parse(replace_members(&[("mimetype", b"application/zip")])).is_err());
        assert!(
            Book::parse(replace_members(&[(
                "Books/main.opf",
                &vec![b' '; XML_LIMIT + 1]
            )]))
            .is_err()
        );
    }
    #[test]
    fn inline_markup_preserves_word_boundaries_and_omits_scripts() {
        let doc = xml(b"<p>A <em>small</em> sail crosses sun<strong>lit</strong> water.<script>hidden</script></p>").unwrap();
        assert_eq!(
            text(doc.root_element()),
            "A small sail crosses sunlit water."
        );
    }
    #[test]
    fn resource_urls_are_container_local_and_percent_decoded() {
        assert_eq!(
            resource("Books/text/one.xhtml", "../images/a%20b.png#part").unwrap(),
            "Books/images/a b.png"
        );
        for href in [
            "file:///etc/passwd",
            "https://outside.invalid/a",
            "//outside.invalid/a",
            "x%2f..%2fa",
            "x%00y",
            "a\\b",
        ] {
            assert!(resource("Books/one.xhtml", href).is_err(), "{href}");
        }
    }
    #[test]
    fn rejects_custom_entities_deep_xml_and_invalid_containers() {
        assert!(xml(b"<!DOCTYPE x [<!ENTITY value 'payload'>]><x>&value;</x>").is_err());
        let deep = format!("{}{}", "<x>".repeat(129), "</x>".repeat(129));
        assert!(xml(deep.as_bytes()).is_err());
        let many = format!("<root>{}</root>", "<x/>".repeat(100_000));
        assert!(crate::xml_content::validate(many.as_bytes(), 100_000, 128).is_err());
        assert!(Book::parse(b"not a ZIP".to_vec()).is_err());
        assert!(Book::parse(BOOK[..BOOK.len() - 8].to_vec()).is_err());
    }
}
