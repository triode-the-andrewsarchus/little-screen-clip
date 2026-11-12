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

/// Bundled metric-compatible Times, Helvetica and Courier families (SIL OFL 1.1).
pub static GRAPHVIZ_FONT_BYTES: &[&[u8]] = &[
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Arimo-Italic[wght].ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Arimo[wght].ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Cousine-Bold.ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Cousine-BoldItalic.ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Cousine-Italic.ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Cousine-Regular.ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Tinos-Bold.ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Tinos-BoldItalic.ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Tinos-Italic.ttf"),
    include_bytes!("../../little-clip-render/assets/fonts/graphviz/Tinos-Regular.ttf"),
];
