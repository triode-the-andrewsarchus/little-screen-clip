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
    for ext in ["svg", "HEIC", "HeIf", "JxL"] {
        assert!(is_image_path(Path::new(&format!("image.{ext}"))));
    }
    assert!(!is_image_path(Path::new("file.txt")));
}

#[test]
fn jpeg_xl_codestream_and_container_decode_with_limits_and_preserve_alpha() {
    use image::ImageDecoder;
    for (bytes, alpha) in [
        (include_bytes!("fixtures/panels.jxl").as_slice(), false),
        (
            include_bytes!("fixtures/panels-alpha16.jxl").as_slice(),
            true,
        ),
    ] {
        assert_eq!(guess_image_mime(Path::new("photo.JXL")), "image/jxl");
        assert_eq!(image_extension_for_mime("image/jxl"), Some("jxl"));
        assert_eq!(read_image_dimensions(bytes), Some((64, 32)));
        let decoder = image_reader(bytes, Some("image/png"))
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(
            decoder.color_type(),
            if alpha {
                image::ColorType::Rgba16
            } else {
                image::ColorType::Rgb8
            }
        );
        let decoded = DynamicImage::from_decoder(decoder).unwrap().to_rgba8();
        assert_eq!(
            decoded.get_pixel(0, 0).0,
            [30, 120, 210, if alpha { 128 } else { 255 }]
        );
        assert_eq!(decoded.get_pixel(63, 31).0, [180, 60, 40, 255]);
        let thumbnail = ImageThumbnail::generate(bytes, 16, 16).unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (16, 8));
        assert_eq!(
            &thumbnail.premultiplied_rgba[..4],
            if alpha {
                &[15, 60, 105, 128]
            } else {
                &[30, 120, 210, 255]
            }
        );
        let item = ClipItem::from_image_bytes(bytes.to_vec(), "image/jxl").unwrap();
        let ClipPayload::Image {
            path,
            width,
            height,
            mime,
            ..
        } = item.payload
        else {
            panic!("JPEG XL payload")
        };
        assert_eq!((width, height), (64, 32));
        assert_eq!(mime, "image/jxl");
        assert_eq!(path.unwrap().extension().unwrap(), "jxl");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.JXL");
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            ClipItem::from_text(path.to_str().unwrap()).payload,
            ClipPayload::Image {
                width: 64,
                height: 32,
                ..
            }
        ));
        for allocation_limit in [false, true] {
            let mut reader = image_reader(bytes, None).unwrap();
            let mut limits = little_clip_core::item::raster_preview_limits();
            if allocation_limit {
                limits.max_alloc = Some(8);
            } else {
                limits.max_image_width = Some(63);
            }
            reader.limits(limits);
            assert!(reader.decode().is_err());
        }
    }
    for bytes in [b"\xff\x0a".as_slice(), b"\0\0\0\x0cJXL \r\n\x87\n"] {
        assert!(ClipItem::from_image_bytes(bytes.to_vec(), "image/jxl").is_none());
    }
    assert!(
        ClipItem::from_image_bytes(
            include_bytes!("fixtures/gradient.avif").to_vec(),
            "image/jxl"
        )
        .is_none()
    );
}

#[test]
fn jpeg_xl_grayscale_and_float_keep_native_sample_depths() {
    use image::ImageDecoder;
    for (bytes, color) in [
        (
            include_bytes!("fixtures/gray16.jxl").as_slice(),
            image::ColorType::L16,
        ),
        (
            include_bytes!("fixtures/float.jxl").as_slice(),
            image::ColorType::Rgb32F,
        ),
    ] {
        let decoder = image_reader(bytes, None).unwrap().into_decoder().unwrap();
        assert_eq!(decoder.color_type(), color);
        assert_eq!(decoder.dimensions(), (8, 6));
        let decoded = DynamicImage::from_decoder(decoder).unwrap();
        if color == image::ColorType::L16 {
            assert_eq!(decoded.as_luma16().unwrap().get_pixel(0, 0).0, [16384]);
        } else {
            let sample = decoded.as_rgb32f().unwrap().get_pixel(0, 0).0;
            for (actual, expected) in sample.into_iter().zip([0.25, 0.5, 0.75]) {
                assert!((actual - expected).abs() < 0.001, "{sample:?}");
            }
        }
        assert!(ImageThumbnail::generate(bytes, 4, 4).is_some());
    }
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
