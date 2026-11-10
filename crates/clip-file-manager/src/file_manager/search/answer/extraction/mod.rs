//! Declarative, data-only extraction. No executable names or paths are accepted.
mod document_fields;
pub(in crate::file_manager::search) mod frontmatter;
mod lines;
mod review;
mod sandbox;
mod session;
pub(in crate::file_manager::search::answer) mod tools;
use little_clip_core::process::CommandRequest;
use serde::{Deserialize, Serialize};
pub(in crate::file_manager::search) use session::{Input, SharedPlans, try_answer};
use std::{
    io::{Read, Write},
    sync::atomic::AtomicBool,
    time::Duration,
};

// A 512 KiB source can expand sixfold when JSON escapes control characters.
pub(super) const MAX_INPUT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::file_manager::search) enum Plan {
    FrontmatterField { schema: String, path: Vec<String> },
    LabelledField { schema: String, path: Vec<String> },
}

pub(in crate::file_manager::search) enum Attempt {
    Answer(super::Answer),
    Escalate(String),
    Skip,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::file_manager::search) enum Request {
    Inspect {
        source: String,
    },
    InspectDocument {
        source: String,
    },
    Execute {
        source: String,
        plan: Plan,
    },
    Search {
        sources: Vec<super::Source>,
        spec: tools::Search,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::file_manager::search) enum Response {
    Header {
        header: Option<frontmatter::Header>,
    },
    Field {
        value: Option<frontmatter::Field>,
        evidence: String,
        location: String,
    },
    Matches {
        spans: Vec<tools::Span>,
        limited: bool,
    },
}

pub(in crate::file_manager::search) fn call(
    request: &Request,
    cancel: &AtomicBool,
) -> Result<Response, String> {
    let input = serde_json::to_vec(request).map_err(|_| "Invalid extraction request")?;
    if input.len() > MAX_INPUT {
        return Err("Extraction request exceeds its byte budget".into());
    }
    let output = CommandRequest::new("/proc/self/exe")
        .die_with_parent()
        .arg("--internal-file-extraction-plan")
        .stdin_bytes(input)
        .timeout(Duration::from_secs(3))
        .own_process_group()
        .max_output_bytes(64 * 1024)
        .run_cancellable(cancel)
        .map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout_truncated {
        return Err("Isolated extraction worker failed".into());
    }
    serde_json::from_slice::<Result<Response, String>>(&output.stdout)
        .map_err(|_| "Invalid extraction response")?
}

fn execute(request: Request) -> Result<Response, String> {
    match request {
        Request::Inspect { source } => Ok(Response::Header {
            header: frontmatter::inspect(&source)?,
        }),
        Request::InspectDocument { source } => Ok(Response::Header {
            header: inspect_document(&source)?,
        }),
        Request::Execute { source, plan } => execute_field(&source, plan),
        Request::Search { sources, spec } => tools::execute(&sources, &spec),
    }
}

fn inspect_document(source: &str) -> Result<Option<frontmatter::Header>, String> {
    match lines::inspect(source)? {
        Some(header) => Ok(Some(header)),
        None => document_fields::inspect(source),
    }
}

fn execute_field(source: &str, plan: Plan) -> Result<Response, String> {
    let (schema, path, document) = match plan {
        Plan::FrontmatterField { schema, path } => (schema, path, false),
        Plan::LabelledField { schema, path } => (schema, path, true),
    };
    if path.is_empty() || path.len() > 8 || path.iter().any(|key| key.is_empty() || key.len() > 256)
    {
        return Err("Invalid structured-field path".into());
    }
    let header = if document {
        inspect_document(source)?
    } else {
        frontmatter::inspect(source)?
    };
    let selected = header.filter(|header| header.schema == schema);
    let value = selected
        .as_ref()
        .and_then(|header| header.fields.iter().find(|field| field.path == path))
        .cloned();
    Ok(Response::Field {
        value,
        evidence: selected
            .as_ref()
            .map(|header| header.raw.clone())
            .unwrap_or_default(),
        location: selected
            .map(|header| {
                if document {
                    format!("labelled text field {}", path.join("."))
                } else {
                    format!("{} front matter field {}", header.format, path.join("."))
                }
            })
            .unwrap_or_default(),
    })
}

pub(crate) fn run() -> Result<(), String> {
    // Never parse a request if the OS boundary could not be installed.
    sandbox::install()?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut input)
        .map_err(|_| "Could not read extraction request")?;
    if input.len() > MAX_INPUT {
        return Err("Extraction request exceeds its byte budget".into());
    }
    let result = serde_json::from_slice(&input)
        .map_err(|_| "Invalid extraction request".to_string())
        .and_then(execute);
    let output = serde_json::to_vec(&result).map_err(|_| "Invalid extraction response")?;
    std::io::stdout()
        .write_all(&output)
        .map_err(|_| "Could not return extraction response".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_names_paths_and_unknown_operations_are_rejected() {
        for source in [
            r#"{"operation":"exec","command":"grep date /etc/passwd"}"#,
            r#"{"operation":"inspect","source":"","path":"/etc/passwd"}"#,
            r#"{"operation":"execute","source":"","plan":{"operation":"shell","command":"true"}}"#,
        ] {
            assert!(serde_json::from_str::<Request>(source).is_err());
        }
    }

    #[test]
    fn changed_layout_cannot_reuse_a_structured_plan() {
        let source = "---\ndate: 2021-11-18\n---\n";
        let header = frontmatter::inspect(source).unwrap().unwrap();
        let plan = Plan::FrontmatterField {
            schema: header.schema,
            path: vec!["date".into()],
        };
        let response = execute(Request::Execute {
            source: source.into(),
            plan: plan.clone(),
        })
        .unwrap();
        assert!(matches!(response, Response::Field { value: Some(_), .. }));
        let response = execute(Request::Execute {
            source: "---\nother: value\n---\n".into(),
            plan,
        })
        .unwrap();
        assert!(matches!(response, Response::Field { value: None, .. }));
    }

    #[test]
    fn labelled_capture_reparses_current_document_and_declines_changed_or_ambiguous_layout() {
        let source = "Date: 2021-11-18\nCount: 0\n";
        let header = lines::inspect(source).unwrap().unwrap();
        let plan = Plan::LabelledField {
            schema: header.schema,
            path: vec!["Date".into()],
        };
        let execute_plan = |source: &str| {
            execute(Request::Execute {
                source: source.into(),
                plan: plan.clone(),
            })
            .unwrap()
        };
        let Response::Field {
            value: Some(field),
            evidence,
            location,
        } = execute_plan("Date: 2025-01-04\nCount: 2\n")
        else {
            panic!()
        };
        assert_eq!(field.value, "2025-01-04");
        assert!(evidence.contains("Date: 2025-01-04"));
        assert!(location.contains("Date"));
        assert!(matches!(
            execute_plan("Date: 2025-01-04\nDate: 2021-11-18\n"),
            Response::Field { value: None, .. }
        ));
        assert!(matches!(
            execute_plan("Date: 2025-01-04\nA contradictory sentence.\nCount: 2\n"),
            Response::Field { value: None, .. }
        ));
        assert!(matches!(
            execute_plan("Count: 2\nDate: 2025-01-04\n"),
            Response::Field { value: None, .. }
        ));
    }
}
