//! Memory-only draw.io reader and SVG renderer extracted from document-svg.
//!
//! No filesystem, network, font discovery or external stencil resolver is used.
//! The host remains responsible for process isolation and final SVG/image budgets.

mod convert;
mod drawio;
mod error;
pub mod ir;
mod ooxml;
mod reverse;
pub mod svg;

pub use error::{Error, Result};

/// Resource budgets applied while parsing a document.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub source_bytes: usize,
    pub inflated_page_bytes: usize,
    pub xml_events: usize,
    pub pages: usize,
    pub svg_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            source_bytes: 256 * 1024,
            inflated_page_bytes: 2 * 1024 * 1024,
            xml_events: 16_384,
            pages: 16,
            svg_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Rendered pages plus nonfatal compatibility diagnostics.
#[derive(Debug)]
pub struct Rendered {
    pub pages: Vec<String>,
    pub warnings: Vec<String>,
}

/// Decode a plain or compressed mxfile, or a bare mxGraphModel.
pub fn render(source: &[u8], limits: Limits) -> Result<Rendered> {
    struct Sink {
        pages: Vec<String>,
        remaining: usize,
    }
    impl convert::PageConsumer for Sink {
        fn consume(&mut self, page: ir::Page) -> Result<()> {
            use std::io::Write;
            struct BoundedSvg {
                bytes: Vec<u8>,
                limit: usize,
            }
            impl Write for BoundedSvg {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                        return Err(std::io::Error::other("drawio SVG output exceeds budget"));
                    }
                    self.bytes.extend_from_slice(bytes);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let mut output = BoundedSvg {
                bytes: Vec::new(),
                limit: self.remaining,
            };
            svg::write_page(&page, &mut output, svg::SvgOptions::default())?;
            self.remaining -= output.bytes.len();
            self.pages.push(
                String::from_utf8(output.bytes)
                    .map_err(|error| Error::InvalidInput(error.to_string()))?,
            );
            Ok(())
        }
    }
    let options = convert::ConvertOptions {
        max_input_bytes: limits.source_bytes as u64,
        max_zip_entry_bytes: limits.inflated_page_bytes as u64,
        max_xml_events: limits.xml_events,
        max_pages: limits.pages,
        embed_drawio_source: false,
    };
    let mut sink = Sink {
        pages: Vec::new(),
        remaining: limits.svg_bytes,
    };
    let warnings = drawio::convert(source, &options, &mut sink)?;
    Ok(Rendered {
        pages: sink.pages,
        warnings,
    })
}
