//! Bundled syntax construction shared by raster and interactive source views.
use syntect::parsing::{SyntaxDefinition, SyntaxSet};

/// Builds the shared syntax inventory with the caller's line-ending policy.
///
/// Raster cards tokenize lines without terminators; desktop source layouts retain
/// newlines. The inventory is identical so unsupported renders still show code.
pub fn build_syntax_set(newlines: bool) -> SyntaxSet {
    let defaults = if newlines {
        SyntaxSet::load_defaults_newlines()
    } else {
        SyntaxSet::load_defaults_nonewlines()
    };
    let mut builder = defaults.into_builder();
    for definition in [
        little_clip_core::syntax::CONFIG_SUBLIME_SYNTAX,
        little_clip_core::syntax::TYPESCRIPT_SUBLIME_SYNTAX,
    ]
    .into_iter()
    .chain(
        little_clip_core::syntax::DIAGRAM_SYNTAX_DEFINITIONS
            .iter()
            .copied(),
    ) {
        match SyntaxDefinition::load_from_str(definition, newlines, None) {
            Ok(syntax) => builder.add(syntax),
            Err(error) => tracing::warn!(%error, "Could not load bundled syntax definition"),
        }
    }
    builder.build()
}
