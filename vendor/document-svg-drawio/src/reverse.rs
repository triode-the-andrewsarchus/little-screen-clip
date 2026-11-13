use crate::{
    error::{Error, Result},
    ooxml::{decode_xml_reference, local_name},
};
use base64::Engine;
use quick_xml::{Reader, events::Event};
const MAX_NESTED_SVG_DEPTH: usize = 4;
pub(crate) fn validate_svg_document(bytes: &[u8], depth: usize) -> Result<()> {
    if depth > MAX_NESTED_SVG_DEPTH {
        return Err(Error::LimitExceeded(format!(
            "nested SVG depth exceeds {MAX_NESTED_SVG_DEPTH}"
        )));
    }
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut saw_root = false;
    let mut root_closed = false;
    let mut element_depth = 0usize;
    let mut style_depth = None::<usize>;
    let mut style_text = String::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::DocType(doctype) => {
                // Every draw.io SVG export starts with the standard SVG 1.1
                // document type, so refusing all of them would refuse the
                // files this converter most needs to read. What has to stay
                // out is an entity declaration, which needs an internal subset;
                // an external DTD is never fetched by this reader or written to
                // the output.
                let text = doctype
                    .decode()
                    .map_err(|error| {
                        Error::InvalidInput(format!("invalid SVG document type: {error}"))
                    })?
                    .to_ascii_uppercase();
                if text.contains('[') || text.contains("ENTITY") {
                    return Err(Error::InvalidInput(
                        "SVG document type entities are not allowed".into(),
                    ));
                }
            }
            Event::PI(_) => {
                return Err(Error::InvalidInput(
                    "SVG processing instructions are not allowed".into(),
                ));
            }
            // A reference inside a style rule is reported separately from the
            // text around it, so `u&#114;l(...)` would slip past a check that
            // only ever sees one chunk at a time. Resolving each reference into
            // the same buffer as the text, and checking the whole rule once the
            // element closes, is what makes the check see what CSS will.
            Event::GeneralRef(reference) if style_depth.is_some() => {
                push_style_text(
                    &mut style_text,
                    &decode_xml_reference(&reference, "SVG CSS")?,
                )?;
            }
            Event::Start(start) => {
                if root_closed {
                    return Err(Error::InvalidInput(
                        "input contains content after the SVG root".into(),
                    ));
                }
                if !saw_root {
                    if local_name(start.name().as_ref()) != b"svg" {
                        return Err(Error::InvalidInput(
                            "input must have an <svg> root element".into(),
                        ));
                    }
                    validate_svg_root_namespace(&start)?;
                    saw_root = true;
                }
                validate_svg_element(&start, reader.decoder(), depth)?;
                if local_name(start.name().as_ref()).eq_ignore_ascii_case(b"style") {
                    style_depth = Some(element_depth + 1);
                }
                element_depth += 1;
            }
            Event::Empty(start) => {
                if root_closed {
                    return Err(Error::InvalidInput(
                        "input contains content after the SVG root".into(),
                    ));
                }
                if !saw_root {
                    if local_name(start.name().as_ref()) != b"svg" {
                        return Err(Error::InvalidInput(
                            "input must have an <svg> root element".into(),
                        ));
                    }
                    validate_svg_root_namespace(&start)?;
                    saw_root = true;
                    root_closed = true;
                }
                validate_svg_element(&start, reader.decoder(), depth)?;
            }
            Event::Text(text) => {
                let value = text.decode().map_err(|error| {
                    Error::InvalidInput(format!("invalid SVG text encoding: {error}"))
                })?;
                if (!saw_root || root_closed) && !value.trim().is_empty() {
                    return Err(Error::InvalidInput(
                        "input contains text outside the SVG root".into(),
                    ));
                }
                if style_depth.is_some() {
                    push_style_text(&mut style_text, &value)?;
                }
            }
            Event::CData(text) => {
                if !saw_root || root_closed {
                    return Err(Error::InvalidInput(
                        "input contains CDATA outside the SVG root".into(),
                    ));
                }
                if style_depth.is_some() {
                    let value = String::from_utf8_lossy(text.as_ref());
                    push_style_text(&mut style_text, &value)?;
                }
            }
            Event::End(end) => {
                element_depth = element_depth.saturating_sub(1);
                if element_depth == 0 {
                    root_closed = true;
                }
                if local_name(end.name().as_ref()).eq_ignore_ascii_case(b"style") {
                    style_depth = None;
                    validate_css_references(&style_text)?;
                    style_text.clear();
                }
            }
            Event::Eof => {
                if !style_text.is_empty() {
                    validate_css_references(&style_text)?;
                }
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    if !saw_root || !root_closed {
        return Err(Error::InvalidInput(
            "input does not contain an SVG root".into(),
        ));
    }
    Ok(())
}

fn validate_svg_root_namespace(start: &quick_xml::events::BytesStart<'_>) -> Result<()> {
    let raw_name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
    let namespace_key = raw_name.split_once(':').map_or_else(
        || "xmlns".to_owned(),
        |(prefix, _)| format!("xmlns:{prefix}"),
    );
    let mut namespace = None::<String>;
    for item in start.attributes().with_checks(true) {
        let item = item
            .map_err(|error| Error::InvalidInput(format!("invalid SVG root attribute: {error}")))?;
        if item.key.as_ref() == namespace_key.as_bytes() {
            namespace = Some(String::from_utf8_lossy(item.value.as_ref()).into_owned());
        }
    }
    if raw_name.contains(':') && namespace.as_deref() != Some("http://www.w3.org/2000/svg") {
        return Err(Error::InvalidInput(
            "prefixed SVG root must use the SVG namespace".into(),
        ));
    }
    if namespace
        .as_deref()
        .is_some_and(|value| value != "http://www.w3.org/2000/svg")
    {
        return Err(Error::InvalidInput(
            "SVG root uses an unsupported namespace".into(),
        ));
    }
    Ok(())
}

fn validate_svg_element(
    start: &quick_xml::events::BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
    depth: usize,
) -> Result<()> {
    let name = String::from_utf8_lossy(local_name(start.name().as_ref())).to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "script"
            | "foreignobject"
            | "iframe"
            | "object"
            | "embed"
            | "animate"
            | "animatetransform"
            | "animatemotion"
            | "discard"
            | "set"
    ) {
        return Err(Error::InvalidInput(format!(
            "active SVG element <{name}> is not allowed"
        )));
    }
    for item in start.attributes().with_checks(true) {
        let item =
            item.map_err(|error| Error::InvalidInput(format!("invalid SVG attribute: {error}")))?;
        let key = String::from_utf8_lossy(local_name(item.key.as_ref())).to_ascii_lowercase();
        let value = item
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)?
            .into_owned();
        if key.starts_with("on") || key == "base" {
            return Err(Error::InvalidInput(format!(
                "SVG event attribute {key} is not allowed"
            )));
        }
        if key == "href" {
            validate_svg_href(&value, depth)?;
        }
        // `content` is where draw.io keeps a copy of the diagram. No renderer
        // reads it and no CSS can reach it, and this converter parses it as
        // XML before trusting it, so the CSS rules do not apply and would
        // reject an ordinary diagram that happens to contain a backslash.
        if key != "content" {
            validate_css_references(&value)?;
        }
    }
    Ok(())
}

fn validate_svg_href(value: &str, depth: usize) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('#') {
        return Ok(());
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("data:image/png;")
        || lower.starts_with("data:image/jpeg;")
        || lower.starts_with("data:image/gif;")
        || lower.starts_with("data:image/webp;")
    {
        return Ok(());
    }
    if lower.starts_with("data:image/svg+xml;base64,") {
        let encoded = value
            .split_once(',')
            .map(|(_, data)| data)
            .unwrap_or_default();
        let compact = encoded
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        let nested = base64::engine::general_purpose::STANDARD
            .decode(compact)
            .map_err(|error| {
                Error::InvalidInput(format!("invalid nested SVG data URI: {error}"))
            })?;
        return validate_svg_document(&nested, depth + 1);
    }
    Err(Error::InvalidInput(format!(
        "external or active SVG reference is not allowed: {}",
        value.chars().take(80).collect::<String>()
    )))
}

fn validate_css_references(value: &str) -> Result<()> {
    let lower = value.to_ascii_lowercase();
    if lower.contains('\\') || lower.contains("/*") {
        return Err(Error::InvalidInput(
            "SVG CSS escapes and comments are not allowed".into(),
        ));
    }
    if lower.contains("@import") || lower.contains("javascript:") {
        return Err(Error::InvalidInput(
            "external or active SVG CSS is not allowed".into(),
        ));
    }
    let mut remainder = value;
    while let Some(index) = remainder.to_ascii_lowercase().find("url(") {
        let after = &remainder[index + 4..];
        let Some(end) = after.find(')') else {
            return Err(Error::InvalidInput("unterminated SVG CSS url()".into()));
        };
        let target = after[..end]
            .trim()
            .trim_matches(|character| matches!(character, '\'' | '"'));
        if !target.starts_with('#') {
            return Err(Error::InvalidInput(
                "external SVG CSS url() reference is not allowed".into(),
            ));
        }
        remainder = &after[end + 1..];
    }
    Ok(())
}

/// Collect one `<style>` element's content, bounded so a pathological document
/// cannot make the check itself the problem.
fn push_style_text(buffer: &mut String, value: &str) -> Result<()> {
    const MAX_STYLE_BYTES: usize = 4 * 1024 * 1024;
    if buffer.len().saturating_add(value.len()) > MAX_STYLE_BYTES {
        return Err(Error::LimitExceeded(format!(
            "SVG style content exceeds {MAX_STYLE_BYTES} bytes"
        )));
    }
    buffer.push_str(value);
    Ok(())
}
