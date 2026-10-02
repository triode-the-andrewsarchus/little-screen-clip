//! Portable font assets and glyph fallback shared by screen and workbench rendering.

use crate::{FontStyle, ScreenRenderer};
use fontdue::{Font, FontSettings};
use std::sync::LazyLock;

/// Bundled Noto Sans CJK JP regular face (SIL OFL 1.1).
pub static CJK_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoSansCJK-JP-Regular.ttf");
/// Bundled Noto Emoji regular monochrome face (SIL OFL 1.1).
pub static EMOJI_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoEmoji-Regular.ttf");

// Parse once per process, only when a character needs a bundled fallback.
static FALLBACK_FONTS: LazyLock<Vec<Font>> = LazyLock::new(|| {
    [CJK_FONT_BYTES, EMOJI_FONT_BYTES]
        .into_iter()
        .filter_map(|bytes| {
            Font::from_bytes(
                bytes,
                FontSettings {
                    load_substitutions: false,
                    ..FontSettings::default()
                },
            )
            .map_err(|error| tracing::warn!(%error, "could not load bundled display font"))
            .ok()
        })
        .collect()
});

impl ScreenRenderer {
    pub(crate) fn font_for_style(&self, style: FontStyle) -> Option<&Font> {
        match style {
            FontStyle::Regular => self.font_regular.as_ref().or(self.font_bold.as_ref()),
            FontStyle::Bold => self.font_bold.as_ref().or(self.font_regular.as_ref()),
            FontStyle::Italic => self.font_italic.as_ref().or(self.font_regular.as_ref()),
            FontStyle::BoldItalic => self
                .font_bold
                .as_ref()
                .or(self.font_italic.as_ref())
                .or(self.font_regular.as_ref()),
            FontStyle::Mono => self.font_mono.as_ref().or(self.font_regular.as_ref()),
        }
    }

    pub(crate) fn font_for_character<'a>(&'a self, primary: &'a Font, ch: char) -> &'a Font {
        if primary.has_glyph(ch) {
            return primary;
        }
        [
            self.font_regular.as_ref(),
            self.font_bold.as_ref(),
            self.font_italic.as_ref(),
            self.font_mono.as_ref(),
        ]
        .into_iter()
        .flatten()
        .find(|font| font.has_glyph(ch))
        .or_else(|| FALLBACK_FONTS.iter().find(|font| font.has_glyph(ch)))
        .unwrap_or(primary)
    }
}
