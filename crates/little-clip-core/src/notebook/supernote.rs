//! Bounded Supernote container metadata, without decoding unrelated page ink.
//!
//! Format interpretation follows supernotelib's Apache-2.0 parser, copyright
//! (c) 2020 jya. Its pristine source and license are retained under
//! `vendor/supernote-tool`; local code uses checked offsets and explicit budgets.
use std::io::{Read, Seek, SeekFrom};

const MAX_SOURCE: u64 = 512 * 1024 * 1024;
const MAX_METADATA_BLOCK: usize = 1024 * 1024;
const MAX_METADATA_TOTAL: usize = 16 * 1024 * 1024;
const MAX_FIELDS: usize = 8192;
const MAX_PAGES: usize = 4096;
const VERSIONS: &[&str] = &[
    "20200001", "20200005", "20200006", "20200007", "20200008", "20210009", "20210010", "20220011",
    "20220013", "20230014", "20230015", "20260016",
];

/// Ordered metadata. Repeated properties, including legacy PAGE entries, survive.
pub type Properties = Vec<(String, String)>;

/// Page metadata only; bitmap addresses remain lazy references into the source.
#[derive(Debug)]
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub properties: Properties,
}

/// Container index for one pinned ordinary source; contains no decoded ink.
#[derive(Debug)]
pub struct Document {
    pub version: String,
    pub device: Option<String>,
    pub header: Properties,
    pub footer: Properties,
    pub pages: Vec<Page>,
}

/// Identifies a Supernote header, independently of a potentially ambiguous suffix.
pub fn has_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"noteSN_FILE_VER_") || bytes.starts_with(b"SN_FILE_ASA_")
}

/// Reads only bounded footer/header/page metadata from a seekable input.
///
/// The caller owns guarded source opening and revalidation before publication.
///
/// # Errors
///
/// Rejects unsupported versions, invalid offsets, malformed metadata, ambiguous
/// scalar fields and inputs exceeding the source/page/metadata budgets.
pub fn inspect(input: &mut (impl Read + Seek)) -> Result<Document, String> {
    let mut reader = Reader::new(input)?;
    reader.input.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut signature = [0; 24];
    reader.input.read_exact(&mut signature).map_err(io_error)?;
    let (version, legacy) = version(&signature)?;
    reader
        .input
        .seek(SeekFrom::Start(reader.end))
        .map_err(io_error)?;
    let footer_address = reader.u32()?;
    if footer_address == 0 {
        return Err("Supernote footer address is missing".into());
    }
    let footer = reader.metadata(footer_address)?;
    let header_address = address(required(&footer, "FILE_FEATURE")?, "header")?;
    let header = reader.metadata(header_address)?;
    let device = scalar(&header, "APPLY_EQUIPMENT")?.map(str::to_owned);
    let (width, height) = if device.as_deref() == Some("N5") {
        (1920, 2560)
    } else {
        (1404, 1872)
    };
    let addresses = page_addresses(&footer, legacy)?;
    let mut pages = Vec::with_capacity(addresses.len());
    for address in addresses {
        let properties = reader.metadata(address)?;
        let horizontal = matches!(scalar(&properties, "ORIENTATION")?, Some("1090" | "1270"));
        pages.push(Page {
            width: if horizontal { height } else { width },
            height: if horizontal { width } else { height },
            properties,
        });
    }
    Ok(Document {
        version,
        device,
        header,
        footer,
        pages,
    })
}

fn version(bytes: &[u8]) -> Result<(String, bool), String> {
    if bytes.starts_with(b"SN_FILE_ASA_20190529") {
        return Ok(("20190529".into(), true));
    }
    let value = bytes
        .strip_prefix(b"noteSN_FILE_VER_")
        .and_then(|version| std::str::from_utf8(version).ok())
        .ok_or("Not a Supernote notebook")?;
    if !VERSIONS.contains(&value) {
        return Err(format!("Unsupported Supernote version: {value}"));
    }
    Ok((value.into(), false))
}

fn page_addresses(fields: &Properties, legacy: bool) -> Result<Vec<u32>, String> {
    let mut pages = Vec::new();
    let mut names = std::collections::HashSet::new();
    for (name, value) in fields {
        let matched = if legacy {
            name == "PAGE"
        } else {
            name.strip_prefix("PAGE").is_some_and(|tail| {
                !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit())
            })
        };
        if !matched {
            continue;
        }
        if !legacy && !names.insert(name) {
            return Err("Duplicate Supernote page identity".into());
        }
        let pointer = address(value, "page")?;
        if pointer == 0 || pages.len() >= MAX_PAGES {
            return Err("Invalid Supernote page address or page-count limit exceeded".into());
        }
        // The footer is an ordered mapping. Physical offsets and lexical key
        // ordering do not authorize reordering the notebook's pages.
        pages.push(pointer);
    }
    if pages.is_empty() {
        return Err("Supernote notebook has no pages".into());
    }
    Ok(pages)
}

fn scalar<'a>(fields: &'a Properties, key: &str) -> Result<Option<&'a str>, String> {
    let mut values = fields
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str());
    let first = values.next();
    if values.next().is_some() {
        return Err(format!("Duplicate Supernote {key} property"));
    }
    Ok(first)
}
fn required<'a>(fields: &'a Properties, key: &str) -> Result<&'a str, String> {
    scalar(fields, key)?.ok_or_else(|| format!("Missing Supernote {key} property"))
}
fn address(value: &str, kind: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("Invalid Supernote {kind} address"))
}
fn io_error(error: std::io::Error) -> String {
    format!("Supernote read failed: {error}")
}

struct Reader<'a, R> {
    input: &'a mut R,
    // The final u32 is the footer pointer, not block payload.
    end: u64,
    metadata_left: usize,
}
impl<'a, R: Read + Seek> Reader<'a, R> {
    fn new(input: &'a mut R) -> Result<Self, String> {
        let length = input.seek(SeekFrom::End(0)).map_err(io_error)?;
        if !(28..=MAX_SOURCE).contains(&length) {
            return Err("Supernote source is truncated or exceeds 512 MiB".into());
        }
        Ok(Self {
            input,
            end: length - 4,
            metadata_left: MAX_METADATA_TOTAL,
        })
    }
    fn u32(&mut self) -> Result<u32, String> {
        let mut bytes = [0; 4];
        self.input.read_exact(&mut bytes).map_err(io_error)?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn metadata(&mut self, address: u32) -> Result<Properties, String> {
        if address == 0 {
            return Ok(Vec::new());
        }
        let start = u64::from(address);
        if start + 4 > self.end {
            return Err("Supernote block address is outside the source".into());
        }
        self.input.seek(SeekFrom::Start(start)).map_err(io_error)?;
        let length = self.u32()? as usize;
        if length > MAX_METADATA_BLOCK
            || length > self.metadata_left
            || start + 4 + length as u64 > self.end
        {
            return Err("Supernote metadata block exceeds the source or metadata budget".into());
        }
        self.metadata_left -= length;
        let mut bytes = vec![0; length];
        self.input.read_exact(&mut bytes).map_err(io_error)?;
        parse_fields(&bytes)
    }
}

fn parse_fields(bytes: &[u8]) -> Result<Properties, String> {
    let mut source = std::str::from_utf8(bytes).map_err(|_| "Supernote metadata is not UTF-8")?;
    let mut fields = Vec::new();
    while !source.trim().is_empty() {
        source = source.trim_start();
        let body = source
            .strip_prefix('<')
            .ok_or("Malformed Supernote metadata tag")?;
        let (field, tail) = body
            .split_once('>')
            .ok_or("Unclosed Supernote metadata tag")?;
        let (key, value) = field
            .split_once(':')
            .ok_or("Supernote metadata tag has no value separator")?;
        if key.is_empty() || key.contains(['<', '>']) || fields.len() >= MAX_FIELDS {
            return Err("Supernote metadata property is invalid or exceeds its budget".into());
        }
        fields.push((key.to_owned(), value.to_owned()));
        source = tail;
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    const TEN: &[u8] = include_bytes!(
        "../../../../docs/examples/notebooks/supernote/demo-a5x-20230015-1to10.note"
    );
    const MODERN: &[u8] = include_bytes!(
        "../../../../docs/examples/notebooks/supernote/stroke-n5-20260016-isolation-tools-colors-widths.note"
    );
    const TWO: &[u8] = include_bytes!(
        "../../../../docs/examples/notebooks/supernote/blank-a6x-3.26.40-two-pages.note"
    );

    #[test]
    fn real_x_and_current_x2_sources_have_correct_page_counts_and_sizes() {
        for (source, pages, device, size) in [
            (TEN, 10, "A5X", (1404, 1872)),
            (TWO, 2, "A6X", (1404, 1872)),
            (MODERN, 5, "N5", (1920, 2560)),
        ] {
            let doc = inspect(&mut Cursor::new(source)).unwrap();
            assert_eq!(doc.pages.len(), pages);
            assert_eq!(doc.device.as_deref(), Some(device));
            assert!(
                doc.pages
                    .iter()
                    .all(|page| (page.width, page.height) == size)
            );
        }
    }
    #[test]
    fn footer_order_and_repeated_legacy_entries_are_preserved() {
        let fields = parse_fields(b"<PAGE10:50><PAGE2:80><FILE_FEATURE:24>").unwrap();
        assert_eq!(page_addresses(&fields, false).unwrap(), [50, 80]);
        assert_eq!(
            page_addresses(&parse_fields(b"<PAGE:60><PAGE:40>").unwrap(), true).unwrap(),
            [60, 40]
        );
        assert!(page_addresses(&parse_fields(b"<PAGE1:60><PAGE1:40>").unwrap(), false).is_err());
        assert!(
            scalar(
                &parse_fields(b"<PROTOCOL:a><PROTOCOL:b>").unwrap(),
                "PROTOCOL"
            )
            .is_err()
        );
    }
    #[test]
    fn unknown_versions_and_out_of_source_blocks_are_rejected_before_allocation() {
        let mut future = TWO.to_vec();
        future[16..24].copy_from_slice(b"29999999");
        assert!(
            inspect(&mut Cursor::new(future))
                .unwrap_err()
                .contains("Unsupported Supernote")
        );
        let mut bad = TWO.to_vec();
        let end = bad.len();
        bad[end - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            inspect(&mut Cursor::new(bad))
                .unwrap_err()
                .contains("outside the source")
        );
        let mut bad = TWO.to_vec();
        let offset = u32::from_le_bytes(bad[bad.len() - 4..].try_into().unwrap()) as usize;
        bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            inspect(&mut Cursor::new(bad))
                .unwrap_err()
                .contains("budget")
        );
    }
}
