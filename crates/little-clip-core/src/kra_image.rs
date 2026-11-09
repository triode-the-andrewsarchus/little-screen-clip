//! Bounded Krita saved composites and read-only source document information.
use crate::image_document::{Layer, Summary};
use crate::zip_content::entry;
use std::io::Cursor;
use zip::ZipArchive;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_XML: usize = 2 * 1024 * 1024;

fn archive(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, String> {
    let mut archive = crate::zip_content::archive(bytes)?;
    if entry(&mut archive, "mimetype", 64)? != b"application/x-krita" {
        return Err("Archive is not a Krita document".into());
    }
    Ok(archive)
}

/// Returns the saved full-resolution PNG, without developing native layer tiles.
///
/// # Errors
/// Returns an error for invalid, oversized or composite-free documents.
pub fn composite(bytes: &[u8]) -> Result<Vec<u8>, String> {
    png_entry(&mut archive(bytes)?, "mergedimage.png", MAX_BYTES)
}

/// Returns the saved thumbnail, when available within a small decode budget.
pub fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return None;
    }
    png_entry(&mut archive(bytes).ok()?, "preview.png", 4 * 1024 * 1024).ok()
}

fn png_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let bytes = entry(archive, name, limit)?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(format!("Krita {name} is not a PNG"));
    }
    Ok(bytes)
}

/// Reads canvas, profile and layer metadata without opening native tile payloads.
///
/// XML entities, external references and layer filenames are never followed.
///
/// # Errors
/// Returns an error for malformed or oversized source metadata.
pub fn inspect(bytes: &[u8]) -> Result<Summary, String> {
    let xml = entry(&mut archive(bytes)?, "maindoc.xml", MAX_XML)?;
    let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
    let mut summary = None;
    let mut nesting = 0usize;
    let mut layer_depths = Vec::new();
    loop {
        use quick_xml::events::Event;
        let event = reader.read_event().map_err(|e| e.to_string())?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(node) | Event::Empty(node) => {
                let is_layer =
                    record_source_node(&node, reader.decoder(), &mut summary, layer_depths.len())?;
                if is_layer && !empty {
                    layer_depths.push(nesting);
                }
                if !empty {
                    nesting += 1;
                }
                if nesting > 128 {
                    return Err("Krita XML is too deeply nested".into());
                }
            }
            Event::End(_) => {
                nesting = nesting.checked_sub(1).ok_or("Unbalanced Krita XML")?;
                if layer_depths.last() == Some(&nesting) {
                    layer_depths.pop();
                }
            }
            Event::Eof => break,
            // Krita's public DOCTYPE is descriptive; quick-xml never fetches it.
            _ => {}
        }
    }
    if nesting != 0 {
        return Err("Truncated Krita XML".into());
    }
    summary.ok_or_else(|| "Missing Krita canvas".into())
}

fn record_source_node(
    node: &quick_xml::events::BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
    summary: &mut Option<Summary>,
    depth: usize,
) -> Result<bool, String> {
    match node.local_name().as_ref() {
        b"IMAGE" => {
            if summary.is_some() {
                return Err("Multiple Krita canvases".into());
            }
            *summary = Some(canvas(&attributes(node, decoder)?)?);
            Ok(false)
        }
        b"layer" => {
            let doc = summary.as_mut().ok_or("Krita layer precedes canvas")?;
            if doc.layers.len() >= 8192 {
                return Err("Too many Krita layers".into());
            }
            doc.layers.push(layer(&attributes(node, decoder)?, depth));
            Ok(true)
        }
        _ => Ok(false),
    }
}

type Fields = std::collections::HashMap<String, String>;
fn attributes(
    node: &quick_xml::events::BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
) -> Result<Fields, String> {
    node.attributes()
        .map(|field| {
            let field = field.map_err(|e| e.to_string())?;
            Ok((
                String::from_utf8_lossy(field.key.as_ref()).into_owned(),
                field
                    .decode_and_unescape_value(decoder)
                    .map_err(|e| e.to_string())?
                    .into_owned(),
            ))
        })
        .collect()
}
fn number<T: std::str::FromStr>(fields: &Fields, key: &str) -> Result<T, String> {
    fields
        .get(key)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("Invalid Krita {key}"))
}
fn canvas(fields: &Fields) -> Result<Summary, String> {
    let width: u32 = number(fields, "width")?;
    let height: u32 = number(fields, "height")?;
    if width == 0
        || height == 0
        || width > 30_000
        || height > 30_000
        || fields.get("mime").map(String::as_str) != Some("application/x-kra")
    {
        return Err("Unsupported Krita canvas".into());
    }
    Ok(Summary {
        dimensions: (width, height),
        depth: None,
        profile: fields.get("profile").cloned(),
        color_mode: fields.get("colorspacename").cloned().unwrap_or_default(),
        layers: Vec::new(),
        paths: 0,
    })
}
fn layer(fields: &Fields, depth: usize) -> Layer {
    let kind = fields.get("nodetype").cloned().unwrap_or_default();
    Layer {
        name: fields.get("name").cloned().unwrap_or_default(),
        bounds: None,
        offset: fields
            .get("x")
            .zip(fields.get("y"))
            .and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?))),
        opacity: fields.get("opacity").and_then(|s| s.parse().ok()),
        visible: fields.get("visible").and_then(|s| match s.as_str() {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        }),
        blend_mode: fields.get("compositeop").cloned().unwrap_or_default(),
        depth,
        group: kind == "grouplayer",
        mask: false,
        vector_mask: false,
        kind: Some(match kind.as_str() {
            "paintlayer" => "Paint layer".into(),
            "shapelayer" => "Vector layer".into(),
            "transparencymask" => "Transparency mask".into(),
            "filtermask" => "Filter mask".into(),
            _ => kind,
        }),
    }
}
