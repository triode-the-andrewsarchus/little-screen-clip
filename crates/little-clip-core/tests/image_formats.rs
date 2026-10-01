use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use little_clip_core::item::{
    ClipItem, ClipPayload, ImageThumbnail, guess_image_mime, image_extension_for_mime,
    image_reader, is_image_path, read_image_dimensions,
};
use std::{io::Cursor, path::Path};

#[test]
fn all_enabled_formats_are_classified_case_insensitively() {
    for format in ImageFormat::all() {
        if format.reading_enabled() || format == ImageFormat::Avif {
            for ext in format.extensions_str() {
                assert!(
                    is_image_path(Path::new(&format!("image.{}", ext.to_uppercase()))),
                    "{ext}"
                );
            }
        }
    }
    for ext in ["svg", "HEIC", "HeIf"] {
        assert!(is_image_path(Path::new(&format!("image.{ext}"))));
    }
    assert!(!is_image_path(Path::new("file.txt")));
}

#[test]
fn additional_raster_formats_decode_and_produce_clipboard_thumbnails() {
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 6, Rgb([128, 64, 32])));
    for (format, ext) in [
        (ImageFormat::Bmp, "bmp"),
        (ImageFormat::OpenExr, "exr"),
        (ImageFormat::Farbfeld, "ff"),
        (ImageFormat::Gif, "gif"),
        (ImageFormat::Hdr, "hdr"),
        (ImageFormat::Ico, "ico"),
        (ImageFormat::Jpeg, "jpeg"),
        (ImageFormat::Png, "png"),
        (ImageFormat::Pnm, "pnm"),
        (ImageFormat::Qoi, "qoi"),
        (ImageFormat::Tga, "tga"),
        (ImageFormat::Tiff, "tif"),
        (ImageFormat::WebP, "webp"),
    ] {
        let mut encoded = Cursor::new(Vec::new());
        let source = match format {
            ImageFormat::OpenExr | ImageFormat::Hdr => DynamicImage::ImageRgb32F(image.to_rgb32f()),
            ImageFormat::Farbfeld => DynamicImage::ImageRgba16(image.to_rgba16()),
            ImageFormat::Ico => DynamicImage::ImageRgba8(image.to_rgba8()),
            _ => image.clone(),
        };
        source.write_to(&mut encoded, format).unwrap();
        let bytes = encoded.into_inner();
        let mime = guess_image_mime(Path::new(&format!("image.{ext}")));
        assert_eq!(
            image_extension_for_mime(&mime),
            Some(format.extensions_str()[0])
        );
        let decoded = image_reader(&bytes, Some(&mime)).unwrap().decode().unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 6), "{ext}");
        let item = ClipItem::from_image_bytes(bytes.clone(), &mime)
            .unwrap_or_else(|| panic!("{ext}: {mime}"));
        let ClipPayload::Image {
            width,
            height,
            path,
            ..
        } = item.payload
        else {
            panic!("{ext}")
        };
        assert_eq!((width, height), (8, 6), "{ext}");
        assert_eq!(
            path.unwrap().extension().unwrap(),
            format.extensions_str()[0]
        );
        let thumb = ImageThumbnail::generate_with_mime(&bytes, Some(&mime), 4, 4).unwrap();
        assert!(thumb.width <= 4 && thumb.height <= 4);
    }
}

#[test]
fn heic_and_avif_decode_from_bytes_without_startup_initialization() {
    for (bytes, mime, ext) in [
        (
            include_bytes!("fixtures/gradient.heic").as_slice(),
            "image/heic",
            "heic",
        ),
        (
            include_bytes!("fixtures/gradient.heic").as_slice(),
            "image/heif",
            "heif",
        ),
        (
            include_bytes!("fixtures/gradient.avif").as_slice(),
            "image/avif",
            "avif",
        ),
    ] {
        assert_eq!(read_image_dimensions(bytes), Some((8, 6)));
        let decoded = image_reader(bytes, None)
            .unwrap()
            .decode()
            .unwrap()
            .to_rgb8();
        assert!(decoded.get_pixel(0, 0)[0] > decoded.get_pixel(0, 0)[2]);
        assert!(decoded.get_pixel(0, 5)[2] > decoded.get_pixel(0, 5)[0]);
        assert!(ImageThumbnail::generate(bytes, 4, 4).is_some());
        assert!(ClipItem::from_image_bytes(bytes.to_vec(), mime).is_some());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("image.{ext}"));
        std::fs::write(&path, bytes).unwrap();
        let ClipPayload::Image {
            width,
            height,
            mime: actual,
            thumbnail,
            ..
        } = ClipItem::from_text(path.to_str().unwrap()).payload
        else {
            panic!("{ext}")
        };
        assert_eq!((width, height), (8, 6));
        assert_eq!(actual, mime);
        assert!(thumbnail.is_some());
    }
}

#[test]
fn invalid_heif_is_rejected_and_signatures_override_hints() {
    assert!(
        ClipItem::from_image_bytes(b"\0\0\0\x18ftypheic\0\0\0\0heicmif1".to_vec(), "image/heic")
            .is_none()
    );
    let bytes = include_bytes!("fixtures/gradient.avif");
    assert!(
        image_reader(bytes, Some("image/png"))
            .unwrap()
            .decode()
            .is_ok()
    );
}
