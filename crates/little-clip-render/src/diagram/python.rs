//! Shared bounded local execution for pinned, source-only Python diagram engines.
use base64::Engine;
use little_clip_core::CommandRequest;
use std::{sync::LazyLock, time::Duration};

pub(super) struct Program<'a> {
    pub name: &'a str,
    pub modules: &'a [(&'a str, bool, &'a str)],
    pub adapter: &'a str,
}

impl Program<'_> {
    pub fn call(&self, input: serde_json::Value) -> Result<(String, Vec<String>), String> {
        static FONTS: LazyLock<Vec<String>> = LazyLock::new(|| {
            little_clip_core::font_assets::GRAPHVIZ_FONT_BYTES
                .iter()
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                .collect()
        });
        let input = serde_json::to_vec(&serde_json::json!({
            "name": self.name,
            "input": input,
            "modules": self.modules,
            "fonts": &*FONTS,
        }))
        .map_err(|error| error.to_string())?;
        // Both program fragments are fixed include_str! assets. Only the JSON
        // payload contains authored text, passed as data after trusted setup.
        let bootstrap = format!(
            "{}\n{}\nmain()\n",
            include_str!("python/bootstrap.py"),
            self.adapter
        );
        let output = CommandRequest::new("python3")
            .arg("-I")
            .arg("-c")
            .arg(bootstrap)
            .stdin_bytes(input)
            .resource_limit(rustix::process::Resource::As, 512 * 1024 * 1024)
            .resource_limit(rustix::process::Resource::Cpu, 4)
            .resource_limit(rustix::process::Resource::Fsize, 0)
            .timeout(Duration::from_secs(5))
            .max_output_bytes(super::MAX_SVG)
            .own_process_group()
            .die_with_parent()
            .run()
            .map_err(|error| {
                format!(
                    "Local {} preview requires Python 3 with Pillow: {error}",
                    self.name
                )
            })?;
        if !output.status.success() || output.stdout_truncated || output.stderr_truncated {
            let diagnostic = output.stderr_lossy();
            return Err(if diagnostic.trim().is_empty() {
                format!(
                    "{} parser/layout failed or exceeded the preview budget",
                    self.name
                )
            } else {
                diagnostic.trim().chars().take(2048).collect()
            });
        }
        let warnings = output
            .stderr_lossy()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(12)
            .map(|line| line.chars().take(1024).collect())
            .collect();
        let result = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
        Ok((result, warnings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_policy_denies_directories_network_processes_and_dynamic_libraries() {
        let program = Program {
            name: "Policy fixture",
            modules: &[],
            adapter: r#"
def prepare(fonts):
    import os, socket, subprocess, ctypes
    def render(input):
        operations = [lambda: os.listdir('/etc'), lambda: socket.socket(),
                      lambda: subprocess.run(['/bin/true']),
                      lambda: ctypes.CDLL('/etc/shadow')]
        failures = []
        for operation in operations:
            try:
                operation()
            except ValueError as error:
                failures.append(str(error))
        if len(failures) != len(operations):
            raise ValueError('resource policy missing')
        return '\n'.join(failures)
    return render
"#,
        };
        let (result, _) = program.call(serde_json::json!({})).unwrap();
        for event in [
            "os.listdir",
            "socket.__new__",
            "subprocess.Popen",
            "ctypes.dlopen",
        ] {
            assert!(result.contains(event), "{result}");
        }
    }

    #[test]
    fn authored_data_has_no_execution_or_external_resource_host() {
        let program = Program {
            name: "Policy fixture",
            modules: &[],
            adapter: r#"
def prepare(fonts):
    def render(input):
        try:
            open('/etc/shadow')
        except ValueError as error:
            return str(error) + ':' + input['source']
        raise ValueError('resource policy missing')
    return render
"#,
        };
        let authored = "raise SystemExit('executed')";
        let (result, warnings) = program
            .call(serde_json::json!({"source": authored}))
            .unwrap();
        assert!(result.contains("resource access is disabled: open"));
        assert!(result.ends_with(authored));
        assert!(warnings.is_empty());
    }
}
