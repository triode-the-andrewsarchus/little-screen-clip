//! Perceptual similarity gallery layout and zoomable canvas.

pub mod canvas;
pub mod dhash;
pub mod embedding;
pub mod grid;
pub mod scanner;
pub mod siglip;
#[cfg(test)]
pub mod tests;

pub(in crate::file_manager) use canvas::SimilarityCanvas;

/// Gallery view layout presentation modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhotoMode {
    /// Adaptive masonry packing of varying aspect ratios.
    #[default]
    Masonry,
    /// Uniform 1x1 image square canvas clustered by perceptual similarity.
    Similarity,
}

impl PhotoMode {
    pub fn from_str(s: &str) -> Self {
        s.parse().unwrap_or(Self::Masonry)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Masonry => "masonry",
            Self::Similarity => "similarity",
        }
    }
}

impl std::fmt::Display for PhotoMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PhotoMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "similarity" => Self::Similarity,
            _ => Self::Masonry,
        })
    }
}
