//! Declarative, data-only extraction. No executable names or paths are accepted.
pub(in crate::file_manager::search) mod frontmatter;
mod review;
mod sandbox;
mod session;
use little_clip_core::process::CommandRequest;
use serde::{Deserialize, Serialize};
pub(in crate::file_manager::search) use session::{SharedPlans, try_answer};
use std::{
    io::{Read, Write},
    sync::atomic::AtomicBool,
    time::Duration,
};

pub(super) const MAX_INPUT: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::file_manager::search) enum Plan {
    FrontmatterField { schema: String, path: Vec<String> },
}

pub(in crate::file_manager::search) enum Attempt {
    Answer(super::Answer),
    Escalate(String),
    Skip,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::file_manager::search) enum Request {
    Inspect { source: String },
    Execute { source: String, plan: Plan },
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
        Request::Execute {
            source,
            plan: Plan::FrontmatterField { schema, path },
        } => {
            if path.is_empty()
                || path.len() > 8
                || path.iter().any(|key| key.is_empty() || key.len() > 256)
            {
                return Err("Invalid structured-field path".into());
            }
            let header = frontmatter::inspect(&source)?;
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
                        format!("{} front matter field {}", header.format, path.join("."))
                    })
                    .unwrap_or_default(),
            })
        }
    }
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
}
