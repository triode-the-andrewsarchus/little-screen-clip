//! Page preparation shared by editable PDFs and read-only notebooks.
use super::*;
use little_clip_core::pdf::{get_pdf_info_cancellable, render_pdf_page_preview_cancellable};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PageSource {
    #[default]
    Pdf,
    Notebook,
    RetainedNotebook,
}

pub(super) struct SourceInfo {
    pub pdf: PdfInfo,
    pub warnings: Vec<String>,
    pub retained_notebook: bool,
}
impl From<PdfInfo> for SourceInfo {
    fn from(pdf: PdfInfo) -> Self {
        Self {
            pdf,
            warnings: Vec::new(),
            retained_notebook: false,
        }
    }
}

impl PageSource {
    pub fn read_only(self) -> bool {
        self != Self::Pdf
    }

    pub fn has_region_detail(self) -> bool {
        self != Self::Notebook
    }

    pub fn info(self, path: &Path, cancel: &AtomicBool) -> Result<SourceInfo, String> {
        match self {
            Self::Pdf => get_pdf_info_cancellable(path, cancel).map(SourceInfo::from),
            Self::Notebook | Self::RetainedNotebook => {
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
                    retained_notebook: matches!(info.format.as_str(), "Xournal" | "reMarkable"),
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
            Self::Pdf => render_pdf_page_preview_cancellable(path, page, edge, cancel),
            Self::Notebook | Self::RetainedNotebook => {
                little_clip_core::notebook::render_page(path, page, edge, cancel)
            }
        }
    }
}
