// SPDX-License-Identifier: MPL-2.0
//! Preserve d2parser.ParseError identity until the caller chooses its transport.
use d2_rs_core::ast;

#[derive(Debug)]
pub(super) enum SourceCompileError {
    Diagnostics(Option<Vec<ast::Error>>),
    Other(String),
}

impl SourceCompileError {
    pub(super) fn public_message(self) -> String {
        match self {
            // d2wasm.Compile json.Marshal(pe.Errors), including encoding/json's
            // default HTML and JavaScript line-separator escaping.
            Self::Diagnostics(errors) => serde_json::to_string(&errors)
                .expect("source diagnostics contain only serializable strings and ranges")
                .replace('<', "\\u003c")
                .replace('>', "\\u003e")
                .replace('&', "\\u0026")
                .replace('\u{2028}', "\\u2028")
                .replace('\u{2029}', "\\u2029"),
            Self::Other(message) => message,
        }
    }
}

impl std::fmt::Display for SourceCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Diagnostics(errors) => {
                for (index, error) in errors.iter().flatten().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    write!(f, "{error}")?;
                }
                Ok(())
            }
            Self::Other(message) => f.write_str(message),
        }
    }
}
impl From<String> for SourceCompileError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}
impl From<d2_rs_language::parser::ParseError> for SourceCompileError {
    fn from(error: d2_rs_language::parser::ParseError) -> Self {
        match error.fault {
            Some(fault) => Self::Other(fault.to_string()),
            None => Self::Diagnostics(error.errors),
        }
    }
}
impl From<d2_rs_language::compiler::GraphCompileError> for SourceCompileError {
    fn from(error: d2_rs_language::compiler::GraphCompileError) -> Self {
        match error {
            d2_rs_language::compiler::GraphCompileError::InvalidAttributes(errors) => {
                Self::Diagnostics(Some(errors))
            }
            error => Self::Other(error.to_string()),
        }
    }
}
