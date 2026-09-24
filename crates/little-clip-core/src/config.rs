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
    pub fn resolve_palette(&self) -> crate::theme::AppPalette {
        crate::theme::AppPalette::for_appearance(self.workbench.appearance)
    }

    /// Save the shared appearance choice, preserving all other settings.
    pub fn save_workbench_appearance_to_file(
        appearance: WorkbenchAppearance,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = find_config_file_path();
        let current = if path.exists() {
            std::fs::read_to_string(&path)?
        } else {
            String::new()
        };
        let value = match appearance {
            WorkbenchAppearance::Dark => "dark",
            WorkbenchAppearance::Light => "light",
        };
        std::fs::write(
            &path,
            update_setting_in_text(&current, "workbench", "appearance", value),
        )?;
        Ok(())
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

/// Update one quoted string setting without rewriting the rest of the TOML file.
fn update_setting_in_text(text: &str, table: &str, setting: &str, value: &str) -> String {
    let mut output = Vec::new();
    let mut in_table = false;
    let mut updated = false;
    let mut table_found = false;

    for line in text.lines() {
        let trimmed = line.trim();
        // Check for a table header such as [workbench] or [display].
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_table && !updated {
                output.push(format!("{setting} = \"{value}\""));
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
                output.push(format!("{leading}{setting} = \"{value}\""));
                updated = true;
                continue;
            }
        }

        output.push(line.to_string());
    }

    if in_table && !updated {
        output.push(format!("{setting} = \"{value}\""));
    }

    if !table_found {
        if !output.is_empty() && !output.last().map(|s| s.is_empty()).unwrap_or(false) {
            output.push(String::new());
        }
        output.push(format!("[{table}]"));
        output.push(format!("{setting} = \"{value}\""));
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
        assert_eq!(light.colors.canvas, "#f4f2ea");
        assert_ne!(light.background, "#010101");
        assert!(!light.is_dark);

        let mut dark_config = config;
        dark_config.workbench.appearance = WorkbenchAppearance::Dark;
        let dark = dark_config.resolve_palette();
        assert_eq!(dark.colors.canvas, "#151c24");
        assert!(dark.is_dark);
        assert_ne!(light.background, dark.background);
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
}
