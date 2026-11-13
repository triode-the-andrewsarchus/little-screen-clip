use quick_xml::XmlVersion;
use quick_xml::events::BytesStart;
use quick_xml::events::attributes::Attribute;

pub(crate) fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

/// The attribute's text, with XML entities resolved.
///
/// `Attribute::value` holds the raw, still-escaped bytes. Taking them verbatim
/// means an alt text of `R&amp;D` reaches the SVG writer as the literal
/// `R&amp;D`, which is escaped again and shows up as `R&amp;D` on screen.
fn attribute_text(item: &Attribute<'_>) -> String {
    // OOXML parts are XML 1.0; none of them carry a 1.1 declaration.
    item.normalized_value(XmlVersion::Implicit1_0).map_or_else(
        |_| String::from_utf8_lossy(item.value.as_ref()).into_owned(),
        |value| value.into_owned(),
    )
}

pub(crate) fn attribute(start: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    start
        .attributes()
        .with_checks(false)
        .flatten()
        .find_map(|item| (local_name(item.key.as_ref()) == name).then(|| attribute_text(&item)))
}
