use super::preprocess::prepare_input;
use super::{DeskewMethod, RunControl};
use image::RgbaImage;
use little_clip_ops::document_unwarp::{PlanarMap, remap_document};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) const UVDOC: &[u8] = include_bytes!("../../../../assets/document-unwarp/uvdoc.onnx");
pub(super) const SYNTHETICDOC: &[u8] =
    include_bytes!("../../../../assets/document-unwarp/syntheticdoc.onnx");

pub(super) fn run(
    source: &RgbaImage,
    method: DeskewMethod,
    illumination: bool,
    cancelled: &AtomicBool,
    control: &RunControl,
) -> Result<RgbaImage, String> {
    let check = || cancelled.load(Ordering::Acquire);
    if check() {
        return Err("Document unwarp cancelled".into());
    }
    let mut limits = little_clip_core::item::raster_preview_limits();
    limits
        .reserve_buffer(source.width(), source.height(), image::ColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    let input = prepare_input(source, method)?;
    if check() {
        return Err("Document unwarp cancelled".into());
    }
    let prediction = predict(input, method, illumination, cancelled, control)?;
    let grid = output_map::<2>(&prediction.grid)?;
    let shadow = prediction
        .shadow
        .as_ref()
        .map(output_map::<3>)
        .transpose()?;
    remap_document(
        source,
        grid,
        shadow,
        if method == DeskewMethod::UvDoc {
            0
        } else {
            128
        },
        &check,
    )
}

struct Prediction {
    grid: ort::value::DynValue,
    shadow: Option<ort::value::DynValue>,
}

// Own only the required output tensors after the model run; the raster pass
// stays independent of session construction, tensor layout and GPU execution.
fn predict(
    input: Vec<f32>,
    method: DeskewMethod,
    illumination: bool,
    cancelled: &AtomicBool,
    control: &RunControl,
) -> Result<Prediction, String> {
    let check = || cancelled.load(Ordering::Acquire);
    let bytes = match method {
        DeskewMethod::UvDoc => UVDOC,
        DeskewMethod::SyntheticDoc => SYNTHETICDOC,
        DeskewMethod::Manual => return Err("Choose a document model first".into()),
    };
    let mut session = crate::onnx_runtime::load_bundled_session(method.label(), bytes)?;
    if check() {
        return Err("Document unwarp cancelled".into());
    }
    let options = Arc::new(ort::session::RunOptions::new().map_err(|e| e.to_string())?);
    *control.lock().map_err(|e| e.to_string())? = Some(options.clone());
    // Close the cancellation race between session loading and run publication.
    if check() {
        let _ = options.terminate();
    }
    let (width, height) = method.input_size();
    let tensor = ort::value::Tensor::from_array(([1_usize, 3, height, width], input))
        .map_err(|e| e.to_string())?;
    let mut outputs = session
        .run_with_options(ort::inputs!["image" => tensor], &options)
        .map_err(|e| format!("{} inference failed: {e}", method.label()))?;
    if check() {
        return Err("Document unwarp cancelled".into());
    }
    let grid = outputs.remove("grid").ok_or("Missing document grid")?;
    let shadow = if illumination && method == DeskewMethod::SyntheticDoc {
        Some(outputs.remove("shadow").ok_or("Missing illumination map")?)
    } else {
        None
    };
    Ok(Prediction { grid, shadow })
}

// Validate the runtime's tensor contract at the boundary, before sampling.
fn output_map<const C: usize>(value: &ort::value::DynValue) -> Result<PlanarMap<'_, C>, String> {
    let (shape, values) = value
        .try_extract_tensor::<f32>()
        .map_err(|e| e.to_string())?;
    if shape.len() != 4 || shape[0] != 1 || shape[1] != C as i64 {
        return Err("Unexpected document model output shape".into());
    }
    let width = usize::try_from(shape[3]).map_err(|e| e.to_string())?;
    let height = usize::try_from(shape[2]).map_err(|e| e.to_string())?;
    PlanarMap::new(values, width, height)
}
