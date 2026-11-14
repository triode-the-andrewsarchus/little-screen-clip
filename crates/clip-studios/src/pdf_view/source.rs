//! Page preparation shared by editable PDFs and read-only notebooks.
use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PageSource {
    #[default]
    Pdf,
    Notebook,
}

impl PageSource {
    pub fn read_only(self) -> bool {
        self == Self::Notebook
    }

    pub fn info(self, path: &Path, cancel: &AtomicBool) -> Result<PdfInfo, String> {
        match self {
            Self::Pdf => get_pdf_info(path),
            Self::Notebook => {
                let info = little_clip_core::notebook::inspect(path, cancel)?;
                Ok(PdfInfo {
                    page_count: info.pages.len(),
                    title: None,
                    author: None,
                    page_size: info
                        .pages
                        .first()
                        .map(|page| format!("{} × {}", page.width, page.height)),
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
