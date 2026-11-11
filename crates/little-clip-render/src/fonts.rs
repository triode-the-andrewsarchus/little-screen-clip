//! Portable font assets and glyph fallback shared by screen and workbench rendering.

use crate::{FontStyle, ScreenRenderer};
use fontdue::{Font, FontSettings};
use std::sync::{LazyLock, OnceLock};

pub use little_clip_core::font_assets::{CJK_FONT_BYTES, EMOJI_FONT_BYTES};

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
            FontStyle::Regular => self.font_regular.as_deref().or(self.font_bold.as_deref()),
            FontStyle::Bold => self.font_bold.as_deref().or(self.font_regular.as_deref()),
            FontStyle::Italic => self.font_italic.as_deref().or(self.font_regular.as_deref()),
            FontStyle::BoldItalic => self
                .font_bold
                .as_deref()
                .or(self.font_italic.as_deref())
                .or(self.font_regular.as_deref()),
            FontStyle::Mono => self.font_mono.as_deref().or(self.font_regular.as_deref()),
        }
    }

    pub(crate) fn font_for_character<'a>(&'a self, primary: &'a Font, ch: char) -> &'a Font {
        if primary.has_glyph(ch) {
            return primary;
        }
        [
            self.font_regular.as_deref(),
            self.font_bold.as_deref(),
            self.font_italic.as_deref(),
            self.font_mono.as_deref(),
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
