//! One fixed resource/lifecycle boundary for separate local Java renderers.
use little_clip_core::CommandRequest;
use std::{path::PathBuf, time::Duration};

pub(super) fn svg(
    name: &str,
    source: &str,
    helper_name: &str,
    helper_path: &str,
) -> Result<String, String> {
    if source.len() > super::MAX_SOURCE {
        return Err(format!("{name} source exceeds the 256 KiB preview budget"));
    }
    let helper = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(helper_name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(helper_path));
    if !helper.is_file() {
        return Err(format!(
            "Bundled {name} companion is missing; distribute {helper_name} alongside the application"
        ));
    }
    // Source is only stdin data. It never supplies a JVM option, class name,
    // path or script. Bound JVM reservations within the existing worker limit.
    let output = CommandRequest::new("java")
        .args([
            "-Xms16m",
            "-Xmx128m",
            "-Xss512k",
            "-XX:+UseSerialGC",
            "-XX:ReservedCodeCacheSize=32m",
            "-XX:MaxMetaspaceSize=96m",
            "-XX:CompressedClassSpaceSize=32m",
            "-XX:ActiveProcessorCount=1",
            "-XX:-UsePerfData",
            "-Djava.awt.headless=true",
            "-jar",
        ])
        .arg(helper)
        .stdin_bytes(source.as_bytes().to_vec())
        .resource_limit(rustix::process::Resource::As, super::MEMORY_LIMIT)
        .resource_limit(rustix::process::Resource::Cpu, 4)
        .timeout(Duration::from_secs(5))
        .max_output_bytes(super::MAX_SVG)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|error| {
            format!(
                "{name} companion could not run (a local Java 11+ runtime is required): {error}"
            )
        })?;
    if output.stdout_truncated {
        return Err(format!("{name} generated SVG exceeds the preview budget"));
    }
    if !output.status.success() {
        let diagnostic = output.stderr_lossy().chars().take(2048).collect::<String>();
        if diagnostic.trim().is_empty() {
            return Err(format!("{name} companion failed: {}", output.status));
        }
        return Err(format!("{name}: {diagnostic}"));
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}
