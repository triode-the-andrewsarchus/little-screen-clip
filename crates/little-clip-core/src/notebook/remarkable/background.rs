//! Nested template identity and bounded, resource-free background rasterization.
use super::package::Package;
use remarkable_rm::{BackgroundImage, RenderOptions};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Seek},
    sync::atomic::AtomicBool,
};

const MAX_TEMPLATE: usize = 8 * 1024 * 1024;

pub(super) struct Templates(BTreeMap<String, String>);
impl Templates {
    pub fn scan<R: Read + Seek>(
        package: &mut Package<R>,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let members: Vec<_> = package
            .names()
            .filter(|name| name.ends_with(".rmt"))
            .map(str::to_owned)
            .collect();
        if members.len() > 128 {
            return Err("reMarkable template count exceeds the preview limit".into());
        }
        let mut identities = BTreeMap::new();
        for member in members {
            let bytes = package
                .read(&member, MAX_TEMPLATE, cancel)?
                .ok_or("Missing template archive")?;
            let mut template = Package::open(Cursor::new(bytes), cancel)?;
            let json = template
                .read("template.json", 64 * 1024, cancel)?
                .ok_or("Missing template metadata")?;
            let metadata: serde_json::Value =
                serde_json::from_slice(&json).map_err(|error| error.to_string())?;
            for identity in [metadata.get("fileName"), metadata.get("name")]
                .into_iter()
                .flatten()
            {
                let identity = identity
                    .as_str()
                    .filter(|name| !name.is_empty() && name.len() <= 512)
                    .ok_or("Invalid template identity")?;
                if identities
                    .insert(identity.to_owned(), member.clone())
                    .is_some_and(|old| old != member)
                {
                    return Err("Ambiguous reMarkable template identity".into());
                }
            }
        }
        Ok(Self(identities))
    }

    pub fn warnings<'a>(&self, templates: impl Iterator<Item = &'a str>) -> Vec<String> {
        let missing: BTreeSet<_> = templates
            .filter(|name| !self.0.contains_key(*name))
            .collect();
        let mut warnings: Vec<_> = missing
            .iter()
            .take(16)
            .map(|name| {
                format!(
                    "Paper template not included: {}",
                    name.chars().take(100).collect::<String>()
                )
            })
            .collect();
        if missing.len() > 16 {
            warnings.push("Additional paper templates are not included".into());
        }
        warnings
    }

    pub fn render<R: Read + Seek>(
        &self,
        package: &mut Package<R>,
        name: &str,
        options: RenderOptions,
        cancel: &AtomicBool,
    ) -> Result<Option<BackgroundImage>, String> {
        let Some(member) = self.0.get(name) else {
            return Ok(None);
        };
        let bytes = package
            .read(member, MAX_TEMPLATE, cancel)?
            .ok_or("Missing template archive")?;
        let mut template = Package::open(Cursor::new(bytes), cancel)?;
        let image = if let Some(svg) = template.read("template.svg", MAX_TEMPLATE, cancel)? {
            svg_png(&svg, options.width.max(options.height), cancel)?
        } else {
            template
                .read("template.png", MAX_TEMPLATE, cancel)?
                .ok_or("Template archive contains no artwork")?
        };
        raster(&image, options).map(Some)
    }
}

fn svg_png(bytes: &[u8], edge: u32, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    use roxmltree::{Document, ParsingOptions};
    crate::xml_content::validate(bytes, 100_000, 32)?;
    let svg = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let document = Document::parse_with_options(
        svg,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
        },
    )
    .map_err(|error| error.to_string())?;
    if !document.root_element().has_tag_name("svg") {
        return Err("Template artwork is not SVG".into());
    }
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        if matches!(node.tag_name().name(), "image" | "foreignObject" | "script") {
            return Err("Unsupported image/foreign content in SVG paper template".into());
        }
        if node
            .attributes()
            .any(|attribute| attribute.name() == "href" && !attribute.value().starts_with('#'))
        {
            return Err("External SVG template resources are unsupported".into());
        }
    }
    crate::notebook::vector::rasterize(svg, edge, cancel).map(|image| image.0)
}

pub(super) fn raster(bytes: &[u8], options: RenderOptions) -> Result<BackgroundImage, String> {
    let size = imagesize::blob_size(bytes).map_err(|error| error.to_string())?;
    if size
        .width
        .checked_mul(size.height)
        .is_none_or(|pixels| pixels > 16 * 1024 * 1024)
    {
        return Err("Paper background exceeds its pixel budget".into());
    }
    let mut reader = crate::item::image_reader(bytes, None).map_err(|error| error.to_string())?;
    reader.limits(crate::item::raster_preview_limits());
    let image = reader
        .decode()
        .map_err(|error| error.to_string())?
        .resize(
            options.width,
            options.height,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let mut canvas =
        image::RgbaImage::from_pixel(options.width, options.height, image::Rgba([255; 4]));
    let left = (options.width - image.width()) / 2;
    image::imageops::overlay(&mut canvas, &image, i64::from(left), 0);
    // The opaque white canvas makes these bytes valid premultiplied RGBA too.
    Ok(BackgroundImage {
        width: options.width,
        height: options.height,
        rgba: canvas.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn svg_templates_reject_external_resources_and_image_payloads() {
        let cancel = AtomicBool::new(false);
        assert!(svg_png(br#"<svg xmlns="http://www.w3.org/2000/svg"><image href="file:///dev/zero"/></svg>"#, 200, &cancel).is_err());
        assert!(
            svg_png(
                br#"<svg xmlns="http://www.w3.org/2000/svg"><use href="/etc/passwd"/></svg>"#,
                200,
                &cancel
            )
            .is_err()
        );
        assert!(
            svg_png(
                b"<!DOCTYPE svg [<!ENTITY a SYSTEM '/etc/passwd'>]><svg>&a;</svg>",
                200,
                &cancel
            )
            .is_err()
        );
    }
}
