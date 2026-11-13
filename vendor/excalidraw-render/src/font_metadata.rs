//! Excalidraw's `FONT_METADATA` table, transcribed from the pinned release.
//!
//! The reference renderer declares one metric set per source font family and
//! reads it in two places compatibility revision 1 reproduces: `getVerticalOffset`
//! positions every text baseline from `unitsPerEm`, `ascender`, and `descender`
//! (#105), and the restore path's `getLineHeight` fills a text element's missing
//! `lineHeight` from the family's `lineHeight` when the element has no height
//! (#108). Both read the element's *source* family, independent of the face that
//! shapes its glyphs, so the table lives outside the render module where
//! normalization can reach it too.

/// The family metadata metrics Excalidraw's `FONT_METADATA` declares for one
/// source font family: what the reference renderer reads when it positions text
/// and derives a missing line height, independent of the face that shapes the
/// glyphs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FamilyMetrics {
    pub(crate) units_per_em: u16,
    pub(crate) ascender: i16,
    pub(crate) descender: i16,
    /// The family's metadata line height, as a multiple of the font size.
    pub(crate) line_height: f64,
}

impl FamilyMetrics {
    /// Excalifont's metrics: the reference renderer's fallback for a family it
    /// has not registered, and the family every unrecognized source family
    /// normalizes to.
    pub(crate) const EXCALIFONT: Self = Self {
        units_per_em: 1_000,
        ascender: 886,
        descender: -374,
        line_height: 1.25,
    };

    /// Excalidraw v0.18.1 `FONT_METADATA[family].metrics` for every family the
    /// renderer resolves. Every bundled face carries the same vertical metrics
    /// in its `hhea` table; Helvetica does not ship and is shaped with
    /// Liberation Sans, whose own metrics differ, so revision 1 positions
    /// Helvetica text by these values rather than the substitute's (#105).
    pub(crate) const fn for_source_family(source_family: i32) -> Option<Self> {
        let (units_per_em, ascender, descender, line_height) = match source_family {
            // Virgil and Excalifont share one metric set; Segoe UI Emoji
            // (1000) reuses it upstream.
            1 | 5 | 1_000 => return Some(Self::EXCALIFONT),
            // Helvetica: the local-only family Liberation Sans substitutes.
            2 => (2_048, 1_577, -471, 1.15),
            // Cascadia Code.
            3 => (2_048, 1_900, -480, 1.2),
            // Nunito.
            6 => (1_000, 1_011, -353, 1.35),
            // Lilita One.
            7 => (1_000, 923, -220, 1.15),
            // Comic Shanns.
            8 => (1_000, 750, -250, 1.25),
            // Liberation Sans.
            9 => (2_048, 1_854, -434, 1.15),
            // Xiaolai, the CJK fallback.
            100 => (1_000, 880, -144, 1.15),
            _ => return None,
        };
        Some(Self {
            units_per_em,
            ascender,
            descender,
            line_height,
        })
    }

    /// Canvas centers the family's ascender/descender box within the first
    /// line box, so the first baseline sits at
    /// `line_step / 2 + font_size * (ascender + descender) / units_per_em / 2`.
    pub(crate) fn first_baseline(self, font_size: f64, line_step: f64) -> f64 {
        let scale = font_size / f64::from(self.units_per_em);
        line_step / 2.0 + f64::from(self.ascender + self.descender) * scale / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::FamilyMetrics;

    #[test]
    fn the_table_covers_every_normalized_family_and_excalifont_is_the_fallback() {
        // Excalidraw v0.18.1 FONT_METADATA[family].metrics.lineHeight.
        for (family, line_height) in [
            (1, 1.25),
            (2, 1.15),
            (3, 1.2),
            (5, 1.25),
            (6, 1.35),
            (7, 1.15),
            (8, 1.25),
            (9, 1.15),
            (100, 1.15),
            (1_000, 1.25),
        ] {
            assert_eq!(
                FamilyMetrics::for_source_family(family).map(|metrics| metrics.line_height),
                Some(line_height),
                "family {family}"
            );
        }
        assert_eq!(FamilyMetrics::for_source_family(4), None);
        assert_eq!(
            FamilyMetrics::for_source_family(5),
            Some(FamilyMetrics::EXCALIFONT)
        );
    }
}
