//! Parser and renderer for reMarkable `.rm` notebook page files.
//!
//! Notebook pages on the tablet are stored as a per-page binary format
//! (versions 3 through 6). This crate decodes the bytes into stroke data
//! and rasterizes the strokes to PNG.
//!
//! The wire format is documented in `RM_FORMAT_V6_SPEC.md` at the workspace
//! root. The implementation matches that document section-by-section.
//!
//! Phase status: v6 parser implemented; PNG rendering is Phase 4 step 11.

pub mod blocks;
pub mod crdt;
pub mod document;
pub mod error;
pub mod primitives;
pub mod render;
pub mod scene;
pub mod tag;
#[cfg(feature = "templates")]
pub mod templates;

pub use blocks::{BlockHeader, BlockType};
pub use crdt::{CrdtId, LwwValue};
pub use document::{Layer, Page, parse_page};
pub use error::ParseError;
pub use render::{
    BackgroundImage, DEFAULT_HEIGHT, DEFAULT_WIDTH, RenderError, RenderOptions, render_page,
    render_page_with_background,
};
pub use scene::line::{Line, Pen, PenColor, Point};
#[cfg(feature = "templates")]
pub use templates::{TemplateError, decode_template};

mod render_text;
