use super::super::*;
use crate::shell::render_context_slider;

/// Descriptor for one entry in the film stock picker list.
#[derive(Clone)]
pub struct FilmStockEntry {
    pub brand: &'static str,
    pub name: &'static str,
    pub iso: u32,
    /// "Color Neg", "Color Slide", "B&W"
    pub film_type_label: &'static str,
    pub build: fn() -> filmr::film::FilmStock,
}

/// All available film stock presets, grouped by brand in order.
pub fn all_film_stocks() -> Vec<FilmStockEntry> {
    vec![
        // ── Kodak ──
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 160",
            iso: 160,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_160,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_400,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 400 (Art)",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_400_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Portra 800",
            iso: 800,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_PORTRA_800,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Gold 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_GOLD_200,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Ektar 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::kodak::KODAK_EKTAR_100,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Ektachrome 100",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_EKTACHROME_100,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Ektachrome 100VS",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_EKTACHROME_100VS,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Kodachrome 25",
            iso: 25,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_KODACHROME_25,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Kodachrome 64",
            iso: 64,
            film_type_label: "Color Slide",
            build: filmr::presets::kodak::KODAK_KODACHROME_64,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Tri-X 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::kodak::KODAK_TRI_X_400,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Tri-X 400 (Art)",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::kodak::KODAK_TRI_X_400_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Kodak",
            name: "Plus-X 125",
            iso: 125,
            film_type_label: "B&W",
            build: filmr::presets::kodak::KODAK_PLUS_X_125,
        },
        // ── Fujifilm ──
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Velvia 50",
            iso: 50,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::VELVIA_50,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Velvia 50 (Art)",
            iso: 50,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::VELVIA_50_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Provia 100F",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::PROVIA_100F,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Astia 100F",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::fujifilm::ASTIA_100F,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Superia 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::fujifilm::SUPERIA_100,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Superia 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::fujifilm::SUPERIA_200,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Superia 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::fujifilm::SUPERIA_400,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Neopan 100",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::fujifilm::NEOPAN_100,
        },
        FilmStockEntry {
            brand: "Fujifilm",
            name: "Neopan 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::fujifilm::NEOPAN_400,
        },
        // ── Ilford ──
        FilmStockEntry {
            brand: "Ilford",
            name: "HP5 Plus 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::HP5_PLUS_400,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "HP5 Plus 400 (Art)",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::HP5_PLUS_400_ARTISTIC,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "FP4 Plus 125",
            iso: 125,
            film_type_label: "B&W",
            build: filmr::presets::ilford::FP4_PLUS_125,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Pan F Plus 50",
            iso: 50,
            film_type_label: "B&W",
            build: filmr::presets::ilford::PAN_F_PLUS_50,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Delta 100 Pro",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::ilford::DELTA_100_PROFESSIONAL,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Delta 400 Pro",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::DELTA_400_PROFESSIONAL,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "XP2 Super 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::ilford::XP2_SUPER_400,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "SFX 200",
            iso: 200,
            film_type_label: "B&W",
            build: filmr::presets::ilford::SFX_200,
        },
        FilmStockEntry {
            brand: "Ilford",
            name: "Ortho Plus 80",
            iso: 80,
            film_type_label: "B&W",
            build: filmr::presets::ilford::ORTHO_PLUS_80,
        },
        // ── Agfa ──
        FilmStockEntry {
            brand: "Agfa",
            name: "Vista 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::VISTA_100,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Vista 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::VISTA_200,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Vista 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::VISTA_400,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "APX 100",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::agfa::APX_100,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "APX 400",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::agfa::APX_400,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Precisa 100",
            iso: 100,
            film_type_label: "Color Slide",
            build: filmr::presets::agfa::PRECISA_100,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Scala 200",
            iso: 200,
            film_type_label: "B&W",
            build: filmr::presets::agfa::SCALA_200,
        },
        FilmStockEntry {
            brand: "Agfa",
            name: "Optima 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::agfa::OPTIMA_200,
        },
        // ── Polaroid ──
        FilmStockEntry {
            brand: "Polaroid",
            name: "600 Color",
            iso: 640,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_600_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "SX-70 Color",
            iso: 160,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_SX70_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "i-Type Color",
            iso: 640,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_I_TYPE_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "B&W 667",
            iso: 3000,
            film_type_label: "B&W",
            build: filmr::presets::polaroid::POLAROID_BW_667,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "Spectra Color",
            iso: 640,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_SPECTRA_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "100 Color",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::polaroid::POLAROID_100_COLOR,
        },
        FilmStockEntry {
            brand: "Polaroid",
            name: "55 B&W",
            iso: 50,
            film_type_label: "B&W",
            build: filmr::presets::polaroid::POLAROID_55_BW,
        },
        // ── CineStill / Lomography / Other ──
        FilmStockEntry {
            brand: "CineStill",
            name: "800T",
            iso: 800,
            film_type_label: "Color Neg",
            build: filmr::presets::other::CINESTILL_800T,
        },
        FilmStockEntry {
            brand: "CineStill",
            name: "50D",
            iso: 50,
            film_type_label: "Color Neg",
            build: filmr::presets::other::CINESTILL_50D,
        },
        FilmStockEntry {
            brand: "Lomography",
            name: "Color Chrome",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::LOMOGRAPHY_COLOR_CHROME,
        },
        FilmStockEntry {
            brand: "Lomography",
            name: "LomoChrome Purple",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::LOMOGRAPHY_LOMOCHROME_PURPLE,
        },
        FilmStockEntry {
            brand: "Ferrania",
            name: "Solaris 100",
            iso: 100,
            film_type_label: "Color Neg",
            build: filmr::presets::other::FERRANIA_SOLARIS_100,
        },
        FilmStockEntry {
            brand: "Ferrania",
            name: "Solaris 400",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::FERRANIA_SOLARIS_400,
        },
        FilmStockEntry {
            brand: "ORWO",
            name: "UN54",
            iso: 100,
            film_type_label: "B&W",
            build: filmr::presets::other::ORWO_UN54,
        },
        FilmStockEntry {
            brand: "ORWO",
            name: "UN64",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::other::ORWO_UN64,
        },
        FilmStockEntry {
            brand: "Ricoh",
            name: "GR Street",
            iso: 400,
            film_type_label: "B&W",
            build: filmr::presets::other::RICOH_GR_STREET,
        },
        FilmStockEntry {
            brand: "Lucky",
            name: "Color 200",
            iso: 200,
            film_type_label: "Color Neg",
            build: filmr::presets::other::LUCKY_COLOR_200,
        },
        FilmStockEntry {
            brand: "Generic",
            name: "Standard Daylight",
            iso: 400,
            film_type_label: "Color Neg",
            build: filmr::presets::other::STANDARD_DAYLIGHT,
        },
    ]
}

/// Helper: renders a slider with full visual context (start, center, end, and default tick marks,
/// gauge fill line from default to current, and double-click to reset to default).

/// Applies post-processing photographic S-curve contrast and green/magenta tint.
pub fn apply_film_sim_post_adjustments(
    out_rgb: &image::RgbImage,
    contrast: f32,
    tint: f32,
) -> image::RgbaImage {
    let has_contrast = contrast.abs() > 0.001;
    let has_tint = tint.abs() > 0.001;

    if !has_contrast && !has_tint {
        return image::RgbaImage::from_fn(out_rgb.width(), out_rgb.height(), |x, y| {
            let p = out_rgb.get_pixel(x, y);
            image::Rgba([p[0], p[1], p[2], 255])
        });
    }

    // Precompute 256-entry S-curve LUT for contrast
    let mut lut = [0u8; 256];
    if has_contrast {
        let k = if contrast >= 0.0 {
            1.0 + contrast * 2.2
        } else {
            1.0 / (1.0 + contrast.abs() * 2.2)
        };
        for (i, slot) in lut.iter_mut().enumerate() {
            let t = i as f32 / 255.0;
            let curved = if t < 0.5 {
                0.5 * (2.0 * t).powf(k)
            } else {
                1.0 - 0.5 * (2.0 * (1.0 - t)).powf(k)
            };
            *slot = (curved.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    } else {
        for (i, slot) in lut.iter_mut().enumerate() {
            *slot = i as u8;
        }
    }

    // Tint multipliers (Green vs Magenta)
    let (r_mult, g_mult, b_mult) = if has_tint {
        if tint > 0.0 {
            // Magenta: boost Red and Blue, slightly reduce Green
            (1.0 + 0.18 * tint, 1.0 - 0.18 * tint, 1.0 + 0.18 * tint)
        } else {
            // Green: boost Green, slightly reduce Red and Blue
            let t = tint.abs();
            (1.0 - 0.18 * t, 1.0 + 0.18 * t, 1.0 - 0.18 * t)
        }
    } else {
        (1.0, 1.0, 1.0)
    };

    image::RgbaImage::from_fn(out_rgb.width(), out_rgb.height(), |x, y| {
        let p = out_rgb.get_pixel(x, y);
        let r_c = lut[p[0] as usize] as f32;
        let g_c = lut[p[1] as usize] as f32;
        let b_c = lut[p[2] as usize] as f32;

        let r_final = (r_c * r_mult).clamp(0.0, 255.0).round() as u8;
        let g_final = (g_c * g_mult).clamp(0.0, 255.0).round() as u8;
        let b_final = (b_c * b_mult).clamp(0.0, 255.0).round() as u8;
        image::Rgba([r_final, g_final, b_final, 255])
    })
}

/// Messages sent from the film sim background render thread.
#[derive(Debug)]
pub enum FilmSimMsg {
    Done { rgba: RgbaImage, duration_ms: u128 },
    Error(String),
}

/// Message streamed from the asynchronous film simulation thumbnail worker.
pub struct FilmSimThumbnailMsg {
    pub epoch: u64,
    pub stock_idx: usize,
    pub rgba: RgbaImage,
}

/// Creative light leak presets mapped to physics-based `filmr::light_leak::LightLeakConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightLeakPreset {
    #[default]
    None,
    WarmCorner,
    SunStreak,
    SoftEdge,
    OrganicBlob,
}

impl LightLeakPreset {
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::WarmCorner => "Warm Corner",
            Self::SunStreak => "Sun Streak",
            Self::SoftEdge => "Soft Edge",
            Self::OrganicBlob => "Organic",
        }
    }

    pub fn to_config(&self) -> LightLeakConfig {
        match self {
            Self::None => LightLeakConfig {
                enabled: false,
                leaks: Vec::new(),
            },
            Self::WarmCorner => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.9, 0.1),
                    color: [1.0, 0.55, 0.2],
                    radius: 0.45,
                    intensity: 1.1,
                    shape: LightLeakShape::Circle,
                    rotation: 0.0,
                    roughness: 0.4,
                }],
            },
            Self::SunStreak => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.15, 0.15),
                    color: [1.0, 0.8, 0.35],
                    radius: 0.6,
                    intensity: 1.2,
                    shape: LightLeakShape::Linear,
                    rotation: 0.75,
                    roughness: 0.3,
                }],
            },
            Self::SoftEdge => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.05, 0.5),
                    color: [0.95, 0.4, 0.2],
                    radius: 0.5,
                    intensity: 0.9,
                    shape: LightLeakShape::Circle,
                    rotation: 0.0,
                    roughness: 0.6,
                }],
            },
            Self::OrganicBlob => LightLeakConfig {
                enabled: true,
                leaks: vec![LightLeak {
                    position: (0.8, 0.8),
                    color: [1.0, 0.45, 0.3],
                    radius: 0.55,
                    intensity: 1.15,
                    shape: LightLeakShape::Organic,
                    rotation: 0.3,
                    roughness: 0.7,
                }],
            },
        }
    }
}

/// All state for the Film Simulation tab.
pub struct FilmSimState {
    // ── Exposure & Color Parameters ──
    pub selected_stock_idx: usize,
    /// Exposure multiplier: 0.1 .. 4.0, default 1.0
    pub brightness: f32,
    /// Post-process S-curve contrast: -1.0 .. +1.0, default 0.0
    pub contrast: f32,
    /// Warmth: -1.0 .. +1.0, default 0.0
    pub warmth: f32,
    /// Green vs Magenta tint: -1.0 .. +1.0, default 0.0 (negative = green, positive = magenta)
    pub tint: f32,
    /// Saturation intensity: 0.0 .. 2.0, default 1.0
    pub intensity: f32,
    /// White balance correction mode
    pub wb_mode: WhiteBalanceMode,
    /// White balance correction strength: 0.0 .. 1.0, default 1.0
    pub wb_strength: f32,

    // ── Optical & Creative Effects ──
    /// Halation glow strength: 0.0 .. 1.0, default 0.25
    pub halation: f32,
    /// Vignette falloff strength: 0.0 .. 1.0, default 0.35
    pub vignette: f32,
    /// Creative light leak preset
    pub light_leak: LightLeakPreset,
    pub enable_grain: bool,
    pub style: FilmStyle,

    // ── Preview state ──
    pub preview_rgba: Option<RgbaImage>,
    pub preview_texture: Option<egui::TextureHandle>,
    pub dirty_preview_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub last_render_duration_ms: u128,

    // ── Async render worker (Main image) ──
    pub is_rendering: bool,
    pub render_rx: Option<std::sync::mpsc::Receiver<FilmSimMsg>>,
    pub render_error: Option<String>,

    // ── Async thumbnail worker (Sidebar 42x28 mini-photos) ──
    pub stock_thumbnails: std::collections::HashMap<usize, egui::TextureHandle>,
    pub pending_thumbnails: Vec<(usize, image::RgbaImage)>,
    pub thumbnail_rx: Option<std::sync::mpsc::Receiver<FilmSimThumbnailMsg>>,
    pub thumbnail_epoch: u64,
    pub thumbnail_dirty_at: Option<std::time::Instant>,
    pub is_generating_thumbnails: bool,

    // ── Debounce timer ──
    /// Set to `Some(Instant)` whenever a parameter changes; cleared when render fires.
    pub params_dirty_at: Option<std::time::Instant>,

    // ── UI helpers ──
    pub search_query: String,
    pub auto_fitted: bool,
}

impl Default for FilmSimState {
    fn default() -> Self {
        Self {
            selected_stock_idx: 1, // Default: Kodak Portra 400
            brightness: 1.0,
            contrast: 0.0,
            warmth: 0.0,
            tint: 0.0,
            intensity: 1.0,
            wb_mode: WhiteBalanceMode::Auto,
            wb_strength: 1.0,
            halation: 0.25,
            vignette: 0.35,
            light_leak: LightLeakPreset::None,
            enable_grain: true,
            style: FilmStyle::Accurate,
            preview_rgba: None,
            preview_texture: None,
            dirty_preview_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            last_render_duration_ms: 0,
            is_rendering: false,
            render_rx: None,
            render_error: None,
            stock_thumbnails: std::collections::HashMap::new(),
            pending_thumbnails: Vec::new(),
            thumbnail_rx: None,
            thumbnail_epoch: 0,
            thumbnail_dirty_at: None,
            is_generating_thumbnails: false,
            params_dirty_at: None,
            search_query: String::new(),
            auto_fitted: false,
        }
    }
}

impl FilmSimState {
    /// Mark parameters as dirty (triggers debounced re-render for both main preview and thumbnails).
    pub fn mark_dirty(&mut self) {
        let now = std::time::Instant::now();
        self.params_dirty_at = Some(now);
        self.thumbnail_dirty_at = Some(now + std::time::Duration::from_millis(250));
    }
}

impl ImagePreviewState {
    pub fn run_film_sim_render(&mut self) {
        if self.film_sim_state.is_rendering || self.current_rgba.is_empty() {
            return;
        }
        let stocks = all_film_stocks();
        let idx = self
            .film_sim_state
            .selected_stock_idx
            .min(stocks.len().saturating_sub(1));
        let entry = &stocks[idx];
        let mut stock = (entry.build)().with_style(self.film_sim_state.style);
        stock.halation_strength = self.film_sim_state.halation;
        stock.vignette_strength = self.film_sim_state.vignette;

        let config = SimulationConfig {
            exposure_time: self.film_sim_state.brightness,
            warmth: self.film_sim_state.warmth,
            saturation: self.film_sim_state.intensity,
            auto_levels: false,
            white_balance_mode: self.film_sim_state.wb_mode,
            white_balance_strength: self.film_sim_state.wb_strength,
            output_mode: OutputMode::Positive,
            enable_grain: self.film_sim_state.enable_grain,
            light_leak: self.film_sim_state.light_leak.to_config(),
            ..SimulationConfig::default()
        };

        // Convert RgbaImage → RgbImage for filmr
        let rgba = self.current_rgba.clone();
        let w = rgba.width();
        let h = rgba.height();
        let rgb_pixels: Vec<u8> = rgba.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let rgb_img =
            image::RgbImage::from_raw(w, h, rgb_pixels).expect("pixel buffer size mismatch");

        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;

        let (tx, rx) = std::sync::mpsc::channel::<FilmSimMsg>();
        self.film_sim_state.render_rx = Some(rx);
        self.film_sim_state.is_rendering = true;
        self.film_sim_state.render_error = None;

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                filmr::processor::process_image(&rgb_img, &stock, &config)
            }));
            match process_result {
                Ok(out_rgb) => {
                    let out_rgba = apply_film_sim_post_adjustments(&out_rgb, contrast, tint);
                    let duration_ms = start.elapsed().as_millis();
                    let _ = tx.send(FilmSimMsg::Done {
                        rgba: out_rgba,
                        duration_ms,
                    });
                }
                Err(e) => {
                    let err_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Film simulation panicked".to_string()
                    };
                    let _ = tx.send(FilmSimMsg::Error(err_msg));
                }
            }
        });
    }

    /// Spawn low-priority background thread to generate 42x28 thumbnail previews for all film stocks.

    pub fn run_thumbnail_batch(&mut self) {
        if self.current_rgba.is_empty() {
            return;
        }
        self.film_sim_state.thumbnail_epoch += 1;
        let epoch = self.film_sim_state.thumbnail_epoch;

        // Downscale base image to 42x28 (sub-3ms per stock)
        let thumb_base = image::imageops::resize(
            &self.current_rgba,
            42,
            28,
            image::imageops::FilterType::Nearest,
        );
        let rgb_pixels: Vec<u8> = thumb_base
            .pixels()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let thumb_rgb = match image::RgbImage::from_raw(42, 28, rgb_pixels) {
            Some(img) => img,
            None => return,
        };

        let brightness = self.film_sim_state.brightness;
        let warmth = self.film_sim_state.warmth;
        let intensity = self.film_sim_state.intensity;
        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;
        let wb_mode = self.film_sim_state.wb_mode;
        let wb_strength = self.film_sim_state.wb_strength;
        let halation = self.film_sim_state.halation;
        let vignette = self.film_sim_state.vignette;
        let light_leak = self.film_sim_state.light_leak;
        let style = self.film_sim_state.style;
        let enable_grain = self.film_sim_state.enable_grain;

        let (tx, rx) = std::sync::mpsc::channel::<FilmSimThumbnailMsg>();
        self.film_sim_state.thumbnail_rx = Some(rx);
        self.film_sim_state.is_generating_thumbnails = true;

        std::thread::Builder::new()
            .name("film_sim_thumbnails".to_string())
            .spawn(move || {
                let stocks = all_film_stocks();
                for (idx, entry) in stocks.iter().enumerate() {
                    let mut stock = (entry.build)().with_style(style);
                    stock.halation_strength = halation;
                    stock.vignette_strength = vignette;

                    let config = SimulationConfig {
                        exposure_time: brightness,
                        warmth,
                        saturation: intensity,
                        auto_levels: false,
                        white_balance_mode: wb_mode,
                        white_balance_strength: wb_strength,
                        output_mode: OutputMode::Positive,
                        enable_grain,
                        light_leak: light_leak.to_config(),
                        ..SimulationConfig::default()
                    };

                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        filmr::processor::process_image(&thumb_rgb, &stock, &config)
                    }));

                    if let Ok(out_rgb) = res {
                        let out_rgba = apply_film_sim_post_adjustments(&out_rgb, contrast, tint);
                        if tx
                            .send(FilmSimThumbnailMsg {
                                epoch,
                                stock_idx: idx,
                                rgba: out_rgba,
                            })
                            .is_err()
                        {
                            // Receiver dropped (new epoch started), terminate worker
                            break;
                        }
                    }
                }
            })
            .ok();
    }

    /// Drain any messages from the film sim background thread and thumbnail worker.

    pub fn poll_film_sim(&mut self) {
        if let Some(rx) = self.film_sim_state.render_rx.take() {
            match rx.try_recv() {
                Ok(FilmSimMsg::Done { rgba, duration_ms }) => {
                    self.film_sim_state.preview_rgba = Some(rgba);
                    self.film_sim_state.dirty_preview_texture = true;
                    self.film_sim_state.last_render_duration_ms = duration_ms;
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_error = None;
                }
                Ok(FilmSimMsg::Error(e)) => {
                    self.film_sim_state.render_error = Some(e);
                    self.film_sim_state.is_rendering = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.film_sim_state.render_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.film_sim_state.is_rendering = false;
                }
            }
        }

        // Drain pending thumbnail messages
        let mut thumbs_done = false;
        if let Some(rx) = self.film_sim_state.thumbnail_rx.take() {
            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        if msg.epoch == self.film_sim_state.thumbnail_epoch {
                            self.film_sim_state
                                .pending_thumbnails
                                .push((msg.stock_idx, msg.rgba));
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        keep = false;
                        thumbs_done = true;
                        break;
                    }
                }
            }
            if keep {
                self.film_sim_state.thumbnail_rx = Some(rx);
            }
        }
        if thumbs_done {
            self.film_sim_state.is_generating_thumbnails = false;
        }
    }

    /// Commit the film sim preview into current_rgba and push to undo stack.

    pub fn apply_film_sim(&mut self) {
        let preview = match self.film_sim_state.preview_rgba.take() {
            Some(p) => p,
            None => return,
        };
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = preview;
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;
        // Reset sim state so re-entering the tab renders fresh.
        self.film_sim_state.preview_rgba = None;
        self.film_sim_state.preview_texture = None;
        self.film_sim_state.dirty_preview_texture = false;
        // Invalidate and regenerate thumbnails for updated image
        self.film_sim_state.stock_thumbnails.clear();
        self.film_sim_state.pending_thumbnails.clear();
        self.run_thumbnail_batch();
        self.show_toast("🎞 Film simulation applied");
    }

    /// Full 3-panel film simulation viewport (replaces the normal render_viewport when FilmSim is active).

    pub(crate) fn render_film_sim_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);

        // Upload any newly received thumbnails into GPU textures
        for (idx, rgba) in self.film_sim_state.pending_thumbnails.drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            let tex = ctx.load_texture(
                format!("film_sim_thumb_{}", idx),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.film_sim_state.stock_thumbnails.insert(idx, tex);
        }

        // Kick off thumbnail generation if empty
        if self.film_sim_state.stock_thumbnails.is_empty()
            && !self.film_sim_state.is_generating_thumbnails
            && !self.current_rgba.is_empty()
        {
            self.run_thumbnail_batch();
        }

        let panel_layout = ImageToolMode::FilmSim
            .panel_layout()
            .expect("Film Look has a panel layout")
            .with_panel_sizes(&self.panel_state);
        let panels = panel_layout.allocate(ui);
        let left = panels.left.expect("Film Look has a stock browser");
        let right = panels.right.expect("Film Look has an inspector");
        let bottom = panels.bottom.expect("Film Look has a control strip");
        let left_w = left.width();
        let left_h = left.height();
        let right_w = right.width();
        let right_h = right.height();
        let center_w = panels.center.width();
        let main_h = panels.center.height();

        let stocks = all_film_stocks();

        // Ensure fit-to-view is performed on the center canvas when first entering FilmSim
        if !self.film_sim_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.film_sim_state.auto_fitted = true;
        }

        // ── 1. LEFT PANEL: Film Stock Picker with Async 42x28 Mini-Photos ──
        crate::shell::StudioPanelLayout::show_region(ui, left, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_width(left_w - 16.0);
                    ui.set_height((left_h - 16.0).max(1.0));

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Film Stock")
                                .strong()
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(base_c),
                        );
                        ui.label(
                            egui::RichText::new(format!("({})", stocks.len()))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        if self.film_sim_state.is_generating_thumbnails {
                            ui.label(
                                egui::RichText::new("...")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            )
                            .on_hover_text("Generating thumbnails in background…");
                        }
                    });
                    ui.add_space(4.0);

                    // Search box
                    ui.add(
                        egui::TextEdit::singleline(&mut self.film_sim_state.search_query)
                            .hint_text("Filter stocks...")
                            .desired_width(left_w - 24.0),
                    );
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(2.0);

                    let search = self.film_sim_state.search_query.trim().to_lowercase();
                    egui::ScrollArea::vertical()
                        .id_source("film_sim_stock_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(left_w - 24.0);
                            let mut current_brand = "";
                            for (i, entry) in stocks.iter().enumerate() {
                                if !search.is_empty() {
                                    let matches = entry.name.to_lowercase().contains(&search)
                                        || entry.brand.to_lowercase().contains(&search)
                                        || entry.film_type_label.to_lowercase().contains(&search);
                                    if !matches {
                                        continue;
                                    }
                                }

                                // Brand separator heading
                                if entry.brand != current_brand {
                                    if !current_brand.is_empty() {
                                        ui.add_space(6.0);
                                    }
                                    ui.label(
                                        egui::RichText::new(entry.brand.to_uppercase())
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    current_brand = entry.brand;
                                }

                                let selected = self.film_sim_state.selected_stock_idx == i;
                                let (row_bg, name_col, meta_col) = if selected {
                                    (
                                        base_c,
                                        contrast_text_c32(base_c, None, palette),
                                        contrast_text_c32(base_c, None, palette),
                                    )
                                } else {
                                    (b_low, f_high, f_med)
                                };

                                let row_frame = egui::Frame::none()
                                    .fill(row_bg)
                                    .rounding(Rounding::same(4.0))
                                    .inner_margin(egui::Margin::symmetric(4.0, 3.0));

                                let resp = row_frame
                                    .show(ui, |ui| {
                                        ui.set_width(left_w - 32.0);
                                        ui.horizontal(|ui| {
                                            // 42x28 mini-photo thumbnail preview
                                            if let Some(tex) =
                                                self.film_sim_state.stock_thumbnails.get(&i)
                                            {
                                                ui.image((tex.id(), Vec2::new(42.0, 28.0)));
                                            } else {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    Vec2::new(42.0, 28.0),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    Rounding::same(2.0),
                                                    b_med,
                                                );
                                            }
                                            ui.add_space(2.0);
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(entry.name)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(name_col),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(entry.film_type_label)
                                                            .monospace()
                                                            .color(meta_col),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "ISO {}",
                                                            entry.iso
                                                        ))
                                                        .monospace()
                                                        .color(meta_col),
                                                    );
                                                });
                                            });
                                        });
                                    })
                                    .response;

                                if resp.interact(egui::Sense::click()).clicked() && !selected {
                                    self.film_sim_state.selected_stock_idx = i;
                                    self.film_sim_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }
                        });
                });
        });

        // ── 2. CENTER PANEL: Split-view Comparison Canvas ──
        crate::shell::StudioPanelLayout::show_region(ui, panels.center, |ui| {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.canvas))
                .rounding(Rounding::same(4.0))
                .show(ui, |ui| {
                    ui.set_width(center_w);
                    ui.set_height(main_h);

                    // Reload preview texture if dirty
                    if self.film_sim_state.dirty_preview_texture {
                        if let Some(ref rgba) = self.film_sim_state.preview_rgba {
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                rgba.as_raw(),
                            );
                            self.film_sim_state.preview_texture = Some(ctx.load_texture(
                                "film_sim_preview_tex",
                                ci,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        self.film_sim_state.dirty_preview_texture = false;
                    }

                    let (response, painter) = ui.allocate_painter(
                        Vec2::new(center_w, main_h),
                        egui::Sense::click_and_drag(),
                    );
                    let canvas_rect = response.rect;

                    painter.rect_filled(
                        canvas_rect,
                        Rounding::same(4.0),
                        hex_to_c32(&palette.colors.canvas),
                    );

                    // If mouse is near the comparison split divider when drag starts, initiate split dragging
                    let near_split = if let Some(pos) = response
                        .hover_pos()
                        .or_else(|| response.interact_pointer_pos())
                    {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));
                        let split_x = img_rect.min.x
                            + self.film_sim_state.comparison_split.clamp(0.01, 0.99)
                                * img_rect.width();
                        (pos.x - split_x).abs() <= 18.0
                            && pos.y >= img_rect.min.y
                            && pos.y <= img_rect.max.y
                    } else {
                        false
                    };
                    if near_split && response.drag_started() {
                        self.film_sim_state.is_dragging_split = true;
                    }

                    // Pan/zoom via this response
                    let middle_drag =
                        ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                    if (!self.film_sim_state.is_dragging_split && response.dragged()) || middle_drag
                    {
                        self.pan += response.drag_delta();
                    }
                    if response.hovered() {
                        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                        if scroll_delta.abs() > 0.0 {
                            let zoom_factor = (scroll_delta * 0.002).exp();
                            let old_zoom = self.zoom;
                            let new_zoom = (old_zoom * zoom_factor).clamp(0.05, 5.0);
                            if let Some(ptr_pos) = response.hover_pos() {
                                let center = canvas_rect.center() + self.pan;
                                let to_ptr = ptr_pos - center;
                                self.pan += to_ptr * (1.0 - new_zoom / old_zoom);
                            }
                            self.zoom = new_zoom;
                        }
                    }
                    if !self.film_sim_state.is_dragging_split && response.double_clicked() {
                        let avail_size = Vec2::new(center_w, main_h);
                        self.fit_to_view(avail_size);
                    }

                    if let Some(orig_tex) = self.texture.clone() {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));

                        draw_checkerboard(&painter, img_rect, self.zoom, b_low, b_med);

                        self.render_film_sim_comparison(
                            ui,
                            &painter,
                            &response,
                            img_rect,
                            canvas_rect,
                            ctx,
                            &orig_tex,
                            palette,
                        );

                        painter.rect_stroke(
                            img_rect,
                            Rounding::ZERO,
                            Stroke::new(
                                1.0_f32,
                                hex_to_c32_alpha(&palette.colors.text_secondary, 40),
                            ),
                        );
                    }

                    // Rendering spinner overlay
                    if self.film_sim_state.is_rendering {
                        let spinner_pos = canvas_rect.center();
                        let spin_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
                        let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                        painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                        painter.rect_stroke(
                            spin_rect,
                            Rounding::same(8.0),
                            Stroke::new(1.0_f32, b_med),
                        );
                        painter.text(
                            spinner_pos,
                            egui::Align2::CENTER_CENTER,
                            "⏳ Rendering film simulation…",
                            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                            f_med,
                        );
                    }

                    // Error overlay
                    if let Some(ref err) = self.film_sim_state.render_error.clone() {
                        let err_pos = Pos2::new(canvas_rect.min.x + 10.0, canvas_rect.min.y + 10.0);
                        painter.text(
                            err_pos,
                            egui::Align2::LEFT_TOP,
                            format!("⚠ {}", err),
                            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                            hex_to_c32(&palette.diff_del),
                        );
                    }
                });
        });

        // ── 3. RIGHT PANEL: Style + Optical Effects + Stock Info ──
        crate::shell::StudioPanelLayout::show_region(ui, right, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_width(right_w - 16.0);
                    ui.set_height((right_h - 16.0).max(1.0));

                    egui::ScrollArea::vertical()
                        .id_source("film_sim_right_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(right_w - 24.0);

                            // 1. Style
                            ui.label(
                                egui::RichText::new("Style")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(4.0);

                            let styles = [
                                (
                                    FilmStyle::Accurate,
                                    "Accurate",
                                    "Physics-faithful rendering",
                                ),
                                (
                                    FilmStyle::Artistic,
                                    "Artistic",
                                    "Boosted contrast & saturation",
                                ),
                                (FilmStyle::Vintage, "Vintage", "Warm, faded, nostalgic"),
                                (FilmStyle::HighContrast, "Hi-Con", "High contrast, punchy"),
                                (FilmStyle::Pastel, "Pastel", "Low-contrast, soft tones"),
                            ];

                            for (style, label, tooltip) in &styles {
                                let sel = self.film_sim_state.style == *style;
                                let (s_bg, s_fg) = if sel {
                                    (base_c, contrast_text_c32(base_c, None, palette))
                                } else {
                                    (b_med, f_med)
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(*label)
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .strong()
                                        .color(s_fg),
                                )
                                .fill(s_bg)
                                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                                .rounding(Rounding::same(4.0))
                                .min_size(Vec2::new(right_w - 28.0, crate::ui_ext::CONTROL_HEIGHT));

                                if ui.add(btn).on_hover_text(*tooltip).clicked() && !sel {
                                    self.film_sim_state.style = *style;
                                    self.film_sim_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }

                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

                            // 2. Optical Effects: Halation, Vignette, Light Leak, Grain
                            ui.label(
                                egui::RichText::new("Optics & Effects")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(4.0);

                            let full_w = (right_w - 32.0).max(100.0);
                            let opt_slider_w = (full_w - 68.0).max(60.0);

                            if render_context_slider(
                                ui,
                                "Halation",
                                &mut self.film_sim_state.halation,
                                0.0..=1.0,
                                0.25,
                                "%",
                                opt_slider_w,
                                palette,
                            ) {
                                self.film_sim_state.mark_dirty();
                            }
                            ui.add_space(4.0);

                            if render_context_slider(
                                ui,
                                "Vignette",
                                &mut self.film_sim_state.vignette,
                                0.0..=1.0,
                                0.35,
                                "%",
                                opt_slider_w,
                                palette,
                            ) {
                                self.film_sim_state.mark_dirty();
                            }
                            ui.add_space(4.0);

                            ui.label(
                                egui::RichText::new("Light Leak")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_med),
                            );
                            egui::ComboBox::from_id_source("film_sim_light_leak_combo")
                                .selected_text(self.film_sim_state.light_leak.label())
                                .width(full_w)
                                .show_ui(ui, |ui| {
                                    let presets = [
                                        LightLeakPreset::None,
                                        LightLeakPreset::WarmCorner,
                                        LightLeakPreset::SunStreak,
                                        LightLeakPreset::SoftEdge,
                                        LightLeakPreset::OrganicBlob,
                                    ];
                                    for p in presets {
                                        let sel = self.film_sim_state.light_leak == p;
                                        if ui.selectable_label(sel, p.label()).clicked() && !sel {
                                            self.film_sim_state.light_leak = p;
                                            self.film_sim_state.mark_dirty();
                                        }
                                    }
                                });
                            ui.add_space(4.0);

                            let mut grain_on = self.film_sim_state.enable_grain;
                            if ui
                                .checkbox(
                                    &mut grain_on,
                                    egui::RichText::new("Grain emulation")
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_med),
                                )
                                .changed()
                            {
                                self.film_sim_state.enable_grain = grain_on;
                                self.film_sim_state.mark_dirty();
                            }

                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

                            // 3. Stock metadata
                            let idx = self
                                .film_sim_state
                                .selected_stock_idx
                                .min(stocks.len().saturating_sub(1));
                            let entry = &stocks[idx];
                            ui.label(
                                egui::RichText::new("Stock Info")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(4.0);

                            let stock_built = (entry.build)();
                            egui::Grid::new("film_sim_stock_meta_grid")
                                .spacing(Vec2::new(6.0, 3.0))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("Brand:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.brand)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Film:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.name)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("ISO:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("{}", entry.iso))
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Type:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.film_type_label)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Res:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:.0} lp/mm",
                                            stock_built.resolution_lp_mm
                                        ))
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_low),
                                    );
                                    ui.end_row();
                                });

                            if self.film_sim_state.last_render_duration_ms > 0 {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "⏱ {:.2}s",
                                        self.film_sim_state.last_render_duration_ms as f64 / 1000.0
                                    ))
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                                );
                            }
                        });
                });
        });

        // ── BOTTOM STRIP: Context Sliders + WB Controls + Action Buttons ───────────
        crate::shell::StudioPanelLayout::show_region(ui, bottom, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                .show(ui, |ui| {
                    ui.set_width((bottom.width() - 24.0).max(1.0));
                    egui::ScrollArea::both()
                        .id_source("film_sim_bottom_controls")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                // Row 1: Exposure & Color Sliders with contextual start/center/end/default tracks
                                ui.horizontal(|ui| {
                                    let slider_w =
                                        ((bottom.width() - 48.0) / 6.0 - 12.0).clamp(70.0, 115.0);

                                    // Brightness
                                    if render_context_slider(
                                        ui,
                                        "Brightness",
                                        &mut self.film_sim_state.brightness,
                                        0.1..=4.0,
                                        1.0,
                                        "×",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Contrast
                                    if render_context_slider(
                                        ui,
                                        "Contrast",
                                        &mut self.film_sim_state.contrast,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Warmth
                                    if render_context_slider(
                                        ui,
                                        "Warmth",
                                        &mut self.film_sim_state.warmth,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Tint (Green ↔ Magenta)
                                    if render_context_slider(
                                        ui,
                                        "Tint",
                                        &mut self.film_sim_state.tint,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Saturation
                                    if render_context_slider(
                                        ui,
                                        "Saturation",
                                        &mut self.film_sim_state.intensity,
                                        0.0..=2.0,
                                        1.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // WB Strength
                                    if render_context_slider(
                                        ui,
                                        "WB Str",
                                        &mut self.film_sim_state.wb_strength,
                                        0.0..=1.0,
                                        1.0,
                                        "%",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                });

                                ui.add_space(3.0);

                                // Row 2: WB Mode Selector + Auto Presets + Actions
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("WB:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    let wb_modes = [
                                        (WhiteBalanceMode::Auto, "Auto"),
                                        (WhiteBalanceMode::Gray, "Gray"),
                                        (WhiteBalanceMode::White, "White"),
                                        (WhiteBalanceMode::Off, "Off"),
                                    ];
                                    for (mode, label) in wb_modes {
                                        let sel = self.film_sim_state.wb_mode == mode;
                                        let (btn_bg, btn_fg) = if sel {
                                            (base_c, contrast_text_c32(base_c, None, palette))
                                        } else {
                                            (b_med, f_med)
                                        };
                                        let b = egui::Button::new(
                                            egui::RichText::new(label)
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .strong()
                                                .color(btn_fg),
                                        )
                                        .fill(btn_bg)
                                        .stroke(Stroke::new(
                                            1.0_f32,
                                            if sel { base_c } else { b_low },
                                        ))
                                        .rounding(Rounding::same(3.0))
                                        .min_size(Vec2::new(36.0, crate::ui_ext::CONTROL_HEIGHT));
                                        if ui.add(b).clicked() && !sel {
                                            self.film_sim_state.wb_mode = mode;
                                            self.film_sim_state.mark_dirty();
                                        }
                                    }

                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(6.0);

                                    // Auto Levels
                                    let al_btn = crate::ui_ext::secondary_button_icon(
                                        Some(Icon::Wand),
                                        "Auto Levels",
                                        diff_mod,
                                        palette,
                                    );
                                    if ui
                            .add(al_btn)
                            .on_hover_text(
                                "Reset brightness to neutral, warmth to 0, and tint to 0",
                            )
                            .clicked()
                        {
                            self.film_sim_state.brightness = 1.0;
                            self.film_sim_state.warmth = 0.0;
                            self.film_sim_state.tint = 0.0;
                            self.film_sim_state.mark_dirty();
                            self.show_toast("Auto levels applied");
                        }

                                    ui.add_space(4.0);

                                    // Auto Enhance
                                    let ae_btn = crate::ui_ext::secondary_button_icon(
                                        Some(Icon::Sparkles),
                                        "Auto Enhance",
                                        base_c,
                                        palette,
                                    );
                                    if ui
                            .add(ae_btn)
                            .on_hover_text(
                                "Boost intensity, warmth, and contrast for a cinematic look",
                            )
                            .clicked()
                        {
                            self.film_sim_state.brightness =
                                (self.film_sim_state.brightness * 1.05).min(4.0);
                            self.film_sim_state.intensity =
                                (self.film_sim_state.intensity + 0.15).min(2.0);
                            self.film_sim_state.warmth =
                                (self.film_sim_state.warmth + 0.05).min(1.0);
                            self.film_sim_state.contrast =
                                (self.film_sim_state.contrast + 0.08).min(1.0);
                            self.film_sim_state.mark_dirty();
                            self.show_toast("Auto enhance applied");
                        }

                                    // Right aligned: Apply, Reset, Undo
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let apply_btn = crate::ui_ext::action_button_icon(
                                                Some(Icon::Check),
                                                "Apply to Image",
                                                diff_add,
                                                palette,
                                            );
                                            let apply_enabled =
                                                self.film_sim_state.preview_rgba.is_some();
                                            if ui
                                .add_enabled(apply_enabled, apply_btn)
                                .on_hover_text(
                                    "Commit film simulation result to image (adds to undo stack)",
                                )
                                .clicked()
                            {
                                self.apply_film_sim();
                            }

                                            ui.add_space(4.0);

                                            let reset_btn = crate::ui_ext::secondary_button_icon(
                                                Some(Icon::RotateCcw),
                                                "Reset",
                                                b_med,
                                                palette,
                                            );
                                            if ui
                                                .add(reset_btn)
                                                .on_hover_text(
                                                    "Reset all film sim parameters to defaults",
                                                )
                                                .clicked()
                                            {
                                                self.film_sim_state.brightness = 1.0;
                                                self.film_sim_state.contrast = 0.0;
                                                self.film_sim_state.warmth = 0.0;
                                                self.film_sim_state.tint = 0.0;
                                                self.film_sim_state.intensity = 1.0;
                                                self.film_sim_state.wb_mode =
                                                    WhiteBalanceMode::Auto;
                                                self.film_sim_state.wb_strength = 1.0;
                                                self.film_sim_state.halation = 0.25;
                                                self.film_sim_state.vignette = 0.35;
                                                self.film_sim_state.light_leak =
                                                    LightLeakPreset::None;
                                                self.film_sim_state.enable_grain = true;
                                                self.film_sim_state.style = FilmStyle::Accurate;
                                                self.film_sim_state.mark_dirty();
                                                self.show_toast(
                                                    "⟲ All parameters reset to defaults",
                                                );
                                            }

                                            ui.add_space(4.0);

                                            if !self.undo_stack.is_empty() {
                                                let undo_fg = contrast_text_c32(
                                                    b_med,
                                                    Some(diff_mod),
                                                    palette,
                                                );
                                                let undo_btn = egui::Button::new(
                                                    egui::RichText::new("⟲ Undo (Ctrl+Z)")
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(undo_fg),
                                                )
                                                .fill(b_med)
                                                .rounding(Rounding::same(4.0));
                                                if ui.add(undo_btn).clicked() {
                                                    self.undo();
                                                    self.film_sim_state.preview_rgba = None;
                                                    self.film_sim_state.preview_texture = None;
                                                    self.film_sim_state.mark_dirty();
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                        });
                });
        });
        crate::shell::StudioPanelLayout::show_resize_handles(
            ui,
            &panels,
            &mut self.panel_state,
            palette,
            "film_look",
        );
    }

    /// Split-view comparison painter for the Film Sim tab center canvas.
    #[allow(clippy::too_many_arguments)]

    pub(crate) fn render_film_sim_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        _ctx: &egui::Context,
        orig_tex: &egui::TextureHandle,
        palette: &AppPalette,
    ) {
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);

        // Render split comparison using unified SplitCompareView
        let split_view = crate::shell::SplitCompareView::new(
            orig_tex,
            self.film_sim_state.preview_texture.as_ref(),
            &mut self.film_sim_state.comparison_split,
            &mut self.film_sim_state.is_dragging_split,
        );
        split_view.paint(ui, response, painter, img_rect, palette);

        // HUD badges
        let badge_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
        let badge_font = egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE);
        let orig_badge_w = painter
            .layout_no_wrap("ORIGINAL".to_owned(), badge_font.clone(), b_med)
            .size()
            .x
            + 20.0;

        let orig_rect = Rect::from_min_size(
            Pos2::new(img_rect.min.x + 10.0, img_rect.min.y + 10.0),
            Vec2::new(orig_badge_w, crate::ui_ext::CONTROL_HEIGHT),
        );
        painter.rect_filled(orig_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(orig_rect, Rounding::same(4.0), Stroke::new(1.0_f32, b_med));
        painter.text(
            orig_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            badge_font.clone(),
            hex_to_c32(&palette.colors.text_secondary),
        );

        let stocks = all_film_stocks();
        let idx = self
            .film_sim_state
            .selected_stock_idx
            .min(stocks.len().saturating_sub(1));
        let sim_label = if self.film_sim_state.is_rendering {
            "⏳ Rendering…".to_string()
        } else {
            format!(
                "🎞 {} ({:.1}s)",
                stocks[idx].name,
                self.film_sim_state.last_render_duration_ms as f64 / 1000.0
            )
        };
        let sim_badge_w = painter
            .layout_no_wrap(sim_label.clone(), badge_font.clone(), base_c)
            .size()
            .x
            + 20.0;
        let sim_rect = Rect::from_min_size(
            Pos2::new(img_rect.max.x - sim_badge_w - 10.0, img_rect.min.y + 10.0),
            Vec2::new(sim_badge_w, crate::ui_ext::CONTROL_HEIGHT),
        );
        painter.rect_filled(sim_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(sim_rect, Rounding::same(4.0), Stroke::new(1.0_f32, base_c));
        painter.text(
            sim_rect.center(),
            egui::Align2::CENTER_CENTER,
            sim_label,
            badge_font,
            base_c,
        );
    }
}
