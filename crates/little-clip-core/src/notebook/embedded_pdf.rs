//! Owned temporary PDF backgrounds retaining their held/named source identity.
use std::io::Write;

pub(crate) struct EmbeddedPdf {
    file: tempfile::NamedTempFile,
    source: crate::file_source::FileSource,
}
impl EmbeddedPdf {
    pub fn new(bytes: &[u8]) -> Result<Self, String> {
        if !bytes.starts_with(b"%PDF-") {
            return Err("Notebook background is not a PDF".into());
        }
        let mut file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        let source = crate::file_source::FileSource::capture_file(file.as_file())?;
        Ok(Self { file, source })
    }
    pub fn page(&self, page: usize, edge: u32) -> Result<Vec<u8>, String> {
        let result = crate::pdf::render_pdf_page_bounded(self.file.path(), page, edge)?.0;
        self.source.check_file(self.file.as_file())?;
        Ok(result)
    }
    pub fn dimensions(&self) -> Result<(f64, f64), String> {
        let info = crate::pdf::get_pdf_info(self.file.path())?;
        self.source.check_file(self.file.as_file())?;
        let size = info
            .page_size
            .ok_or("Embedded notebook PDF has no page size")?;
        let fields = size.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 3 || fields[1] != "x" {
            return Err("Invalid embedded PDF page size".into());
        }
        let (width, height) = (
            fields[0].parse::<f64>().map_err(|_| "Invalid PDF width")?,
            fields[2].parse::<f64>().map_err(|_| "Invalid PDF height")?,
        );
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err("Invalid embedded PDF dimensions".into());
        }
        Ok((width, height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_raster_uses_the_requested_detail_resolution() {
        let bytes = include_bytes!("../../../../docs/examples/notebooks/xournal/cjk/测试.pdf");
        let pdf = EmbeddedPdf::new(bytes).unwrap();
        let png = pdf.page(1, 2048).unwrap();
        let size = imagesize::blob_size(&png).unwrap();
        assert_eq!(size.width.max(size.height), 2048);
        assert!(size.width <= 2048 && size.height <= 2048);
        assert!(EmbeddedPdf::new(b"not a PDF").is_err());
    }
}
