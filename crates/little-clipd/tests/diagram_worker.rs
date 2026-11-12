//! Verify native rendering dispatches before any host UI/runtime setup.
use little_clip_core::CommandRequest;
use std::time::Duration;

#[test]
fn diagram_worker_renders_and_reports_errors_without_host_startup() {
    for executable in [
        env!("CARGO_BIN_EXE_little-clipd"),
        env!("CARGO_BIN_EXE_test_screen"),
    ] {
        for (format, source, success) in [
            ("pikchr", "box \"Source\"; arrow; circle \"Preview\"", true),
            ("mermaid", "flowchart LR\nA[Source] --> B[Preview]", true),
            (
                "lini",
                "|box#a| \"Source\"\n|box#b| \"Preview\"\na -> b",
                true,
            ),
            ("svgbob", "+----+\n|Hi! |\n+----+", true),
            ("pikchr", "box at invalid nonsense", false),
        ] {
            let request = serde_json::json!({ "format": format, "source": source });
            let output = CommandRequest::new(executable)
                .arg("--internal-diagram-worker")
                .resource_limit(rustix::process::Resource::As, 2 * 1024 * 1024 * 1024)
                .resource_limit(rustix::process::Resource::Cpu, 6)
                .stdin_bytes(serde_json::to_vec(&request).unwrap())
                .timeout(Duration::from_secs(10))
                .max_output_bytes(8 * 1024 * 1024)
                .own_process_group()
                .die_with_parent()
                .run()
                .unwrap();
            assert!(output.status.success(), "{}", output.stderr_lossy());
            assert!(!output.stdout_truncated);
            let response: Result<little_clip_render::diagram::Rendered, String> =
                serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(response.is_ok(), success, "{response:?}");
            if let Ok(rendered) = response {
                assert!(rendered.svg.contains("<svg"));
                assert!(rendered.png.starts_with(b"\x89PNG\r\n\x1a\n"));
                assert!(rendered.width > 0 && rendered.height > 0);
            }
        }
    }
}
