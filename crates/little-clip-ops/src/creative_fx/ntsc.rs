//! NTSC and VHS video emulation effect.

use super::DepthMaskConfig;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VhsTapeSpeed {
    #[default]
    SP, // Standard Play (highest quality)
    LP, // Long Play (moderate quality)
    EP, // Extended Play (heaviest degradation)
}

impl VhsTapeSpeed {
    pub fn all() -> &'static [VhsTapeSpeed] {
        &[VhsTapeSpeed::SP, VhsTapeSpeed::LP, VhsTapeSpeed::EP]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::SP => "SP (Standard Play - Clean)",
            Self::LP => "LP (Long Play - Moderate)",
            Self::EP => "EP (Extended Play - Worn)",
        }
    }
}

/// NTSC Chroma demodulation filter mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtscDemodulation {
    Notch,
    OneLineComb,
    TwoLineComb,
    Box,
}

impl NtscDemodulation {
    pub fn all() -> &'static [Self] {
        &[Self::Notch, Self::OneLineComb, Self::TwoLineComb, Self::Box]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Notch => "Notch (Dot Crawl)",
            Self::OneLineComb => "1-Line Comb",
            Self::TwoLineComb => "2-Line Comb",
            Self::Box => "Box Filter",
        }
    }
}

/// NTSC Chroma lowpass filtering intensity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtscChromaLowpass {
    None,
    Light,
    Full,
}

impl NtscChromaLowpass {
    pub fn all() -> &'static [Self] {
        &[Self::None, Self::Light, Self::Full]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "None (Sharp)",
            Self::Light => "Light",
            Self::Full => "Full (Soft)",
        }
    }
}

/// NTSC Scanline subcarrier phase shift (affects dot crawl alignment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtscPhaseShift {
    Degrees0,
    Degrees90,
    Degrees180,
    Degrees270,
}

impl NtscPhaseShift {
    pub fn all() -> &'static [Self] {
        &[
            Self::Degrees0,
            Self::Degrees90,
            Self::Degrees180,
            Self::Degrees270,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Degrees0 => "0°",
            Self::Degrees90 => "90°",
            Self::Degrees180 => "180°",
            Self::Degrees270 => "270°",
        }
    }
}

/// Parameters for NTSC broadcast and VHS tape emulation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NtscParams {
    // ── Broadcast RF & Signal ──
    /// Horizontal luminance smear / bandwidth restriction (0.0 to 1.0). Default is 0.25.
    pub luma_smear: f32,
    /// Composite peaking / edge sharpening (0.0 to 3.0). Default is 1.0.
    pub composite_sharpening: f32,
    /// Ringing intensity around high-contrast edges (0.0 to 2.0). Default is 0.5.
    pub ringing: f32,
    /// Analog RF static / snow intensity (0.0 to 0.05). Default is 0.001.
    pub snow: f32,
    /// Horizontal static noise streaking anisotropy (0.0 to 1.0). Default is 0.5.
    pub snow_anisotropy: f32,
    /// CRT interlaced scanlines simulation. Default is false.
    pub scanlines: bool,
    /// Subcarrier phase shift angle across scanlines. Default is 180°.
    pub phase_shift: NtscPhaseShift,
    /// Demodulation filter algorithm (Notch dot crawl vs Comb clean). Default is Notch.
    pub demodulation: NtscDemodulation,
    /// Chroma lowpass filter bandwidth. Default is Full.
    pub chroma_lowpass: NtscChromaLowpass,

    // ── Chrominance & YIQ ──
    /// Horizontal chroma offset / color bleed in pixels (-8.0 to 8.0). Default is 1.0.
    pub chroma_delay_x: f32,
    /// Vertical chroma offset / scanline shift (-4 to 4 lines). Default is 0.
    pub chroma_delay_y: i32,
    /// Vertical chroma blending across adjacent scanlines. Default is true.
    pub chroma_vert_blend: bool,
    /// Global chroma phase error / master tint rotation (0.0 to 1.0). Default is 0.0.
    pub chroma_phase_error: f32,
    /// Per-scanline chroma phase jitter / color noise (0.0 to 0.15). Default is 0.005.
    pub chroma_phase_noise: f32,
    /// Analog color grain / chroma noise intensity (0.0 to 0.50). Default is 0.05.
    pub chroma_noise: f32,

    // ── Analog Noise ──
    /// Luminance channel noise grain (0.0 to 0.50). Default is 0.01.
    pub luma_noise: f32,
    /// Composite signal noise grain (0.0 to 0.50). Default is 0.05.
    pub composite_noise: f32,

    // ── VHS Tape Simulation ──
    /// Enable VHS magnetic tape emulation pass. Default is true.
    pub vhs_enabled: bool,
    /// VHS tape recording speed / quality. Default is SP.
    pub tape_speed: VhsTapeSpeed,
    /// Horizontal wave / tape wobble displacement (0.0 to 2.0). Default is 0.5.
    pub edge_wave: f32,
    /// VCR head-switching distortion at bottom of frame. Default is true.
    pub head_switching: bool,
    /// VCR tracking noise bar intensity (0.0 to 1.0). Default is 0.0.
    pub tracking_noise: f32,
    /// VHS playback head sharpening / pre-emphasis (0.0 to 1.0). Default is 0.25.
    pub vhs_sharpen: f32,
    /// VHS tape chroma loss / color dropout fading (0.0 to 1.0). Default is 0.05.
    pub vhs_chroma_loss: f32,

    // ── Deterministic Noise ──
    /// Random seed for deterministic noise generation (0 to 1000). Default is 0.
    pub random_seed: i32,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for NtscParams {
    fn default() -> Self {
        Self {
            luma_smear: 0.25,
            composite_sharpening: 1.0,
            ringing: 0.5,
            snow: 0.001,
            snow_anisotropy: 0.5,
            scanlines: false,
            phase_shift: NtscPhaseShift::Degrees180,
            demodulation: NtscDemodulation::Notch,
            chroma_lowpass: NtscChromaLowpass::Full,
            chroma_delay_x: 1.0,
            chroma_delay_y: 0,
            chroma_vert_blend: true,
            chroma_phase_error: 0.0,
            chroma_phase_noise: 0.005,
            chroma_noise: 0.05,
            luma_noise: 0.01,
            composite_noise: 0.05,
            vhs_enabled: true,
            tape_speed: VhsTapeSpeed::SP,
            edge_wave: 0.5,
            head_switching: true,
            tracking_noise: 0.0,
            vhs_sharpen: 0.25,
            vhs_chroma_loss: 0.05,
            random_seed: 0,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Convert NtscParams to an NtscEffect configuration from ntsc-rs.
pub fn params_to_ntsc_effect(params: &NtscParams) -> ntsc_rs::NtscEffect {
    let mut effect = ntsc_rs::NtscEffect::default();
    effect.random_seed = params.random_seed;
    effect.use_field = if params.scanlines {
        ntsc_rs::settings::standard::UseField::InterleavedUpper
    } else {
        ntsc_rs::settings::standard::UseField::Both
    };
    effect.luma_smear = params.luma_smear.clamp(0.0, 1.0);
    effect.composite_sharpening = params.composite_sharpening.clamp(0.0, 3.0);
    effect.snow_intensity = params.snow.clamp(0.0, 0.1);
    effect.snow_anisotropy = params.snow_anisotropy.clamp(0.0, 1.0);

    effect.chroma_delay_horizontal = params.chroma_delay_x.clamp(-12.0, 12.0);
    effect.chroma_delay_vertical = params.chroma_delay_y.clamp(-4, 4);
    effect.chroma_vert_blend = params.chroma_vert_blend;

    effect.chroma_phase_error = params.chroma_phase_error.clamp(0.0, 1.0);
    effect.chroma_phase_noise_intensity = params.chroma_phase_noise.clamp(0.0, 0.2);

    effect.video_scanline_phase_shift = match params.phase_shift {
        NtscPhaseShift::Degrees0 => ntsc_rs::settings::standard::PhaseShift::Degrees0,
        NtscPhaseShift::Degrees90 => ntsc_rs::settings::standard::PhaseShift::Degrees90,
        NtscPhaseShift::Degrees180 => ntsc_rs::settings::standard::PhaseShift::Degrees180,
        NtscPhaseShift::Degrees270 => ntsc_rs::settings::standard::PhaseShift::Degrees270,
    };

    effect.chroma_demodulation = match params.demodulation {
        NtscDemodulation::Notch => ntsc_rs::settings::standard::ChromaDemodulationFilter::Notch,
        NtscDemodulation::OneLineComb => {
            ntsc_rs::settings::standard::ChromaDemodulationFilter::OneLineComb
        }
        NtscDemodulation::TwoLineComb => {
            ntsc_rs::settings::standard::ChromaDemodulationFilter::TwoLineComb
        }
        NtscDemodulation::Box => ntsc_rs::settings::standard::ChromaDemodulationFilter::Box,
    };

    let clow = match params.chroma_lowpass {
        NtscChromaLowpass::None => ntsc_rs::settings::standard::ChromaLowpass::None,
        NtscChromaLowpass::Light => ntsc_rs::settings::standard::ChromaLowpass::Light,
        NtscChromaLowpass::Full => ntsc_rs::settings::standard::ChromaLowpass::Full,
    };
    effect.chroma_lowpass_in = clow;
    effect.chroma_lowpass_out = clow;

    if params.chroma_noise > 0.001 {
        effect.chroma_noise.enabled = true;
        effect.chroma_noise.settings.intensity = params.chroma_noise.clamp(0.0, 1.0);
    } else {
        effect.chroma_noise.enabled = false;
    }

    if params.luma_noise > 0.001 {
        effect.luma_noise.enabled = true;
        effect.luma_noise.settings.intensity = params.luma_noise.clamp(0.0, 1.0);
    } else {
        effect.luma_noise.enabled = false;
    }

    if params.composite_noise > 0.001 {
        effect.composite_noise.enabled = true;
        effect.composite_noise.settings.intensity = params.composite_noise.clamp(0.0, 1.0);
    } else {
        effect.composite_noise.enabled = false;
    }

    if params.ringing > 0.001 {
        effect.ringing.enabled = true;
        effect.ringing.settings.intensity = params.ringing;
    } else {
        effect.ringing.enabled = false;
    }

    if params.vhs_enabled {
        effect.vhs_settings.enabled = true;
        effect.vhs_settings.settings.tape_speed = match params.tape_speed {
            VhsTapeSpeed::SP => ntsc_rs::settings::standard::VHSTapeSpeed::SP,
            VhsTapeSpeed::LP => ntsc_rs::settings::standard::VHSTapeSpeed::LP,
            VhsTapeSpeed::EP => ntsc_rs::settings::standard::VHSTapeSpeed::EP,
        };
        if params.edge_wave > 0.001 {
            effect.vhs_settings.settings.edge_wave.enabled = true;
            effect.vhs_settings.settings.edge_wave.settings.intensity = params.edge_wave;
        } else {
            effect.vhs_settings.settings.edge_wave.enabled = false;
        }
        if params.vhs_sharpen > 0.001 {
            effect.vhs_settings.settings.sharpen.enabled = true;
            effect.vhs_settings.settings.sharpen.settings.intensity = params.vhs_sharpen;
        } else {
            effect.vhs_settings.settings.sharpen.enabled = false;
        }
        effect.vhs_settings.settings.chroma_loss = params.vhs_chroma_loss * 0.0005;
    } else {
        effect.vhs_settings.enabled = false;
    }

    effect.head_switching.enabled = params.head_switching;
    if params.head_switching {
        effect.head_switching.settings.height = 8;
        effect.head_switching.settings.horiz_shift = 15.0;
    }

    if params.tracking_noise > 0.001 {
        effect.tracking_noise.enabled = true;
        effect.tracking_noise.settings.noise_intensity = params.tracking_noise;
        effect.tracking_noise.settings.wave_intensity = params.tracking_noise * 1.5;
        effect.tracking_noise.settings.height = 14;
    } else {
        effect.tracking_noise.enabled = false;
    }

    effect
}

/// Apply NTSC composite and VHS tape emulation to an RGBA image.
pub fn apply_ntsc_effect(img: &RgbaImage, params: &NtscParams) -> RgbaImage {
    apply_ntsc_effect_cancellable(img, params, None)
        .expect("uncancelled NTSC rendering cannot stop")
}

pub(super) fn apply_ntsc_effect_cancellable(
    img: &RgbaImage,
    params: &NtscParams,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if super::is_cancelled(cancel) {
        return None;
    }
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Some(img.clone());
    }

    let effect = params_to_ntsc_effect(params);
    let mut raw_bytes = img.clone().into_raw();
    let ctx = ntsc_rs::ctx::global();

    match cancel {
        Some(flag) => effect
            .apply_effect_to_buffer_cancellable::<ntsc_rs::yiq_fielding::Rgbx, u8>(
                ctx,
                (w as usize, h as usize),
                &mut raw_bytes,
                0,
                [1.0, 1.0],
                flag,
            )?,
        None => effect.apply_effect_to_buffer::<ntsc_rs::yiq_fielding::Rgbx, u8>(
            ctx,
            (w as usize, h as usize),
            &mut raw_bytes,
            0,
            [1.0, 1.0],
        ),
    }

    if super::is_cancelled(cancel) {
        return None;
    }

    if let Some(mut out) = RgbaImage::from_raw(w, h, raw_bytes) {
        // Restore original alpha channel in case YIQ conversion altered it
        for (orig_p, dest_p) in img.pixels().zip(out.pixels_mut()) {
            if super::is_cancelled(cancel) {
                return None;
            }
            dest_p[3] = orig_p[3];
        }
        Some(out)
    } else {
        Some(img.clone())
    }
}
