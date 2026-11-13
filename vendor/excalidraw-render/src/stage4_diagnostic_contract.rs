//! Mixed source-order contract through the full facade and the existing private
//! metadata-operation seam. Expected rows are authored from spec §11.3.
use crate::render::test_faults::{self, Fault};
use crate::render::MetadataStage;
use crate::{
    MetadataOptions, OutputFormat, RenderRequest, Renderer, ResourceLimits, SceneDocument,
};
use base64::Engine as _;
use serde_json::json;

#[test]
fn mixed_normalization_render_resource_metadata_preserve_source_order() {
    let bytes = b"\x89PNG\r\n\x1a\n";
    let mut source = json!({"type":"excalidraw","version":2,"elements":[
        {"id":"asset-image","type":"image","width":3,"height":2,"fileId":"asset","status":"saved","scale":[1,1],"updated":1},
        {"id":"normal","type":"rectangle","x":10,"width":3,"height":2,"seed":0,"roughness":0,"updated":1},
        {"id":"render","type":"future","x":20,"width":3,"height":2,"updated":1}
    ],"appState":{},"files":{"asset":{"id":"asset","mimeType":"image/png","dataURL":format!("data:image/png;base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes)),"created":1}}});
    for reverse in [false, true] {
        if reverse {
            source["elements"].as_array_mut().unwrap().reverse();
        }
        let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
        for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
            let limits = ResourceLimits::builder()
                .max_asset_bytes(1)
                .build()
                .unwrap();
            let request = RenderRequest::builder(format)
                .padding(0.0)
                .resource_limits(limits.clone())
                .metadata_options(MetadataOptions::default().with_provenance(true))
                .build()
                .unwrap();
            let render = || {
                test_faults::run(&[Fault::at(MetadataStage::ProvenanceSerialization)], || {
                    Renderer::new().render(&scene, &request)
                })
            };
            let (first, observed) = render();
            let first = first.unwrap();
            let (second, _) = render();
            let second = second.unwrap();
            assert_eq!(observed.failures, [MetadataStage::ProvenanceSerialization]);
            assert_eq!(first.artifact().bytes(), second.artifact().bytes());
            assert_eq!(first.report(), second.report());
            let mut rows = vec![
                json!({"code":"normalization.zero-rough-seed","severity":"warning","effects":["input_repair","visual"],"element_id":"normal","json_pointer":"/elements/1/seed","details":{"kind":"compatibility","recovery":"repaired","recipe_id":null}}),
                json!({"code":"compatibility.unsupported-element-type","severity":"warning","effects":["visual","semantic"],"element_id":"render","json_pointer":if reverse {"/elements/0"} else {"/elements/2"},"details":{"kind":"compatibility","recovery":"placeholder","recipe_id":"renderer.unknown-element.v1"}}),
            ];
            if reverse {
                rows.reverse();
            }
            rows.push(json!({"code":"resource.asset-bytes-exceeded","severity":"warning","effects":["visual"],"element_id":"asset-image","json_pointer":"/files/asset/dataURL","details":{"kind":"resource_limit","field":"max_asset_bytes","configured":1,"value_kind":"bytes","measurement":{"kind":"attempted","value":8},"recovery":if format==OutputFormat::Png {"placeholder"} else {"omitted"}}}));
            rows.push(json!({"code":"metadata.provenance-serialization-failed","severity":"warning","effects":["metadata"],"element_id":null,"json_pointer":"","details":{"kind":"metadata","component":"provenance","operation":"serialization","recovery":"metadata_omitted"}}));
            for row in &mut rows {
                row["format"] = json!(format.as_str());
            }
            let mut actual = serde_json::to_value(first.report()).unwrap();
            for row in actual["diagnostics"].as_array_mut().unwrap() {
                assert!(!row["message"].as_str().unwrap().is_empty());
                row.as_object_mut().unwrap().remove("message");
            }
            assert_eq!(actual, json!({"schema_version":1,"diagnostics":rows}));
            let control = Renderer::new()
                .render(
                    &scene,
                    &RenderRequest::builder(format)
                        .padding(0.0)
                        .resource_limits(limits)
                        .build()
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(first.artifact().bytes(), control.artifact().bytes());
        }
    }
}
