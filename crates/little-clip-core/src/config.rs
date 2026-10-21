//! Application configuration definitions and TOML serialization/deserialization.
//!
//! Defines schemas and default values for display preferences, appearance, audio settings,
//! image/vector external editors, workbench text editors, and automatic clipboard actions.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Root application configuration holding all sub-configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Display options (rotation, animations, debug badges, elision character).
    #[serde(default)]
    pub display: DisplayConfig,
    /// Audio preview, capture, and waveform settings.
    #[serde(default)]
    pub audio: AudioConfig,
    /// Workbench editor integrations.
    #[serde(default)]
    pub workbench: WorkbenchConfig,
    /// Raster/vector image editor and viewer tool paths.
    #[serde(default)]
    pub image: ImageConfig,
    /// Text preview and external editor configuration.
    #[serde(default)]
    pub text: TextConfig,
    /// Dual-stack slot history depths.
    #[serde(default)]
    pub slots: SlotsConfig,
    /// Automatic clip actions (e.g. link tracking parameter stripping).
    #[serde(default)]
    pub auto_actions: AutoActionsConfig,
    /// 3D model preview and inspection configuration.
    #[serde(default, alias = "model_3d", alias = "model")]
    pub model3d: Model3dConfig,
    /// Standalone Files media browsing preferences.
    #[serde(default)]
    pub file_manager: FileManagerConfig,
    /// AI/ONNX neural network model download URL configuration.
    #[serde(default, alias = "models")]
    pub ai_models: AiModelsConfig,
}

/// Standalone Files media view preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileManagerConfig {
    pub music_threshold_percent: u8,
    /// Render file-manager audio strips as spectra instead of waveforms.
    pub audio_spectrum: bool,
    /// Timeline pixels per sample: 1, 2, 4, 8, or 16 (default: 1).
    pub video_timeline_decimation: u8,
}
impl Default for FileManagerConfig {
    fn default() -> Self {
        Self {
            music_threshold_percent: 75,
            audio_spectrum: false,
            video_timeline_decimation: 1,
        }
    }
}

/// Slot depth configuration for the dual-stack model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotsConfig {
    /// Number of prior history slots for Left Hand (default: 3).
    #[serde(default = "default_left_priors")]
    pub left_priors: usize,
    /// Number of prior history slots for Right Hand (default: 1).
    #[serde(default = "default_right_priors")]
    pub right_priors: usize,
}

fn default_left_priors() -> usize {
    3
}

fn default_right_priors() -> usize {
    1
}

impl Default for SlotsConfig {
    fn default() -> Self {
        Self {
            left_priors: default_left_priors(),
            right_priors: default_right_priors(),
        }
    }
}

/// Automatic mutation actions executed upon copying new items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoActionsConfig {
    /// Automatically strip tracking query parameters (`utm_*`, `fbclid`, `si`, etc.) from copied URLs.
    #[serde(default = "default_true")]
    pub trim_link_tracking: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AutoActionsConfig {
    fn default() -> Self {
        Self {
            trim_link_tracking: true,
        }
    }
}

pub const DEFAULT_BIG_LAMA_MODEL_URL: &str =
    "https://huggingface.co/smile-forge/big-lama-onnx/resolve/main/big-lama.onnx";
pub const DEFAULT_FEYNOBG_FP16_MODEL_URL: &str =
    "https://huggingface.co/PinkPixel/fey-nobg-onnx/resolve/main/model_fp16.onnx";
pub const DEFAULT_RMBG_14_MODEL_URL: &str = DEFAULT_FEYNOBG_FP16_MODEL_URL;
pub const DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL: &str =
    "https://huggingface.co/mtbui2010/depth-anything-v2-small-ONNX/resolve/main/model.onnx";

/// AI/ONNX model download URL overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct AiModelsConfig {
    /// Optional URL override for downloading the Big-LaMa inpainting ONNX model.
    #[serde(alias = "big_lama_url", alias = "lama_model_url")]
    pub lama_url: Option<String>,
    /// Optional URL override for downloading the background removal (FeyNoBg) ONNX model.
    #[serde(
        alias = "rmbg_url",
        alias = "rmbg_14_url",
        alias = "rmbg_model_url",
        alias = "feynobg_url",
        alias = "bg_removal_url"
    )]
    pub rmbg_url: Option<String>,
    /// Optional URL override for downloading the Depth-Anything-V2 depth estimation ONNX model.
    #[serde(alias = "depth_anything_url", alias = "depth_model_url")]
    pub depth_url: Option<String>,
}

/// External tool paths for image viewing, editing, and background removal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageConfig {
    /// Preferred raster image editor (e.g. "krita", "gimp"). Defaults to "krita".
    #[serde(default = "default_image_editor")]
    pub external_editor: String,
    /// Preferred vector / SVG editor (e.g. "inkscape"). Defaults to "inkscape".
    #[serde(default = "default_svg_editor")]
    pub svg_editor: String,
    /// Preferred image viewer (e.g. "nomacs", "eog", "feh"). Defaults to "nomacs".
    #[serde(default = "default_image_viewer")]
    pub viewer: String,
    /// Background removal executable or script (e.g. "scripts/remove_bg.py" or custom command).
    /// If left None, automatically checks for local script or system Python with nobg.
    #[serde(default)]
    pub bg_removal_cmd: Option<String>,
    /// LaMa inpainting executable or script (e.g. "scripts/inpaint_lama.py" or custom command).
    /// If left None, automatically checks for local script or system Python with onnxruntime.
    #[serde(default)]
    pub lama_inpaint_cmd: Option<String>,
    /// Optional URL override for downloading the Big-LaMa inpainting ONNX model.
    #[serde(default)]
    pub lama_model_url: Option<String>,
    /// Optional URL override for downloading the RMBG-1.4 background removal ONNX model.
    #[serde(default)]
    pub rmbg_model_url: Option<String>,
    /// Optional URL override for downloading the Depth-Anything-V2 depth estimation ONNX model.
    #[serde(default)]
    pub depth_model_url: Option<String>,
}

fn default_image_editor() -> String {
    "krita".to_string()
}

fn default_svg_editor() -> String {
    "inkscape".to_string()
}

fn default_image_viewer() -> String {
    "nomacs".to_string()
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            external_editor: default_image_editor(),
            svg_editor: default_svg_editor(),
            viewer: default_image_viewer(),
            bg_removal_cmd: None,
            lama_inpaint_cmd: None,
            lama_model_url: None,
            rmbg_model_url: None,
            depth_model_url: None,
        }
    }
}

impl ImageConfig {
    /// Resolves the executable path and arguments for background removal.
    pub fn resolve_bg_removal_cmd(&self) -> (String, Vec<String>) {
        if let Some(ref cmd) = self.bg_removal_cmd {
            let parts: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
            if !parts.is_empty() {
                return (parts[0].clone(), parts[1..].to_vec());
            }
        }
        // Check relative project script
        let candidates = ["scripts/remove_bg.py", "./scripts/remove_bg.py"];
        for path in &candidates {
            if std::path::Path::new(path).exists() {
                return (path.to_string(), Vec::new());
            }
        }
        // Fall back to python3 scripts/remove_bg.py
        (
            "python3".to_string(),
            vec!["scripts/remove_bg.py".to_string()],
        )
    }

    /// Resolves the executable path and arguments for LaMa inpainting.
    pub fn resolve_lama_inpaint_cmd(&self) -> (String, Vec<String>) {
        if let Some(ref cmd) = self.lama_inpaint_cmd {
            let parts: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
            if !parts.is_empty() {
                return (parts[0].clone(), parts[1..].to_vec());
            }
        }
        let candidates = ["scripts/inpaint_lama.py", "./scripts/inpaint_lama.py"];
        for path in &candidates {
            if std::path::Path::new(path).exists() {
                return (path.to_string(), Vec::new());
            }
        }
        (
            "python3".to_string(),
            vec!["scripts/inpaint_lama.py".to_string()],
        )
    }
}

/// Shared appearance choice for the workbench and hardware display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkbenchAppearance {
    #[default]
    Dark,
    Light,
}

impl WorkbenchAppearance {
    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

/// Workbench application editor settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchConfig {
    /// Preferred text editor command (e.g. "code", "subl", "gedit", "kate", "nano", "nvim")
    /// Defaults to None, which falls back to $VISUAL, $EDITOR, or "xdg-open".
    #[serde(default)]
    pub editor: Option<String>,
    /// Fixed Light/Dark palette used by both workbench and display.
    #[serde(default)]
    pub appearance: WorkbenchAppearance,
}

impl Default for WorkbenchConfig {
    fn default() -> Self {
        Self {
            editor: None,
            appearance: WorkbenchAppearance::default(),
        }
    }
}

impl WorkbenchConfig {
    /// Resolves the command to launch the user's preferred text editor.
    pub fn resolve_editor(&self) -> String {
        if let Some(ref ed) = self.editor {
            let trimmed = ed.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Ok(vis) = std::env::var("VISUAL") {
            let trimmed = vis.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Ok(ed) = std::env::var("EDITOR") {
            let trimmed = ed.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        "xdg-open".to_string()
    }
}

/// External text editor settings for big text preview escape hatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextConfig {
    /// Preferred external text editor command (e.g. "code", "subl", "gedit", "kate", "nano", "nvim").
    /// Defaults to "code".
    #[serde(default = "default_text_editor")]
    pub editor: String,
}

fn default_text_editor() -> String {
    "code".to_string()
}

impl Default for TextConfig {
    fn default() -> Self {
        Self {
            editor: default_text_editor(),
        }
    }
}

impl TextConfig {
    /// Resolves the command to launch the user's preferred text editor.
    pub fn resolve_editor(&self) -> String {
        let trimmed = self.editor.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        if let Ok(vis) = std::env::var("VISUAL") {
            let trimmed = vis.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Ok(ed) = std::env::var("EDITOR") {
            let trimmed = ed.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        "code".to_string()
    }
}

/// 3D model loading, turntable preview, and workbench rendering configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model3dConfig {
    /// Maximum number of triangle facets rendered in the rotating preview card.
    /// Higher values increase geometric fidelity for intricate models; lower values
    /// reduce CPU usage on weaker machines. (default: 10,000).
    #[serde(
        default = "default_preview_triangle_limit",
        alias = "max_preview_triangles",
        alias = "max_thumbnail_triangles",
        alias = "triangle_limit"
    )]
    pub preview_triangle_limit: usize,

    /// Maximum number of triangle facets rendered in the interactive 3D workbench studio viewer.
    /// (default: 150,000).
    #[serde(
        default = "default_workbench_triangle_limit",
        alias = "max_workbench_triangles",
        alias = "workbench_triangle_limit"
    )]
    pub workbench_triangle_limit: usize,
}

fn default_preview_triangle_limit() -> usize {
    10_000
}

fn default_workbench_triangle_limit() -> usize {
    150_000
}

impl Default for Model3dConfig {
    fn default() -> Self {
        Self {
            preview_triangle_limit: default_preview_triangle_limit(),
            workbench_triangle_limit: default_workbench_triangle_limit(),
        }
    }
}

/// Preferred position for sampling a preview snippet in long audio tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSamplePosition {
    /// Sample the snippet centered around the track's midpoint.
    Middle,
    /// Sample the snippet starting from the track's beginning.
    Start,
}

impl Default for AudioSamplePosition {
    fn default() -> Self {
        AudioSamplePosition::Middle
    }
}

/// Audio decoding, rolling capture, and waveform preview configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Threshold (in seconds) above which audio clips use a snippet preview instead of full waveform (default: 300 = 5 min).
    #[serde(default = "default_max_full_waveform_sec")]
    pub max_full_waveform_sec: u64,
    /// Duration (in seconds) of the snippet to preview for long audio files (default: 30).
    #[serde(default = "default_snippet_duration_sec")]
    pub snippet_duration_sec: u64,
    /// Where to sample the snippet from for long audio files: "middle" (default) or "start".
    #[serde(default)]
    pub sample_position: AudioSamplePosition,
    /// Continuous rolling capture duration in seconds (default: 30).
    #[serde(default = "default_rolling_capture_duration_sec")]
    pub rolling_capture_duration_sec: u64,
    /// Whether background rolling audio capture is enabled (default: true).
    #[serde(default = "default_rolling_capture_enabled")]
    pub rolling_capture_enabled: bool,
    /// Optional specific capture device/sink name (default: None, captures `@DEFAULT_SINK@`).
    #[serde(default)]
    pub rolling_capture_device: Option<String>,
    /// Preferred external audio editor escape hatch (e.g. "audacity"). Defaults to "audacity".
    #[serde(default = "default_audio_editor")]
    pub editor: String,
}

fn default_max_full_waveform_sec() -> u64 {
    300
}

fn default_snippet_duration_sec() -> u64 {
    30
}

fn default_rolling_capture_duration_sec() -> u64 {
    30
}

fn default_rolling_capture_enabled() -> bool {
    true
}

fn default_audio_editor() -> String {
    "audacity".to_string()
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            max_full_waveform_sec: 300,
            snippet_duration_sec: 30,
            sample_position: AudioSamplePosition::Middle,
            rolling_capture_duration_sec: 30,
            rolling_capture_enabled: true,
            rolling_capture_device: None,
            editor: default_audio_editor(),
        }
    }
}

/// Display rendering preferences for the small USB screen and Workbench canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// Invert the display 180 degrees (true for standard Trofeo Vision mounting).
    pub rotate_180: bool,
    /// Display active handler badge in bottom-left corner.
    pub show_debug_handler: bool,
    /// Minimum interval between animation frame updates to the small USB display in milliseconds (default: 350ms).
    #[serde(default = "default_animation_interval_ms")]
    pub animation_interval_ms: u64,
    /// Character / glyph used for truncating long filenames (default: "▸").
    #[serde(default = "default_elision_char")]
    pub elision_char: String,
    /// Color for the elision character: "border" to dynamically match the slot box's border color (default),
    /// or any hex color code (e.g. "#f59e0b", "#38bdf8").
    #[serde(default = "default_elision_color")]
    pub elision_color: String,
}

fn default_animation_interval_ms() -> u64 {
    350
}

fn default_elision_char() -> String {
    "▸".to_string()
}

fn default_elision_color() -> String {
    "border".to_string()
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            rotate_180: true,
            show_debug_handler: true,
            animation_interval_ms: 350,
            elision_char: "▸".to_string(),
            elision_color: "border".to_string(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            display: DisplayConfig::default(),
            audio: AudioConfig::default(),
            workbench: WorkbenchConfig::default(),
            image: ImageConfig::default(),
            text: TextConfig::default(),
            slots: SlotsConfig::default(),
            auto_actions: AutoActionsConfig::default(),
            model3d: Model3dConfig::default(),
            file_manager: FileManagerConfig::default(),
            ai_models: AiModelsConfig::default(),
        }
    }
}

impl AppConfig {
    /// Loads configuration from either:
    /// 1. Local `./config.toml`
    /// 2. User config at `$XDG_CONFIG_HOME/little-screen-clip/config.toml`
    /// 3. Creates default template if neither exists
    pub fn load() -> Self {
        let local_path = Path::new("config.toml");
        if local_path.exists() {
            match Self::load_from_path(local_path) {
                Ok(config) => {
                    info!("Loaded configuration from local ./config.toml");
                    return config;
                }
                Err(error) => warn!(
                    "Rejected local configuration at {}: {error}; trying user configuration",
                    local_path.display()
                ),
            }
        }

        if let Some(user_path) = get_user_config_path() {
            if user_path.exists() {
                match Self::load_from_path(&user_path) {
                    Ok(config) => {
                        info!("Loaded configuration from {}", user_path.display());
                        return config;
                    }
                    Err(error) => warn!(
                        "Rejected user configuration at {}: {error}; using defaults",
                        user_path.display()
                    ),
                }
            } else {
                // Auto-create default config template
                let default_cfg = Self::default();
                if let Some(parent) = user_path.parent() {
                    if let Err(err) = std::fs::create_dir_all(parent) {
                        warn!(
                            "Failed to create directory for configuration template at {}: {err}",
                            parent.display()
                        );
                    }
                }
                match toml::to_string_pretty(&default_cfg) {
                    Ok(toml_str) => match std::fs::write(&user_path, toml_str) {
                        Ok(()) => info!(
                            "Created default configuration file at {}",
                            user_path.display()
                        ),
                        Err(err) => warn!(
                            "Failed to write default configuration template to {}: {err}",
                            user_path.display()
                        ),
                    },
                    Err(err) => warn!("Failed to serialize default configuration: {err}"),
                }
                return default_cfg;
            }
        }

        warn!("Using in-memory default configuration");
        Self::default()
    }

    /// Deserializes an `AppConfig` from a TOML file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if TOML deserialization fails.
    pub fn load_from_path(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Resolves the shared fixed Light/Dark colors for the workbench and display.
    pub fn resolve_palette(&self) -> crate::palette::AppPalette {
        crate::palette::AppPalette::for_appearance(self.workbench.appearance)
    }

    /// Save the shared appearance choice, preserving all other settings.
    pub fn save_workbench_appearance_to_file(
        appearance: WorkbenchAppearance,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let value = match appearance {
            WorkbenchAppearance::Dark => "dark",
            WorkbenchAppearance::Light => "light",
        };
        save_setting_to_file("workbench", "appearance", value)
    }

    /// Save the Files audio-strip preference, preserving unrelated settings.
    pub fn save_file_manager_audio_spectrum_to_file(
        spectrum: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        save_setting_to_file("file_manager", "audio_spectrum", spectrum)
    }

    /// URL for downloading the Big-LaMa inpainting ONNX model.
    pub fn lama_model_url(&self) -> &str {
        self.ai_models
            .lama_url
            .as_deref()
            .or(self.image.lama_model_url.as_deref())
            .unwrap_or(DEFAULT_BIG_LAMA_MODEL_URL)
    }

    /// URL for downloading the background removal (FeyNoBg) ONNX model.
    pub fn rmbg_model_url(&self) -> &str {
        self.ai_models
            .rmbg_url
            .as_deref()
            .or(self.image.rmbg_model_url.as_deref())
            .unwrap_or(DEFAULT_FEYNOBG_FP16_MODEL_URL)
    }

    /// URL for downloading the background removal (FeyNoBg) ONNX model.
    pub fn bg_removal_model_url(&self) -> &str {
        self.rmbg_model_url()
    }

    /// URL for downloading the Depth-Anything-V2 depth estimation ONNX model.
    pub fn depth_model_url(&self) -> &str {
        self.ai_models
            .depth_url
            .as_deref()
            .or(self.image.depth_model_url.as_deref())
            .unwrap_or(DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL)
    }
}

/// Finds the active config file path, prioritizing local `config.toml` if present.
pub fn find_config_file_path() -> PathBuf {
    if Path::new("config.toml").exists() {
        PathBuf::from("config.toml")
    } else if let Some(p) = get_user_config_path() {
        if p.exists() {
            p
        } else {
            PathBuf::from("config.toml")
        }
    } else {
        PathBuf::from("config.toml")
    }
}

fn save_setting_to_file(
    table: &str,
    setting: &str,
    value: impl Into<toml::Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = find_config_file_path();
    let current = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    std::fs::write(
        &path,
        update_setting_in_text(&current, table, setting, value),
    )?;
    Ok(())
}

/// Update one scalar setting without rewriting the rest of the TOML file.
fn update_setting_in_text(
    text: &str,
    table: &str,
    setting: &str,
    value: impl Into<toml::Value>,
) -> String {
    let value = value.into().to_string();
    let mut output = Vec::new();
    let mut in_table = false;
    let mut updated = false;
    let mut table_found = false;

    for line in text.lines() {
        let trimmed = line.trim();
        // Check for a table header such as [workbench] or [display].
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_table && !updated {
                output.push(format!("{setting} = {value}"));
                updated = true;
            }
            in_table = trimmed == format!("[{table}]");
            table_found |= in_table;
            output.push(line.to_string());
            continue;
        }

        if in_table && !updated {
            let key = trimmed.split('=').next().unwrap_or("").trim();
            if key == setting {
                let leading: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                output.push(format!("{leading}{setting} = {value}"));
                updated = true;
                continue;
            }
        }

        output.push(line.to_string());
    }

    if in_table && !updated {
        output.push(format!("{setting} = {value}"));
    }

    if !table_found {
        if !output.is_empty() && !output.last().map(|s| s.is_empty()).unwrap_or(false) {
            output.push(String::new());
        }
        output.push(format!("[{table}]"));
        output.push(format!("{setting} = {value}"));
    }

    let mut res = output.join("\n");
    if text.ends_with('\n') {
        res.push('\n');
    }
    res
}

/// Computes the default user configuration file path according to XDG base directory specifications.
pub fn get_user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(xdg).join("little-screen-clip/config.toml"))
    } else if let Ok(home) = std::env::var("HOME") {
        Some(PathBuf::from(home).join(".config/little-screen-clip/config.toml"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_spectrum_persists_as_a_boolean_without_rewriting_other_settings() {
        let defaults: AppConfig = toml::from_str("").unwrap();
        assert!(!defaults.file_manager.audio_spectrum);
        let original = "# custom preferences\n[workbench]\nappearance = \"light\"\n[file_manager]\n# timeline quality\nvideo_timeline_decimation = 4\n";
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let enabled = update_setting_in_text(original, "file_manager", "audio_spectrum", true);
        assert!(enabled.contains("audio_spectrum = true"));
        assert!(enabled.starts_with(original.trim_end()));
        std::fs::write(&path, &enabled).unwrap();
        let loaded = AppConfig::load_from_path(&path).unwrap();
        assert!(loaded.file_manager.audio_spectrum);
        assert_eq!(loaded.file_manager.video_timeline_decimation, 4);
        assert_eq!(loaded.workbench.appearance, WorkbenchAppearance::Light);
        let disabled = update_setting_in_text(&enabled, "file_manager", "audio_spectrum", false);
        assert_eq!(disabled.matches("audio_spectrum =").count(), 1);
        std::fs::write(&path, disabled).unwrap();
        assert!(
            !AppConfig::load_from_path(&path)
                .unwrap()
                .file_manager
                .audio_spectrum
        );
    }

    #[test]
    fn files_music_threshold_defaults_and_accepts_configuration() {
        let defaults: AppConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.file_manager.music_threshold_percent, 75);
        let configured: AppConfig =
            toml::from_str("[file_manager]\nmusic_threshold_percent = 60\n").unwrap();
        assert_eq!(configured.file_manager.music_threshold_percent, 60);
    }

    #[test]
    fn files_video_decimation_defaults_to_native_resolution() {
        let defaults: AppConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.file_manager.video_timeline_decimation, 1);
        for factor in [1, 2, 4, 8, 16] {
            let configured: AppConfig = toml::from_str(&format!(
                "[file_manager]\nvideo_timeline_decimation = {factor}\n"
            ))
            .unwrap();
            assert_eq!(configured.file_manager.video_timeline_decimation, factor);
        }
    }

    #[test]
    fn workbench_appearance_save_preserves_unrelated_config() {
        let original = "[colors]\ntheme = \"zenburn\"\n\n[workbench]\n# editor preference\neditor = \"code\"\n";
        let updated = update_setting_in_text(original, "workbench", "appearance", "light");
        assert!(updated.contains("theme = \"zenburn\""));
        assert!(updated.contains("# editor preference\neditor = \"code\""));
        let config: AppConfig = toml::from_str(&updated).unwrap();
        assert_eq!(config.workbench.appearance, WorkbenchAppearance::Light);

        let reversed = update_setting_in_text(&updated, "workbench", "appearance", "dark");
        assert_eq!(reversed.matches("appearance =").count(), 1);
        assert_eq!(
            toml::from_str::<AppConfig>(&reversed)
                .unwrap()
                .workbench
                .appearance,
            WorkbenchAppearance::Dark
        );
    }

    #[test]
    fn fixed_display_palette_follows_shared_appearance_and_ignores_legacy_colors() {
        let config: AppConfig = toml::from_str(
            r##"[colors]
theme = "zenburn"
background = "#010101"

[workbench]
appearance = "light"
"##,
        )
        .unwrap();
        let light = config.resolve_palette();
        assert_eq!(light.colors.canvas, "#eeeeee");
        assert_ne!(light.colors.canvas, "#010101");
        assert!(!light.is_dark);

        let mut dark_config = config;
        dark_config.workbench.appearance = WorkbenchAppearance::Dark;
        let dark = dark_config.resolve_palette();
        assert_eq!(dark.colors.canvas, "#1f1f1f");
        assert!(dark.is_dark);
        assert_ne!(light.colors.canvas, dark.colors.canvas);
    }

    #[test]
    fn test_model3d_config_defaults_and_overrides() {
        let default_cfg: AppConfig = toml::from_str("").unwrap();
        assert_eq!(default_cfg.model3d.preview_triangle_limit, 10_000);
        assert_eq!(default_cfg.model3d.workbench_triangle_limit, 150_000);

        let custom_toml = r#"
[model3d]
preview_triangle_limit = 4000
workbench_triangle_limit = 80000
"#;
        let custom_cfg: AppConfig = toml::from_str(custom_toml).unwrap();
        assert_eq!(custom_cfg.model3d.preview_triangle_limit, 4000);
        assert_eq!(custom_cfg.model3d.workbench_triangle_limit, 80_000);

        // Test alias support: triangle_limit under [model_3d]
        let alias_toml = r#"
[model_3d]
triangle_limit = 2500
"#;
        let alias_cfg: AppConfig = toml::from_str(alias_toml).unwrap();
        assert_eq!(alias_cfg.model3d.preview_triangle_limit, 2500);
    }

    #[test]
    fn test_ai_model_url_defaults_and_overrides() {
        let default_cfg: AppConfig = toml::from_str("").unwrap();
        assert_eq!(default_cfg.lama_model_url(), DEFAULT_BIG_LAMA_MODEL_URL);
        assert_eq!(default_cfg.rmbg_model_url(), DEFAULT_RMBG_14_MODEL_URL);
        assert_eq!(
            default_cfg.depth_model_url(),
            DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL
        );

        // Test overrides under [ai_models]
        let ai_models_toml = r#"
[ai_models]
lama_url = "https://custom.mirror/lama.onnx"
feynobg_url = "https://custom.mirror/feynobg.onnx"
depth_url = "https://custom.mirror/depth.onnx"
"#;
        let ai_cfg: AppConfig = toml::from_str(ai_models_toml).unwrap();
        assert_eq!(ai_cfg.lama_model_url(), "https://custom.mirror/lama.onnx");
        assert_eq!(
            ai_cfg.rmbg_model_url(),
            "https://custom.mirror/feynobg.onnx"
        );
        assert_eq!(
            ai_cfg.bg_removal_model_url(),
            "https://custom.mirror/feynobg.onnx"
        );
        assert_eq!(ai_cfg.depth_model_url(), "https://custom.mirror/depth.onnx");

        // Test overrides under [image]
        let image_toml = r#"
[image]
lama_model_url = "https://image.mirror/lama.onnx"
rmbg_model_url = "https://image.mirror/rmbg.onnx"
depth_model_url = "https://image.mirror/depth.onnx"
"#;
        let img_cfg: AppConfig = toml::from_str(image_toml).unwrap();
        assert_eq!(img_cfg.lama_model_url(), "https://image.mirror/lama.onnx");
        assert_eq!(img_cfg.rmbg_model_url(), "https://image.mirror/rmbg.onnx");
        assert_eq!(img_cfg.depth_model_url(), "https://image.mirror/depth.onnx");

        // Test [ai_models] taking precedence over [image]
        let both_toml = r#"
[image]
lama_model_url = "https://image.mirror/lama.onnx"

[ai_models]
lama_url = "https://ai.mirror/lama.onnx"
"#;
        let both_cfg: AppConfig = toml::from_str(both_toml).unwrap();
        assert_eq!(both_cfg.lama_model_url(), "https://ai.mirror/lama.onnx");
    }
}
