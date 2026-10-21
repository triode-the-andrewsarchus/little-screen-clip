//! Segment Anything (SAM / MobileSAM) Native Interactive Point-to-Select Engine
//!
//! Provides pure-Rust ONNX Runtime object segmentation under the mouse cursor,
//! allowing users to point-and-click to add or subtract segmented objects from the
//! shared inpainting mask without requiring Python.

use egui::{Color32, Pos2};
use image::{GrayImage, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use crate::onnx_runtime::{
    MOBILE_SAM_DECODER_MODEL_NAME, MOBILE_SAM_DECODER_MODEL_URL, MOBILE_SAM_ENCODER_MODEL_NAME,
    MOBILE_SAM_ENCODER_MODEL_URL, ensure_model_with_url, get_ort_environment,
};

/// Pure-Rust ONNX Runtime session for Segment Anything (MobileSAM).
pub struct NativeSamSession {
    encoder_session: ort::session::Session,
    decoder_session: ort::session::Session,
    cached_embeddings: Option<ndarray::ArrayD<f32>>,
    orig_w: u32,
    orig_h: u32,
    new_w: u32,
    new_h: u32,
}

impl NativeSamSession {
    pub fn new(encoder_path: &Path, decoder_path: &Path) -> Result<Self, String> {
        let env = get_ort_environment()?;
        let encoder_session = ort::session::Session::builder(&env)
            .map_err(|e| format!("ORT encoder builder failed: {e}"))?
            .commit_from_file(encoder_path)
            .map_err(|e| format!("ORT encoder load failed: {e}"))?;

        let decoder_session = ort::session::Session::builder(&env)
            .map_err(|e| format!("ORT decoder builder failed: {e}"))?
            .commit_from_file(decoder_path)
            .map_err(|e| format!("ORT decoder load failed: {e}"))?;

        Ok(Self {
            encoder_session,
            decoder_session,
            cached_embeddings: None,
            orig_w: 0,
            orig_h: 0,
            new_w: 0,
            new_h: 0,
        })
    }

    pub fn load_default() -> Result<Self, String> {
        let enc_path =
            ensure_model_with_url(MOBILE_SAM_ENCODER_MODEL_NAME, MOBILE_SAM_ENCODER_MODEL_URL)?;
        let dec_path =
            ensure_model_with_url(MOBILE_SAM_DECODER_MODEL_NAME, MOBILE_SAM_DECODER_MODEL_URL)?;
        Self::new(&enc_path, &dec_path)
    }

    pub fn encode_image(&mut self, image: &RgbaImage) -> Result<(), String> {
        let (orig_w, orig_h) = image.dimensions();
        if orig_w == 0 || orig_h == 0 {
            return Err("Image has zero dimension".to_string());
        }

        // SAM expects longest side resized to 1024
        let scale = 1024.0 / (orig_w.max(orig_h) as f32);
        let new_w = ((orig_w as f32 * scale).round() as u32).max(1);
        let new_h = ((orig_h as f32 * scale).round() as u32).max(1);

        let resized =
            image::imageops::resize(image, new_w, new_h, image::imageops::FilterType::Triangle);

        let mut img_arr = ndarray::Array3::<f32>::zeros((new_h as usize, new_w as usize, 3));
        for y in 0..new_h {
            for x in 0..new_w {
                let p = resized.get_pixel(x, y);
                img_arr[[y as usize, x as usize, 0]] = p[0] as f32;
                img_arr[[y as usize, x as usize, 1]] = p[1] as f32;
                img_arr[[y as usize, x as usize, 2]] = p[2] as f32;
            }
        }

        let tensor = ort::value::Tensor::from_array(img_arr)
            .map_err(|e| format!("ORT tensor conversion failed: {e}"))?;

        let outputs = self
            .encoder_session
            .run(ort::inputs![tensor])
            .map_err(|e| format!("ORT encoder inference failed: {e}"))?;

        let emb = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| format!("Failed to extract embedding array: {e}"))?
            .to_owned()
            .into_dyn();

        self.cached_embeddings = Some(emb);
        self.orig_w = orig_w;
        self.orig_h = orig_h;
        self.new_w = new_w;
        self.new_h = new_h;

        Ok(())
    }

    pub fn predict_point(&mut self, x: f32, y: f32) -> Result<GrayImage, String> {
        let emb = self
            .cached_embeddings
            .as_ref()
            .ok_or_else(|| "No image encoded in SAM session".to_string())?;

        let clamped_x = x.clamp(0.0, (self.orig_w.saturating_sub(1)) as f32);
        let clamped_y = y.clamp(0.0, (self.orig_h.saturating_sub(1)) as f32);

        let scaled_x = clamped_x * (self.new_w as f32 / self.orig_w as f32);
        let scaled_y = clamped_y * (self.new_h as f32 / self.orig_h as f32);

        let coords = ndarray::Array3::<f32>::from_shape_vec((1, 1, 2), vec![scaled_x, scaled_y])
            .map_err(|e| format!("Shape error: {e}"))?;
        let labels = ndarray::Array2::<f32>::from_shape_vec((1, 1), vec![1.0])
            .map_err(|e| format!("Shape error: {e}"))?;
        let mask_in = ndarray::Array4::<f32>::zeros((1, 1, 256, 256));
        let has_mask = ndarray::Array1::<f32>::zeros(1);
        let orig_sz =
            ndarray::Array1::<f32>::from_shape_vec(2, vec![self.orig_h as f32, self.orig_w as f32])
                .map_err(|e| format!("Shape error: {e}"))?;

        let emb_tensor = ort::value::Tensor::from_array(emb.clone())
            .map_err(|e| format!("Tensor conversion: {e}"))?;
        let coords_tensor = ort::value::Tensor::from_array(coords)
            .map_err(|e| format!("Tensor conversion: {e}"))?;
        let labels_tensor = ort::value::Tensor::from_array(labels)
            .map_err(|e| format!("Tensor conversion: {e}"))?;
        let mask_in_tensor = ort::value::Tensor::from_array(mask_in)
            .map_err(|e| format!("Tensor conversion: {e}"))?;
        let has_mask_tensor = ort::value::Tensor::from_array(has_mask)
            .map_err(|e| format!("Tensor conversion: {e}"))?;
        let orig_sz_tensor = ort::value::Tensor::from_array(orig_sz)
            .map_err(|e| format!("Tensor conversion: {e}"))?;

        let outputs = self
            .decoder_session
            .run(ort::inputs![
                emb_tensor,
                coords_tensor,
                labels_tensor,
                mask_in_tensor,
                has_mask_tensor,
                orig_sz_tensor,
            ])
            .map_err(|e| format!("ORT decoder run failed: {e}"))?;

        let mask_arr = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| format!("Failed to extract mask array: {e}"))?;

        let mut mask_img = GrayImage::new(self.orig_w, self.orig_h);
        for cy in 0..self.orig_h {
            for cx in 0..self.orig_w {
                let val = mask_arr[[0, 0, cy as usize, cx as usize]];
                if val > 0.0 {
                    mask_img.put_pixel(cx, cy, image::Luma([255]));
                }
            }
        }

        Ok(mask_img)
    }
}

/// Stamps a binary SAM mask onto a target inpaint mask.
///
/// If `val` is 255, pixels where `source > 0` are added (marked for inpainting).
/// If `val` is 0, pixels where `source > 0` are erased (cleared from inpainting).
pub fn stamp_sam_mask_on_inpaint_mask(target: &mut GrayImage, source: &GrayImage, val: u8) {
    let (tw, th) = target.dimensions();
    let (sw, sh) = source.dimensions();
    let w = tw.min(sw);
    let h = th.min(sh);

    let target_raw = target.as_mut();
    let source_raw = source.as_raw();

    if tw == sw && th == sh {
        // Fast path: identical dimensions
        for (tp, &sp) in target_raw.iter_mut().zip(source_raw.iter()) {
            if sp > 0 {
                *tp = val;
            }
        }
    } else {
        for y in 0..h {
            let t_row_start = (y * tw) as usize;
            let s_row_start = (y * sw) as usize;
            for x in 0..w as usize {
                if source_raw[s_row_start + x] > 0 {
                    target_raw[t_row_start + x] = val;
                }
            }
        }
    }
}

/// Creates or updates an egui texture for displaying the hovered SAM segmented object
/// in a high-visibility transparent blue overlay.
pub fn update_hovered_mask_texture(
    mask: &GrayImage,
    tex_handle: &mut Option<egui::TextureHandle>,
    ctx: &egui::Context,
) {
    let (w, h) = mask.dimensions();
    let mut pixels = Vec::with_capacity((w * h) as usize);

    // High-visibility transparent sky blue overlay
    let blue_fill = Color32::from_rgba_unmultiplied(30, 144, 255, 125);

    for p in mask.pixels() {
        if p[0] > 0 {
            pixels.push(blue_fill);
        } else {
            pixels.push(Color32::TRANSPARENT);
        }
    }

    let color_image = egui::ColorImage {
        size: [w as usize, h as usize],
        pixels,
    };

    if let Some(tex) = tex_handle {
        tex.set(color_image, egui::TextureOptions::NEAREST);
    } else {
        *tex_handle =
            Some(ctx.load_texture("sam_hover_mask", color_image, egui::TextureOptions::NEAREST));
    }
}

/// Dilates a binary mask outward by `radius_px` using the linear-time 2D Euclidean Distance Transform.
///
/// This provides exact, smooth, isotropic dilation without directional bias in O(W * H) time.
pub fn dilate_binary_mask(mask: &GrayImage, radius_px: f32) -> GrayImage {
    if radius_px <= 0.0 {
        return mask.clone();
    }

    let (w, h) = mask.dimensions();
    if w == 0 || h == 0 {
        return mask.clone();
    }

    let w_usize = w as usize;
    let h_usize = h as usize;
    let r2 = radius_px * radius_px;

    // Helper 1D parabolic envelope distance transform (Felzenszwalb-Huttenlocher)
    fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
        let n = f.len();
        if n == 0 {
            return;
        }
        let mut k = 0usize;
        v[0] = 0;
        z[0] = -1e20;
        z[1] = 1e20;

        for q in 1..n {
            let mut s = ((f[q] + (q * q) as f32) - (f[v[k]] + (v[k] * v[k]) as f32))
                / (2 * q - 2 * v[k]) as f32;
            while s <= z[k] {
                k = k.saturating_sub(1);
                s = ((f[q] + (q * q) as f32) - (f[v[k]] + (v[k] * v[k]) as f32))
                    / (2 * q - 2 * v[k]) as f32;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = 1e20;
        }

        k = 0;
        for q in 0..n {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let dist_sq = (q as isize - v[k] as isize).pow(2) as f32 + f[v[k]];
            d[q] = dist_sq;
        }
    }

    // Grid initialized with 0.0 if mask > 0, else large infinity
    let mut grid = vec![1e20f32; w_usize * h_usize];
    for y in 0..h_usize {
        let row_offset = y * w_usize;
        for x in 0..w_usize {
            if mask.get_pixel(x as u32, y as u32)[0] > 0 {
                grid[row_offset + x] = 0.0;
            }
        }
    }

    let mut temp = vec![0.0f32; w_usize * h_usize];
    let max_dim = w_usize.max(h_usize);
    let mut v = vec![0usize; max_dim];
    let mut z = vec![0.0f32; max_dim + 1];
    let mut col_in = vec![0.0f32; h_usize];
    let mut col_out = vec![0.0f32; h_usize];

    // Pass 1: Columns
    for x in 0..w_usize {
        for y in 0..h_usize {
            col_in[y] = grid[y * w_usize + x];
        }
        edt_1d(&col_in, &mut col_out, &mut v, &mut z);
        for y in 0..h_usize {
            temp[y * w_usize + x] = col_out[y];
        }
    }

    // Pass 2: Rows & thresholding
    let mut dilated = GrayImage::new(w, h);
    let mut row_out = vec![0.0f32; w_usize];

    for y in 0..h_usize {
        let row_start = y * w_usize;
        let row_in = &temp[row_start..row_start + w_usize];
        edt_1d(row_in, &mut row_out, &mut v, &mut z);
        for x in 0..w_usize {
            if row_out[x] <= r2 {
                dilated.put_pixel(x as u32, y as u32, image::Luma([255]));
            }
        }
    }

    dilated
}

pub enum SamWorkerCommand {
    EncodeImage {
        image: Arc<RgbaImage>,
        revision: u64,
    },
    PredictPoint {
        x: f32,
        y: f32,
        query_id: u64,
    },
    Quit,
}

#[allow(dead_code)]
pub enum SamWorkerResponse {
    Ready,
    ImageLoaded {
        revision: u64,
        width: u32,
        height: u32,
    },
    Mask {
        query_id: u64,
        x: f32,
        y: f32,
        width: u32,
        height: u32,
        mask: GrayImage,
    },
    Error(String),
}

pub struct SamWorkerHandle {
    tx: Sender<SamWorkerCommand>,
    rx: Receiver<SamWorkerResponse>,
    stopped: Arc<AtomicBool>,
}

impl SamWorkerHandle {
    pub fn spawn_native() -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = channel::<SamWorkerCommand>();
        let (resp_tx, resp_rx) = channel::<SamWorkerResponse>();

        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_clone = stopped.clone();

        std::thread::Builder::new()
            .name("sam-native-worker".into())
            .spawn(move || {
                run_sam_native_worker_thread(cmd_rx, resp_tx, stopped_clone);
            })
            .map_err(|e| format!("Failed to spawn sam worker thread: {e}"))?;

        Ok(Self {
            tx: cmd_tx,
            rx: resp_rx,
            stopped,
        })
    }

    pub fn send(&self, cmd: SamWorkerCommand) -> Result<(), String> {
        self.tx.send(cmd).map_err(|e| e.to_string())
    }

    pub fn try_recv(&self) -> Result<SamWorkerResponse, std::sync::mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

impl Drop for SamWorkerHandle {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.tx.send(SamWorkerCommand::Quit);
    }
}

fn run_sam_native_worker_thread(
    cmd_rx: Receiver<SamWorkerCommand>,
    resp_tx: Sender<SamWorkerResponse>,
    stopped: Arc<AtomicBool>,
) {
    let mut session = match NativeSamSession::load_default() {
        Ok(sess) => {
            let _ = resp_tx.send(SamWorkerResponse::Ready);
            sess
        }
        Err(err) => {
            let _ = resp_tx.send(SamWorkerResponse::Error(format!(
                "Failed to initialize Native SAM ONNX session: {err}"
            )));
            return;
        }
    };

    while let Ok(cmd) = cmd_rx.recv() {
        if stopped.load(Ordering::SeqCst) {
            break;
        }

        match cmd {
            SamWorkerCommand::Quit => break,
            SamWorkerCommand::EncodeImage { image, revision } => {
                match session.encode_image(&image) {
                    Ok(()) => {
                        let _ = resp_tx.send(SamWorkerResponse::ImageLoaded {
                            revision,
                            width: session.orig_w,
                            height: session.orig_h,
                        });
                    }
                    Err(err) => {
                        let _ = resp_tx.send(SamWorkerResponse::Error(format!(
                            "SAM encoding failed: {err}"
                        )));
                    }
                }
            }
            SamWorkerCommand::PredictPoint { x, y, query_id } => {
                match session.predict_point(x, y) {
                    Ok(mask) => {
                        let _ = resp_tx.send(SamWorkerResponse::Mask {
                            query_id,
                            x,
                            y,
                            width: session.orig_w,
                            height: session.orig_h,
                            mask,
                        });
                    }
                    Err(err) => {
                        let _ = resp_tx.send(SamWorkerResponse::Error(format!(
                            "SAM prediction failed: {err}"
                        )));
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Default)]
pub struct SamState {
    pub worker: Option<SamWorkerHandle>,
    pub loaded_revision: Option<u64>,
    pub is_loading: bool,
    pub is_ready: bool,
    pub status_text: Option<String>,
    pub error_text: Option<String>,

    pub hovered_pos: Option<Pos2>,
    pub hovered_point_px: Option<(f32, f32)>,
    pub raw_hovered_mask: Option<GrayImage>,
    pub hovered_mask: Option<GrayImage>,
    pub hovered_mask_texture: Option<egui::TextureHandle>,

    pub latest_query_id: u64,
    pub in_flight: bool,
    pub last_query_norm_pos: Option<Pos2>,
    pub last_query_instant: Option<Instant>,
    pub pending_click: Option<(Pos2, u8)>,
}

impl SamState {
    pub fn reset_hover(&mut self) {
        self.hovered_pos = None;
        self.hovered_point_px = None;
        self.raw_hovered_mask = None;
        self.hovered_mask = None;
        self.hovered_mask_texture = None;
        self.last_query_norm_pos = None;
        self.pending_click = None;
    }

    pub fn shutdown(&mut self) {
        self.worker = None;
        self.loaded_revision = None;
        self.is_loading = false;
        self.is_ready = false;
        self.status_text = None;
        self.error_text = None;
        self.reset_hover();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dilate_binary_mask_grows_selection_isotropically() {
        let mut mask = GrayImage::new(50, 50);
        // Single central pixel at (25, 25)
        mask.put_pixel(25, 25, image::Luma([255]));

        // Dilate by 5 pixels
        let dilated = dilate_binary_mask(&mask, 5.0);

        // Center must be 255
        assert_eq!(dilated.get_pixel(25, 25)[0], 255);
        // Pixels within 5.0 Euclidean distance must be 255
        assert_eq!(dilated.get_pixel(25, 20)[0], 255);
        assert_eq!(dilated.get_pixel(25, 30)[0], 255);
        assert_eq!(dilated.get_pixel(20, 25)[0], 255);
        assert_eq!(dilated.get_pixel(30, 25)[0], 255);
        assert_eq!(dilated.get_pixel(22, 22)[0], 255); // sqrt((3^2)+(3^2)) = 4.24 <= 5.0

        // Pixels outside 5.0 Euclidean distance must be 0
        assert_eq!(dilated.get_pixel(25, 19)[0], 0); // dist 6.0
        assert_eq!(dilated.get_pixel(25, 31)[0], 0); // dist 6.0
        assert_eq!(dilated.get_pixel(19, 25)[0], 0); // dist 6.0
        assert_eq!(dilated.get_pixel(21, 21)[0], 0); // sqrt((4^2)+(4^2)) = 5.66 > 5.0
        assert_eq!(dilated.get_pixel(5, 5)[0], 0);
    }

    #[test]
    fn stamp_sam_mask_combines_and_erases() {
        let width = 20;
        let height = 20;
        let mut target = GrayImage::new(width, height);

        let mut sam_mask = GrayImage::new(width, height);
        // Fill a 5x5 square in sam_mask
        for y in 5..10 {
            for x in 5..10 {
                sam_mask.put_pixel(x, y, image::Luma([255]));
            }
        }

        // 1. Stamp SAM mask onto target with 255 (addition)
        stamp_sam_mask_on_inpaint_mask(&mut target, &sam_mask, 255);

        for y in 5..10 {
            for x in 5..10 {
                assert_eq!(target.get_pixel(x, y)[0], 255);
            }
        }
        assert_eq!(target.get_pixel(0, 0)[0], 0);

        // 2. Erase SAM mask with 0 (subtraction)
        stamp_sam_mask_on_inpaint_mask(&mut target, &sam_mask, 0);
        for y in 5..10 {
            for x in 5..10 {
                assert_eq!(target.get_pixel(x, y)[0], 0);
            }
        }
    }

    #[test]
    fn test_native_sam_inference_if_models_present() {
        let enc_name = MOBILE_SAM_ENCODER_MODEL_NAME;
        let dec_name = MOBILE_SAM_DECODER_MODEL_NAME;
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let models_dir = PathBuf::from(home).join(".cache/little-screen-clip/models");
        let enc_path = models_dir.join(enc_name);
        let dec_path = models_dir.join(dec_name);

        if !enc_path.exists() || !dec_path.exists() {
            println!("Skipping native SAM test: models not cached");
            return;
        }

        let mut session = NativeSamSession::new(&enc_path, &dec_path)
            .expect("Failed to initialize native SAM session");

        // Create a synthetic image with a circle
        let mut img = RgbaImage::new(200, 200);
        for y in 0..200 {
            for x in 0..200 {
                let dx = x as f32 - 100.0;
                let dy = y as f32 - 100.0;
                if dx * dx + dy * dy <= 40.0 * 40.0 {
                    img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
                } else {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }

        session.encode_image(&img).expect("Failed to encode image");
        let mask = session
            .predict_point(100.0, 100.0)
            .expect("Failed to predict point");

        assert_eq!(mask.dimensions(), (200, 200));
        assert_eq!(mask.get_pixel(100, 100)[0], 255);
        assert_eq!(mask.get_pixel(10, 10)[0], 0);
    }
}
