//! Mermaid’s native parser/layout path with an explicit non-browser SVG contract.
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};
use std::time::Duration;

pub(super) fn svg(source: &str) -> Result<String, String> {
    // The SVG stage permits font references: our following raster stage resolves
    // them against owned bundled fonts. RequirePortable would reject that input
    // before font materialization (svg_fonts_not_self_contained).
    let request = SvgRequest {
        environment: merman::render::SvgEnvironment::deterministic()
            .with_resource_policy(merman::svg::RenderResourcePolicy::constrained()),
        pipeline: Some(merman::svg::SvgPipeline::resvg_safe()),
        ..Default::default()
    };
    // Mermaid uses local "today" for Gantt markers; a fixed test epoch would
    // silently display the wrong date. Keep only the layout/random seed fixed.
    let runtime = merman::model::runtime::RuntimePolicy::deterministic()
        .try_with_system_clock()
        .and_then(|policy| policy.try_with_system_time_zone())
        .map_err(|error| error.to_string())?;
    let output = Renderer::new()
        .with_runtime_policy(runtime)
        .render(RenderRequest::svg(
            source,
            OperationControl::new().with_deadline(Duration::from_secs(5)),
            request,
        ))
        .map_err(|error| error.to_string())?;
    let RenderOutput::Svg(Some(output)) = output else {
        return Err("Source does not contain a Mermaid diagram".into());
    };
    Ok(output.svg().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_mermaid_families_produce_native_svg_with_readable_labels() {
        for (source, label) in [
            ("flowchart LR\nA[Source] --> B[Preview]", "Source"),
            (
                "sequenceDiagram\nAlice->>Bob: Hello\nBob-->>Alice: Ready",
                "Alice",
            ),
            (
                "classDiagram\nVehicle <|-- Bicycle\nVehicle : +start()",
                "Vehicle",
            ),
            ("erDiagram\nUSER ||--o{ ORDER : places", "ORDER"),
        ] {
            let output = svg(source).unwrap();
            assert!(output.contains("<svg"));
            assert!(output.contains(label));
            assert!(
                !output.contains("<foreignObject"),
                "HTML labels must be converted to native SVG"
            );
        }
    }
}
