//! Portable font assets and glyph fallback shared by screen and workbench rendering.

use crate::{FontStyle, ScreenRenderer};
use fontdue::{Font, FontSettings};
use std::sync::{LazyLock, OnceLock};

/// Bundled Noto Sans CJK JP regular face (SIL OFL 1.1).
pub static CJK_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoSansCJK-JP-Regular.ttf");
/// Bundled Noto Emoji regular monochrome face (SIL OFL 1.1).
pub static EMOJI_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoEmoji-Regular.ttf");

// Coverage is cheap to inspect; parse glyph geometry only for a font actually
// needed by a character. In particular, emoji does not initialize the CJK face.
struct FallbackFont {
    coverage: ttf_parser::Face<'static>,
    bytes: &'static [u8],
    parsed: OnceLock<Option<Font>>,
}

static FALLBACK_FONTS: LazyLock<Vec<FallbackFont>> = LazyLock::new(|| {
    [CJK_FONT_BYTES, EMOJI_FONT_BYTES]
        .into_iter()
        .filter_map(|bytes| {
            ttf_parser::Face::parse(bytes, 0)
                .ok()
                .map(|coverage| FallbackFont {
                    coverage,
                    bytes,
                    parsed: OnceLock::new(),
                })
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
        .or_else(|| {
            FALLBACK_FONTS.iter().find_map(|fallback| {
                fallback.coverage.glyph_index(ch)?;
                fallback
                    .parsed
                    .get_or_init(|| {
                        Font::from_bytes(
                            fallback.bytes,
                            FontSettings {
                                load_substitutions: false,
                                ..FontSettings::default()
                            },
                        )
                        .map_err(
                            |error| tracing::warn!(%error, "could not load bundled display font"),
                        )
                        .ok()
                    })
                    .as_ref()
            })
        })
        .unwrap_or(primary)
    }
}
