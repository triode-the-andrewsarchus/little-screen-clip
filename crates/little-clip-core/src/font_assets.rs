//! Portable fallback font bytes shared by desktop and raster presentation.
//!
//! Font parsing and glyph caches belong to each renderer. The original assets
//! and their license files remain in the canonical presentation asset directory.

/// Bundled Noto Sans CJK JP regular face (SIL OFL 1.1).
pub static CJK_FONT_BYTES: &[u8] =
    include_bytes!("../../little-clip-render/assets/fonts/NotoSansCJK-JP-Regular.ttf");
/// Bundled Noto Emoji regular monochrome face (SIL OFL 1.1).
pub static EMOJI_FONT_BYTES: &[u8] =
    include_bytes!("../../little-clip-render/assets/fonts/NotoEmoji-Regular.ttf");
