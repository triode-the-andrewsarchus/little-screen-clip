//! Real-host protocol fixtures, enabled only by explicit test support.
use super::{CPU_LIMIT, HEIGHT, MAX_OUTPUT, MAX_SVG, MEMORY_LIMIT, Rendered, WIDTH, WORKER_FLAG};
use little_clip_core::CommandRequest;
use std::{ffi::OsStr, time::Duration};

/// Checks native rendering and error delivery before a host starts its UI/runtime.
///
/// Pass the host's `CARGO_BIN_EXE_*` path, not the integration-test executable.
///
/// # Panics
/// Panics if dispatch starts ordinary host behavior, exceeds the production
/// resource/protocol bounds, fails a valid fixture or accepts malformed source.
pub fn assert_worker(executable: impl AsRef<OsStr>) {
    for (format, source, success) in [
        (
            "structurizr",
            include_str!("../../../../docs/examples/diagrams/architecture.structurizr"),
            true,
        ),
        ("structurizr", "workspace { model {", false),
        (
            "drawio",
            include_str!("../../../../docs/examples/diagrams/pipeline.drawio"),
            true,
        ),
        (
            "drawio",
            "<mxfile><diagram>broken!</diagram></mxfile>",
            false,
        ),
        ("pikchr", "box \"Source\"; arrow; circle \"Preview\"", true),
        ("mermaid", "flowchart LR\nA[Source] --> B[Preview]", true),
        (
            "lini",
            "|box#a| \"Source\"\n|box#b| \"Preview\"\na -> b",
            true,
        ),
        ("svgbob", "+----+\n|Hi! |\n+----+", true),
        ("graphviz", "digraph { a -> b }", true),
        ("nomnoml", "[Source] -> [Preview]", true),
        ("nomnoml", "[Unclosed", false),
        (
            "dbml",
            include_str!("../../../../docs/examples/diagrams/schema.dbml"),
            true,
        ),
        ("dbml", "Table broken {", false),
        (
            "goat",
            include_str!("../../../../docs/examples/diagrams/pipeline.goat"),
            true,
        ),
        (
            "goat",
            include_str!("../../../../docs/examples/diagrams/unicode.goat"),
            true,
        ),
        ("goat", "a\tb", false),
        ("bytefield", "(draw-column-headers) (draw-box 0)", true),
        ("bytefield", "(draw-box", false),
        (
            "erd",
            include_str!("../../../../docs/examples/diagrams/people.er"),
            true,
        ),
        ("erd", "[A]\nA 1--1 Missing", false),
        (
            "plantuml",
            include_str!("../../../../docs/examples/diagrams/sequence.puml"),
            true,
        ),
        (
            "plantuml",
            include_str!("../../../../docs/examples/diagrams/architecture.puml"),
            true,
        ),
        (
            "plantuml",
            "@startuml\n!include /etc/passwd\nA -> B\n@enduml",
            false,
        ),
        ("plantuml", "@startuml\nA -> B", false),
        ("bytefield", "(slurp \"/etc/passwd\")", false),
        ("nomnoml", "#import: /etc/passwd\n[A]", false),
        (
            "wireviz",
            include_str!("../../../../docs/examples/diagrams/harness.wireviz"),
            true,
        ),
        ("wireviz", "connectors: {}\nconnections: []", false),
        ("d2", "direction: right\na -> b: Preview", true),
        ("d2", "a: {", false),
        ("wavedrom", "{signal:[{name:'clk',wave:'p...'}]}", true),
        (
            "wavedrom",
            "{reg:[{bits:4,name:'op'},{bits:12,name:'payload'}]}",
            true,
        ),
        ("wavedrom", "{assign:[['out',['&','a','b']]]}", true),
        ("wavedrom", "{signal:(()=>{throw 'executed'})()}", false),
        (
            "excalidraw",
            include_str!("../../../../docs/examples/diagrams/drawing.excalidraw"),
            true,
        ),
        (
            "excalidraw",
            r#"{"type":"excalidraw","version":3,"elements":[]}"#,
            false,
        ),
        (
            "bpmn",
            include_str!("../../../../docs/examples/diagrams/order.bpmn"),
            true,
        ),
        ("bpmn", "<definitions/>", false),
        ("d2", "...@/etc/passwd", false),
        ("math", r"\sum_{n=1}^{\infty}\frac{1}{n^2}", true),
        ("mathinline", r"\frac{x}{y}", true),
        ("math", r"\input{/etc/passwd}", false),
        ("pikchr", "box at invalid nonsense", false),
        ("mermaid", "flowchart LR\nA[", false),
    ] {
        let request = serde_json::json!({ "format": format, "source": source });
        let output = CommandRequest::new(executable.as_ref())
            .arg(WORKER_FLAG)
            .resource_limit(rustix::process::Resource::As, MEMORY_LIMIT)
            .resource_limit(rustix::process::Resource::Cpu, CPU_LIMIT)
            .resource_limit(rustix::process::Resource::Fsize, MAX_OUTPUT as u64)
            .stdin_bytes(serde_json::to_vec(&request).unwrap())
            .timeout(Duration::from_secs(10))
            .max_output_bytes(MAX_OUTPUT)
            .own_process_group()
            .die_with_parent()
            .run()
            .unwrap();
        assert!(
            output.status.success(),
            "{format}: {}",
            output.stderr_lossy()
        );
        assert!(!output.stdout_truncated);
        let response: Result<Rendered, String> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            response.is_ok(),
            success,
            "{format}: {:?}",
            response.as_ref().err()
        );
        if let Ok(rendered) = response {
            assert!(rendered.svg.contains("<svg"));
            assert_eq!(rendered.math.is_some(), format.starts_with("math"));
            assert_eq!(rendered.drawing.is_some(), format == "excalidraw");
            if let Some(layout) = rendered.math {
                assert!(layout.valid());
            }
            assert!(rendered.svg.len() <= MAX_SVG);
            assert!(rendered.png.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert!(rendered.png.len() <= MAX_OUTPUT / 2);
            assert!(rendered.width > 0 && rendered.width <= WIDTH);
            assert!(rendered.height > 0 && rendered.height <= HEIGHT);
            assert!(rendered.warnings.len() <= 16);
            assert!(
                rendered
                    .warnings
                    .iter()
                    .all(|warning| warning.chars().count() <= 1024)
            );
        }
    }
}
