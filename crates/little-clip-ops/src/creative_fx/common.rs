//! Shared Creative FX types and masking configuration.

use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CreativeFxType {
    LensAberration,
    Trichromatic,
    PixelSort,
    CmykHalftone,
    DitherPalette,
    Ntsc,
    Structure,
}

impl CreativeFxType {
    pub fn all() -> &'static [CreativeFxType] {
        &[
            CreativeFxType::LensAberration,
            CreativeFxType::Trichromatic,
            CreativeFxType::PixelSort,
            CreativeFxType::CmykHalftone,
            CreativeFxType::DitherPalette,
            CreativeFxType::Ntsc,
            CreativeFxType::Structure,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::LensAberration => "Lens Aberration",
            Self::Trichromatic => "Trichromatic Camera",
            Self::PixelSort => "Pixel Sorting",
            Self::CmykHalftone => "CMYK Halftone",
            Self::DitherPalette => "Dither & Retro Palette",
            Self::Ntsc => "NTSC & VHS Video",
            Self::Structure => "Structure",
        }
    }

    pub fn subtitle(&self) -> &'static str {
        match self {
            Self::LensAberration => "Optical radial color fringing",
            Self::Trichromatic => "3-channel alignment & drift",
            Self::PixelSort => "Algorithmic scanline sorting",
            Self::CmykHalftone => "Offset press misregistration & dots",
            Self::DitherPalette => "Ordered dithering & classic hardware",
            Self::Ntsc => "Analog broadcast & magnetic tape emulation",
            Self::Structure => "Multiscale texture & edge protection",
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::LensAberration => "OPTICAL",
            Self::Trichromatic => "MULTI-CHANNEL",
            Self::PixelSort => "GLITCH ART",
            Self::CmykHalftone => "PRINT & PRESS",
            Self::DitherPalette => "RETRO HARDWARE",
            Self::Ntsc => "ANALOG VIDEO",
            Self::Structure => "DETAIL & TEXTURE",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::LensAberration => {
                "Emulates real-world optical glass dispersion. Aberration increases radially towards \
                 the image edges with customizable strength, falloff, and color balance (blue/yellow vs red/blue)."
            }
            Self::Trichromatic => {
                "Emulates historic three-color separation plate photography (e.g. Prokudin-Gorsky) \
                 or experimental OKLab perceptual drift. Allows independent X/Y offsets and zoom per channel \
                 with automatic intersection cropping."
            }
            Self::PixelSort => {
                "Sorts pixels along scanline segments bounded by luminance, hue, or saturation thresholds, \
                 creating kinetic melting, streaks, and datamosh textures."
            }
            Self::CmykHalftone => {
                "Simulates vintage multi-color offset press lithography and newsprint comics. \
                 Renders Cyan, Magenta, Yellow, and Key dot screens at traditional rosette angles \
                 with mechanical plate misregistration."
            }
            Self::DitherPalette => {
                "Applies ordered Bayer matrices, blue-noise, or Floyd-Steinberg error diffusion \
                 quantized to iconic retro hardware palettes (Game Boy, Macintosh 1-bit, CGA, PICO-8, etc.) \
                 with configurable pixel scaling."
            }
            Self::Ntsc => {
                "Accurately emulates analog NTSC composite color television transmission and VHS cassette \
                 recording artifacts, including RF static snow, luma smear, chroma bleed, tracking bars, \
                 and head-switching jitter."
            }
            Self::Structure => {
                "Selectively amplifies naturally occurring fine-to-medium surface textures \
                 (fabric, foliage, stone, skin detail) while protecting high-contrast object \
                 boundaries from conventional sharpening halos and ringing. Supports negative \
                 values for controlled edge-preserving texture smoothing."
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ─────────────────────────────────────────────────────────────────────────────
// Creative FX Masking Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Source used to spatially mask a creative effect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreativeFxMaskSource {
    /// Luminance from an AI-generated depth map.
    #[default]
    DepthMap,
    /// Alpha from a background-removal result.
    BackgroundRemovalAlpha,
}

/// Configuration for spatially masking creative effects.
///
/// The historical type name is retained for API and serialized-data compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DepthMaskConfig {
    /// Whether masking is enabled for this effect.
    pub enabled: bool,
    /// The mutually exclusive source of mask values.
    #[serde(default)]
    pub source: CreativeFxMaskSource,
    /// Invert mask polarity.
    pub invert: bool,
    /// Depth threshold cut-off (0.0 to 1.0). When 0.0, the full grayscale depth gradient is used continuously.
    pub threshold: f32,
    /// Edge feather softness around the depth threshold (0.0 to 1.0).
    pub feather: f32,
}

impl Default for DepthMaskConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source: CreativeFxMaskSource::DepthMap,
            invert: false,
            threshold: 0.0,
            feather: 0.25,
        }
    }
}
