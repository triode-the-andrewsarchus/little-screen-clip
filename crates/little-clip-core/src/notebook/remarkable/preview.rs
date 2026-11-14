//! Whole-source read-only preparation: raw pages and authored notebook archives.
use super::{
    archive,
    background::{self, Templates},
    package::Package,
};
use crate::notebook::{Info, PageSize, container::bounded, embedded_pdf::EmbeddedPdf};
use std::{
    io::{Read, Seek},
    sync::atomic::AtomicBool,
};

/// Reads authored page dimensions and missing-paper notices without rasterizing.
/// Call inside the existing resource-limited disposable notebook worker.
///
/// # Errors
/// Returns source/container/metadata/geometry/cancellation failures.
pub fn inspect(input: &mut (impl Read + Seek), cancel: &AtomicBool) -> Result<Info, String> {
    if let Some(page) = raw_page(input, cancel)? {
        return Ok(Info {
            format: "reMarkable".into(),
            pages: vec![size(&page)],
            warnings: Vec::new(),
        });
    }
    let mut package = Package::open(input, cancel)?;
    let index = archive::from_package(&mut package, cancel)?;
    let templates = Templates::scan(&mut package, cancel)?;
    let warnings = templates.warnings(
        index
            .pages
            .iter()
            .filter_map(|page| page.template.as_deref()),
    );
    let mut pages = Vec::new();
    for reference in &index.pages {
        pages.push(size(&annotation(
            &mut package,
            &index.root,
            reference,
            cancel,
        )?));
    }
    Ok(Info {
        format: "reMarkable".into(),
        pages,
        warnings,
    })
}

/// Composites one authored archive page or renders the one raw page directly.
/// Missing annotations mean an intentionally empty page in the document plan;
/// malformed present annotations are errors, never blank-success fallbacks.
///
/// # Errors
/// Returns invalid-page, missing PDF, malformed template/ink or budget failures.
pub fn render(
    input: &mut (impl Read + Seek),
    page: usize,
    edge: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, u32, u32), String> {
    if let Some(model) = raw_page(input, cancel)? {
        if page != 1 {
            return Err("Raw reMarkable sources contain only one page".into());
        }
        let options = super::output_options(&model, edge)?;
        return super::render_model(&model, options, None, cancel);
    }
    let mut package = Package::open(input, cancel)?;
    let index = archive::from_package(&mut package, cancel)?;
    let reference = page
        .checked_sub(1)
        .and_then(|page| index.pages.get(page))
        .ok_or("reMarkable page is outside the notebook")?;
    let model = annotation(&mut package, &index.root, reference, cancel)?;
    let options = super::output_options(&model, edge)?;
    let background = page_background(&mut package, &index.root, reference, options, cancel)?;
    super::render_model(&model, options, background.as_ref(), cancel)
}

fn page_background<R: Read + Seek>(
    package: &mut Package<R>,
    root: &str,
    reference: &archive::Page,
    options: remarkable_rm::RenderOptions,
    cancel: &AtomicBool,
) -> Result<Option<remarkable_rm::BackgroundImage>, String> {
    Ok(if let Some(number) = reference.pdf_page {
        let bytes = package
            .read(&format!("{root}.pdf"), 128 * 1024 * 1024, cancel)?
            .ok_or("reMarkable PDF background is not included")?;
        let png =
            EmbeddedPdf::new(&bytes)?.page(number as usize, options.width.max(options.height))?;
        Some(background::raster(&png, options)?)
    } else if let Some(name) = &reference.template {
        Templates::scan(package, cancel)?.render(package, name, options, cancel)?
    } else {
        None
    })
}

fn raw_page(
    input: &mut (impl Read + Seek),
    cancel: &AtomicBool,
) -> Result<Option<remarkable_rm::Page>, String> {
    input.rewind().map_err(|error| error.to_string())?;
    let mut header = Vec::new();
    Read::by_ref(input)
        .take(43)
        .read_to_end(&mut header)
        .map_err(|error| error.to_string())?;
    input.rewind().map_err(|error| error.to_string())?;
    if !super::has_signature(&header) {
        return Ok(None);
    }
    super::parse(&bounded(input, super::MAX_PAGE_BYTES, cancel)?, cancel).map(Some)
}

fn annotation<R: Read + Seek>(
    package: &mut Package<R>,
    root: &str,
    reference: &archive::Page,
    cancel: &AtomicBool,
) -> Result<remarkable_rm::Page, String> {
    let bytes = package.read(
        &format!("{root}/{}.rm", reference.id),
        super::MAX_PAGE_BYTES,
        cancel,
    )?;
    match bytes {
        Some(bytes) if !bytes.is_empty() => super::parse(&bytes, cancel),
        _ => Ok(remarkable_rm::Page::default()),
    }
}

fn size(page: &remarkable_rm::Page) -> PageSize {
    let options = remarkable_rm::RenderOptions::for_page(page);
    PageSize {
        width: options.width,
        height: options.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, path::Path};
    #[test]
    fn missing_pdf_and_corrupt_present_annotations_remain_errors() {
        use std::io::{Cursor, Write};
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/examples/notebooks/remarkable/generated/redirected-pdf.rmdoc");
        let mut original = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let cancel = AtomicBool::new(false);
        for corrupt_ink in [false, true] {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for index in 0..original.len() {
                let mut entry = original.by_index(index).unwrap();
                if !corrupt_ink && entry.name().ends_with(".pdf") {
                    continue;
                }
                let name = entry.name().to_owned();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                if corrupt_ink && name.ends_with(".rm") {
                    bytes = b"corrupt ink".to_vec();
                }
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(&bytes).unwrap();
            }
            let mut source = writer.finish().unwrap();
            let error = render(&mut source, 1, 512, &cancel).unwrap_err();
            if !corrupt_ink {
                assert!(error.contains("PDF background is not included"), "{error}");
            } else {
                assert!(inspect(&mut source, &cancel).is_err());
            }
        }
    }

    #[test]
    fn real_and_derived_archives_render_templates_pdf_order_and_empty_insertions() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/notebooks/remarkable");
        let cancel = AtomicBool::new(false);
        for (name, pages, warned) in [
            ("archives/Test.rmdoc", 2, true),
            ("generated/layered-template.rmn", 2, false),
            ("generated/legacy-paper.rmn", 2, false),
            ("generated/redirected-pdf.rmdoc", 3, false),
        ] {
            let mut file = File::open(root.join(name)).unwrap();
            let info = inspect(&mut file, &cancel).unwrap();
            assert_eq!(info.pages.len(), pages);
            assert_eq!(!info.warnings.is_empty(), warned);
            for page in 1..=pages {
                let (png, width, height) = render(&mut file, page, 512, &cancel).unwrap();
                assert!(width <= 512 && height <= 512);
                let image = image::load_from_memory(&png).unwrap().to_rgb8();
                let corner = image.get_pixel(0, 0).0;
                if name.ends_with("redirected-pdf.rmdoc") {
                    let expected = match page {
                        1 => [204, 230, 255],
                        2 => [255, 204, 204],
                        _ => [255; 3],
                    };
                    assert!(
                        corner
                            .into_iter()
                            .zip(expected)
                            .all(|(a, b)| a.abs_diff(b) <= 1),
                        "{name} page {page}: {corner:?}"
                    );
                } else if name.ends_with("layered-template.rmn")
                    || name.ends_with("legacy-paper.rmn") && page == 1
                {
                    assert_eq!(corner, [255, 248, 224], "{name} page {page}");
                }
                if !(name.ends_with("redirected-pdf.rmdoc") && page == 3) {
                    assert!(
                        image
                            .pixels()
                            .filter(|pixel| pixel.0.iter().all(|c| *c < 180))
                            .count()
                            > 50,
                        "Missing actual ink in {name} page {page}"
                    );
                }
                if let Some(dir) = std::env::var_os("REMARKABLE_REVIEW_DIR") {
                    std::fs::create_dir_all(&dir).unwrap();
                    image
                        .save(Path::new(&dir).join(format!(
                            "{}-{page}.png",
                            Path::new(name).file_stem().unwrap().to_string_lossy()
                        )))
                        .unwrap();
                }
            }
            assert!(render(&mut file, 0, 512, &cancel).is_err());
            assert!(render(&mut file, pages + 1, 512, &cancel).is_err());
        }
    }
}
