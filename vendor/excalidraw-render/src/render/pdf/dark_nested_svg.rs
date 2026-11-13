//! Dark mode for a nested-SVG image's content in PDF (#154, spec section 8.2).
//!
//! The reference's dark SVG export inverts a nested-SVG image with the rest of
//! the scene under its root `THEME_FILTER`. A PDF cannot carry that filter, so
//! it lowers the filter into every color it paints (spec section 8.1); this
//! extends the lowering into the nested SVG before the backend draws it. The
//! sanitized tree is serialized through the backend's lossless normalized
//! writer, where every paint is a `#rrggbb` color attribute and every nested
//! raster is a normalized PNG data URL, and parsed again after:
//!
//! - `fill`, `stroke`, `stop-color`, `flood-color` and `lighting-color` colors
//!   pass through the rounded theme filter;
//! - each visible pixel of a nested raster passes through it, alpha untouched,
//!   and the raster is re-encoded with the sanitizer's normalized PNG encoder;
//! - `<mask>` content keeps its colors, because a mask acts through the
//!   luminance of its light-mode content and the reference filters only the
//!   composite.
//!
//! Nested-SVG images are already inlined by the writer, so their content is
//! lowered in the same pass. Like every other color lowering, this is not a
//! charged resource event.

use base64::Engine as _;

use crate::color;

use super::super::FormatEncodeError;
use super::raster::{edit, expanded_xml, memory, parse};

const COLOR_ATTRIBUTES: [&str; 5] = [
    "fill",
    "stroke",
    "stop-color",
    "flood-color",
    "lighting-color",
];
const PNG_DATA_URL: &str = "data:image/png;base64,";

fn invalid(message: &str) -> FormatEncodeError {
    FormatEncodeError::Encoding(message.to_owned())
}

/// The nested SVG with dark mode lowered into its content.
pub(super) fn lower(tree: &usvg::Tree) -> Result<usvg::Tree, FormatEncodeError> {
    let xml = expanded_xml(tree, &mut 0, None)?;
    let document = roxmltree::Document::parse(&xml)
        .map_err(|_| invalid("invalid normalized dark-mode nested SVG"))?;
    let mut edits = Vec::new();
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        if node
            .ancestors()
            .any(|ancestor| ancestor.has_tag_name("mask"))
        {
            continue;
        }
        for attribute in node.attributes() {
            let replacement = if COLOR_ATTRIBUTES.contains(&attribute.name()) {
                let Some(rgb) = parse_hex(attribute.value()) else {
                    continue;
                };
                let [red, green, blue] = color::theme_filter_rounded(rgb);
                format!("{}=\"#{red:02x}{green:02x}{blue:02x}\"", attribute.name())
            } else if node.has_tag_name("image") && attribute.name() == "href" {
                let Some(encoded) = attribute.value().strip_prefix(PNG_DATA_URL) else {
                    continue;
                };
                let mut url = String::from("xlink:href=\"");
                url.push_str(PNG_DATA_URL);
                base64::engine::general_purpose::STANDARD
                    .encode_string(lower_png(encoded.trim())?, &mut url);
                url.push('"');
                url
            } else {
                continue;
            };
            edits.try_reserve(1).map_err(|_| memory())?;
            edits.push((attribute.range(), replacement));
        }
    }
    parse(&edit(&xml, edits)?)
}

fn parse_hex(value: &str) -> Option<[u8; 3]> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&digits[index..index + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// A normalized nested raster with the theme filter lowered into its visible
/// pixels.
fn lower_png(encoded: &str) -> Result<Vec<u8>, FormatEncodeError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| invalid("nested raster is not base64"))?;
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|_| invalid("nested raster is not a normalized PNG"))?;
    let info = reader.info();
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(invalid("nested raster is not normalized RGBA8"));
    }
    let (width, height) = (info.width, info.height);
    let size = reader.output_buffer_size().ok_or_else(memory)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(size).map_err(|_| memory())?;
    rgba.resize(size, 0);
    reader
        .next_frame(&mut rgba)
        .map_err(|_| invalid("nested raster did not decode"))?;
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] != 0 {
            let [red, green, blue] = color::theme_filter_rounded([pixel[0], pixel[1], pixel[2]]);
            pixel[..3].copy_from_slice(&[red, green, blue]);
        }
    }
    super::super::asset::encode_normalized_png(width, height, &rgba)
        .map_err(|_| invalid("lowered nested raster did not encode"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(svg: &str) -> usvg::Tree {
        usvg::Tree::from_str(svg, &usvg::Options::default()).expect("fixture parses")
    }

    fn fill_of_first_path(group: &usvg::Group) -> Option<[u8; 3]> {
        for node in group.children() {
            match node {
                usvg::Node::Path(path) => {
                    if let Some(usvg::Paint::Color(c)) = path.fill().map(usvg::Fill::paint) {
                        return Some([c.red, c.green, c.blue]);
                    }
                }
                usvg::Node::Group(group) => {
                    if let Some(found) = fill_of_first_path(group) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }

    #[test]
    fn paints_and_gradient_stops_take_the_rounded_theme_filter() {
        let lowered = lower(&tree(concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4">"##,
            r##"<defs><linearGradient id="g"><stop offset="0" stop-color="#fffce8"/>"##,
            r##"<stop offset="1" stop-color="#1e1e1e"/></linearGradient></defs>"##,
            r##"<rect width="2" height="4" fill="#fffce8" stroke="#a5d8ff"/>"##,
            r##"<rect x="2" width="2" height="4" fill="url(#g)"/></svg>"##
        )))
        .expect("lowers");
        // #fffce8 is (24.39, 21.81, 4.61) through the filter; rounded, as the
        // official SVG's root filter rounds.
        assert_eq!(fill_of_first_path(lowered.root()), Some([0x18, 0x16, 0x05]));
        let stops: Vec<_> = lowered.linear_gradients()[0]
            .stops()
            .iter()
            .map(|stop| [stop.color().red, stop.color().green, stop.color().blue])
            .collect();
        assert_eq!(stops, [[0x18, 0x16, 0x05], [0xd3, 0xd3, 0xd3]]);
    }

    #[test]
    fn nested_raster_pixels_take_the_theme_filter_with_alpha_untouched() {
        // One opaque white pixel and one half-transparent black pixel.
        let png = super::super::super::asset::encode_normalized_png(
            2,
            1,
            &[255, 255, 255, 255, 0, 0, 0, 128],
        )
        .expect("fixture encodes");
        let href = base64::engine::general_purpose::STANDARD.encode(png);
        let lowered = lower(&tree(&format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="1"><image width="2" height="1" href="data:image/png;base64,{href}"/></svg>"#
        )))
        .expect("lowers");
        let xml = lowered.to_string(&usvg::WriteOptions::default());
        let encoded = xml
            .split("data:image/png;base64,")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .map(str::trim)
            .expect("the raster stays a PNG data URL");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64");
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .expect("PNG");
        let mut rgba = vec![0; reader.output_buffer_size().expect("PNG buffer fits")];
        reader.next_frame(&mut rgba).expect("decodes");
        // White is 17.85 per channel through the filter, rounded to #121212;
        // black is 237.15, rounded to #ededed.
        assert_eq!(rgba, [0x12, 0x12, 0x12, 255, 0xed, 0xed, 0xed, 128]);
    }

    #[test]
    fn mask_content_keeps_its_light_mode_luminance() {
        let lowered = lower(&tree(concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4">"##,
            r##"<mask id="m"><rect width="4" height="4" fill="#ffffff"/></mask>"##,
            r##"<rect width="4" height="4" fill="#1e1e1e" mask="url(#m)"/></svg>"##
        )))
        .expect("lowers");
        let xml = lowered.to_string(&usvg::WriteOptions::default());
        assert!(xml.contains("fill=\"#ffffff\""), "{xml}");
        assert!(xml.contains("fill=\"#d3d3d3\""), "{xml}");
    }
}
