mod xml;
pub(crate) use xml::{attribute, local_name};
/// Resolve an XML entity reference such as `&amp;` to the text it stands for.
///
/// quick-xml reports these as `Event::GeneralRef` rather than as text, so a
/// parser that only handles `Event::Text` silently drops every `&` in the
/// document. Every text-collecting loop in this module needs both events.
pub(crate) fn decode_xml_reference(
    reference: &quick_xml::events::BytesRef<'_>,
    context: &str,
) -> crate::error::Result<String> {
    let name = reference.decode().map_err(|error| {
        crate::error::Error::InvalidInput(format!("invalid XML reference in {context}: {error}"))
    })?;
    quick_xml::escape::unescape(&format!("&{name};"))
        .map(|value| value.into_owned())
        .map_err(|error| {
            crate::error::Error::InvalidInput(format!(
                "invalid XML reference in {context}: {error}"
            ))
        })
}

pub(crate) fn text_advance_factor(character: char) -> f64 {
    let code = u32::from(character);
    if character == '\t' {
        return 1.5;
    }
    if character.is_control() || character == '\u{00ad}' {
        return 0.0;
    }
    if matches!(
        code,
        0x0300..=0x036f
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x20d0..=0x20ff
            | 0xfe20..=0xfe2f
    ) {
        return 0.0;
    }
    if character.is_whitespace() {
        return 0.28;
    }
    if character.is_ascii() {
        return match character {
            'i' | 'l' | 'I' | '!' | '|' | '.' | ',' | ':' | ';' | '\'' | '`' => 0.28,
            'f' | 'j' | 'r' | 't' | '(' | ')' | '[' | ']' | '{' | '}' => 0.38,
            'm' | 'w' | 'M' | 'W' | '@' | '%' | '&' => 0.82,
            'A'..='Z' => 0.64,
            '0'..='9' => 0.56,
            'a'..='z' => 0.52,
            _ => 0.5,
        };
    }
    if matches!(code, 0xff61..=0xff9f) {
        return 0.5;
    }
    if matches!(
        code,
        0x1100..=0x11ff
            | 0x2e80..=0x9fff
            | 0xac00..=0xd7af
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe6f
            | 0xff01..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f000..=0x1faff
    ) {
        return 1.0;
    }
    0.58
}
