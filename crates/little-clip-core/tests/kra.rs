use little_clip_core::{
    item::{ClipItem, ClipPayload, ImageThumbnail, image_reader},
    kra_image,
};
use std::io::{Cursor, Read, Write};

const LAYERED: &[u8] = include_bytes!("fixtures/layered.kra");

fn changed(name: &str, replacement: Option<&[u8]>) -> Vec<u8> {
    let mut source = zip::ZipArchive::new(Cursor::new(LAYERED)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut file = source.by_index(index).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        let content = if file.name() == name {
            replacement
        } else {
            Some(bytes.as_slice())
        };
        if let Some(content) = content {
            output
                .start_file(file.name(), zip::write::SimpleFileOptions::default())
                .unwrap();
            output.write_all(content).unwrap();
        }
    }
    output.finish().unwrap().into_inner()
}

#[test]
fn native_krita_composites_keep_pixels_sources_and_layer_metadata() {
    for bytes in [LAYERED, include_bytes!("fixtures/flat.kra")] {
        let image = image_reader(bytes, Some("image/jpeg"))
            .unwrap()
            .decode()
            .unwrap()
            .to_rgba8();
        assert_eq!(image.dimensions(), (64, 32));
        let (left, right) = if bytes == LAYERED {
            ([30, 120, 210, 255], [180, 60, 40, 255])
        } else {
            ([90, 170, 70, 255], [90, 170, 70, 255])
        };
        assert_eq!(image.get_pixel(0, 0).0, left);
        assert_eq!(image.get_pixel(63, 31).0, right);
        let item = ClipItem::from_image_bytes(bytes.to_vec(), "application/x-krita").unwrap();
        assert!(matches!(
            item.payload,
            ClipPayload::Image {
                width: 64,
                height: 32,
                ..
            }
        ));
        let thumb = ImageThumbnail::generate(bytes, 16, 16).unwrap();
        assert_eq!((thumb.width, thumb.height), (16, 8));
    }
    let summary = kra_image::inspect(LAYERED).unwrap();
    assert_eq!(summary.dimensions, (64, 32));
    assert_eq!(summary.color_mode, "RGBA");
    assert!(summary.profile.unwrap().contains("sRGB"));
    assert_eq!(summary.layers.len(), 4);
    assert!(summary.layers[0].group);
    let hidden = &summary.layers[2];
    assert_eq!(hidden.name, "Hidden tint α");
    assert_eq!(hidden.depth, 1);
    assert_eq!(hidden.offset, Some((-8, -4)));
    assert_eq!(hidden.bounds, None);
    assert_eq!(hidden.opacity, Some(127));
    assert_eq!(hidden.visible, Some(false));
    assert_eq!(hidden.blend_name(), "Multiply");
    assert_eq!(summary.layers[3].depth, 0);
}

#[test]
fn missing_optional_layer_fields_remain_unknown() {
    let xml = br#"<DOC><IMAGE width="64" height="32" mime="application/x-kra"><layers><layer name="Mask" nodetype="filtermask"/></layers></IMAGE></DOC>"#;
    let summary = kra_image::inspect(&changed("maindoc.xml", Some(xml))).unwrap();
    let layer = &summary.layers[0];
    assert_eq!(layer.opacity, None);
    assert_eq!(layer.visible, None);
    assert_eq!(layer.offset, None);
    assert_eq!(layer.bounds, None);
    assert_eq!(layer.kind.as_deref(), Some("Filter mask"));
}

#[test]
fn native_krita_limits_and_archive_identity_are_enforced() {
    let mut reader = image_reader(LAYERED, None).unwrap();
    let mut limits = little_clip_core::item::raster_preview_limits();
    limits.max_image_width = Some(32);
    reader.limits(limits);
    assert!(reader.decode().is_err());
    assert!(kra_image::composite(&changed("mimetype", Some(b"image/openraster"))).is_err());
    let preview_only = changed("mergedimage.png", None);
    assert!(image_reader(&preview_only, None).is_err());
    assert!(ImageThumbnail::generate(&preview_only, 16, 16).is_some());
    assert!(kra_image::composite(&changed("mergedimage.png", Some(b"not a PNG"))).is_err());
    assert!(kra_image::inspect(&changed("maindoc.xml", Some(b"<DOC><IMAGE"))).is_err());
    let entity = br#"<!DOCTYPE DOC [<!ENTITY secret SYSTEM "file:///etc/passwd">]><DOC><IMAGE width="64" height="32" mime="application/x-kra" profile="&secret;"/></DOC>"#;
    assert!(kra_image::inspect(&changed("maindoc.xml", Some(entity))).is_err());
    let oversized = vec![b' '; 2 * 1024 * 1024 + 1];
    assert!(kra_image::inspect(&changed("maindoc.xml", Some(&oversized))).is_err());
    let mut duplicate = LAYERED.to_vec();
    for at in (0..duplicate.len() - 11).filter(|&i| LAYERED.get(i..i + 11) == Some(b"preview.png"))
    {
        duplicate[at..at + 11].copy_from_slice(b"maindoc.xml");
    }
    assert!(kra_image::composite(&duplicate).is_err());
    let mut excessive = LAYERED.to_vec();
    let end = excessive
        .windows(4)
        .rposition(|s| s == b"PK\x05\x06")
        .unwrap();
    excessive[end + 10..end + 12].copy_from_slice(&9000u16.to_le_bytes());
    assert!(kra_image::composite(&excessive).is_err());
}
