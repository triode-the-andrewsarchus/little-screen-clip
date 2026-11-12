//! Diagram syntax identities shared by Markdown and standalone file viewers.
use std::path::Path;

/// A diagram syntax with an integrated native renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Pikchr,
    Svgbob,
}
impl Format {
    /// Recognizes a Markdown fence's language, without executing its attributes.
    pub fn from_language(language: &str) -> Option<Self> {
        match language
            .split_whitespace()
            .next()?
            .to_ascii_lowercase()
            .as_str()
        {
            "pikchr" => Some(Self::Pikchr),
            "svgbob" | "bob" => Some(Self::Svgbob),
            _ => None,
        }
    }

    /// Recognizes unambiguous standalone extensions.
    ///
    /// Ambiguous `.bob` files need content classification and are not claimed here.
    pub fn from_path(path: &Path) -> Option<Self> {
        Self::from_extension(path.extension()?.to_str()?)
    }

    /// Recognizes an unambiguous extension without treating fence aliases as paths.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "pikchr" => Some(Self::Pikchr),
            "svgbob" => Some(Self::Svgbob),
            _ => None,
        }
    }

    /// Returns the syntax's display name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pikchr => "Pikchr",
            Self::Svgbob => "SvgBob",
        }
    }
}
