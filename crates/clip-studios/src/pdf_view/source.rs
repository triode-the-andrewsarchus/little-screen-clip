//! Page preparation shared by editable PDFs and read-only notebooks.
use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PageSource {
    #[default]
    Pdf,
    Notebook,
}

pub(super) struct SourceInfo {
    pub pdf: PdfInfo,
    pub warnings: Vec<String>,
}
impl From<PdfInfo> for SourceInfo {
    fn from(pdf: PdfInfo) -> Self {
        Self {
            pdf,
            warnings: Vec::new(),
        }
    }
}

impl PageSource {
    pub fn read_only(self) -> bool {
        self == Self::Notebook
    }

    pub fn info(self, path: &Path, cancel: &AtomicBool) -> Result<SourceInfo, String> {
        match self {
            Self::Pdf => get_pdf_info(path).map(SourceInfo::from),
            Self::Notebook => {
                let info = little_clip_core::notebook::inspect(path, cancel)?;
                let pdf = PdfInfo {
                    page_count: info.pages.len(),
                    title: None,
                    author: None,
                    page_size: info
                        .pages
                        .first()
                        .map(|page| format!("{} × {}", page.width, page.height)),
                };
                Ok(SourceInfo {
                    pdf,
                    warnings: info.warnings,
                })
            }
        }
    }

    pub fn render(
        self,
        path: &Path,
        page: usize,
        edge: u32,
        cancel: &AtomicBool,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        match self {
            Self::Pdf => render_pdf_page_preview(path, page, edge),
            Self::Notebook => little_clip_core::notebook::render_page(path, page, edge, cancel),
        }
    }
}
