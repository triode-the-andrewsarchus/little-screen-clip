//! Application configuration definitions and TOML serialization/deserialization.
//!
//! Defines schemas and default values for display preferences, color themes, audio settings,
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
    /// Color theme palette for canvas, slots, borders, and text.
    #[serde(default)]
    pub colors: ColorConfig,
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

/// Desktop appearance choices are separate from hardware display themes.
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
    /// Desktop workbench appearance, independent of the hardware display theme.
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

fn default_theme() -> String {
    "apollo".to_string()
}

/// Theme color palette for backgrounds, slot card fills, borders, and text labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorConfig {
    /// Active Hundred Rabbits theme name (e.g. "apollo", "peppermint", "nord").
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Optional override: Main canvas background color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Optional override: Background fill for Left Hand and Right Hand cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero_card_bg: Option<String>,
    /// Optional override: Background fill for L Prior and R Prior cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_card_bg: Option<String>,
    /// Optional override: Background fill for Undo card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_card_bg: Option<String>,

    // Border Colors
    /// Optional override: Border stroke color for the Undo slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_border: Option<String>,
    /// Optional override: Border stroke color for the Left Hand slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_hand_border: Option<String>,
    /// Optional override: Border stroke color for Left Prior 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_prior1_border: Option<String>,
    /// Optional override: Border stroke color for Left Prior 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_prior2_border: Option<String>,
    /// Optional override: Border stroke color for Left Prior 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_prior3_border: Option<String>,
    /// Optional override: Border stroke color for Right Prior 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_prior2_border: Option<String>,
    /// Optional override: Border stroke color for Right Prior 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_prior1_border: Option<String>,
    /// Optional override: Border stroke color for the Right Hand slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_hand_border: Option<String>,
    /// Optional override: Focused cursor border color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_border: Option<String>,
    /// Optional override: Multi-selected slot border color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_border: Option<String>,

    // Text & Element Colors
    /// Optional override: Primary title font color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_color: Option<String>,
    /// Optional override: Secondary subtitle font color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_color: Option<String>,
    /// Optional override: Body text color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_color: Option<String>,
    /// Optional override: URL text highlight color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_color: Option<String>,
    /// Optional override: Debug handler badge background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_badge_bg: Option<String>,
    /// Optional override: Debug handler badge text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_badge_text: Option<String>,
}

impl ColorConfig {
    /// Resolves the full `AppPalette` using the specified theme and any explicit overrides.
    pub fn resolve_palette(&self) -> crate::theme::AppPalette {
        let themes_dir = crate::theme::find_themes_dir();
        let theme_file = themes_dir.join(format!("{}.svg", self.theme));
        let theme = if theme_file.exists() {
            crate::theme::Theme::from_file(&theme_file)
                .unwrap_or_else(|_| crate::theme::Theme::apollo())
        } else {
            crate::theme::Theme::apollo()
        };

        let mut overrides = std::collections::HashMap::new();
        if let Some(v) = &self.background {
            overrides.insert("background".to_string(), v.clone());
        }
        if let Some(v) = &self.hero_card_bg {
            overrides.insert("hero_card_bg".to_string(), v.clone());
        }
        if let Some(v) = &self.prior_card_bg {
            overrides.insert("prior_card_bg".to_string(), v.clone());
        }
        if let Some(v) = &self.undo_card_bg {
            overrides.insert("undo_card_bg".to_string(), v.clone());
        }

        if let Some(v) = &self.undo_border {
            overrides.insert("undo_border".to_string(), v.clone());
        }
        if let Some(v) = &self.left_hand_border {
            overrides.insert("left_hand_border".to_string(), v.clone());
        }
        if let Some(v) = &self.left_prior1_border {
            overrides.insert("left_prior1_border".to_string(), v.clone());
        }
        if let Some(v) = &self.left_prior2_border {
            overrides.insert("left_prior2_border".to_string(), v.clone());
        }
        if let Some(v) = &self.left_prior3_border {
            overrides.insert("left_prior3_border".to_string(), v.clone());
        }
        if let Some(v) = &self.right_prior2_border {
            overrides.insert("right_prior2_border".to_string(), v.clone());
        }
        if let Some(v) = &self.right_prior1_border {
            overrides.insert("right_prior1_border".to_string(), v.clone());
        }
        if let Some(v) = &self.right_hand_border {
            overrides.insert("right_hand_border".to_string(), v.clone());
        }
        if let Some(v) = &self.cursor_border {
            overrides.insert("cursor_border".to_string(), v.clone());
        }
        if let Some(v) = &self.selected_border {
            overrides.insert("selected_border".to_string(), v.clone());
        }

        if let Some(v) = &self.title_color {
            overrides.insert("title_color".to_string(), v.clone());
        }
        if let Some(v) = &self.subtitle_color {
            overrides.insert("subtitle_color".to_string(), v.clone());
        }
        if let Some(v) = &self.body_color {
            overrides.insert("body_color".to_string(), v.clone());
        }
        if let Some(v) = &self.url_color {
            overrides.insert("url_color".to_string(), v.clone());
        }
        if let Some(v) = &self.debug_badge_bg {
            overrides.insert("debug_badge_bg".to_string(), v.clone());
        }
        if let Some(v) = &self.debug_badge_text {
            overrides.insert("debug_badge_text".to_string(), v.clone());
        }

        crate::theme::AppPalette::from_theme(&theme, &overrides)
    }

    /// Clears all manual overrides, reverting to purely theme-derived colors.
    pub fn clear_overrides(&mut self) {
        self.background = None;
        self.hero_card_bg = None;
        self.prior_card_bg = None;
        self.undo_card_bg = None;
        self.undo_border = None;
        self.left_hand_border = None;
        self.left_prior1_border = None;
        self.left_prior2_border = None;
        self.left_prior3_border = None;
        self.right_prior2_border = None;
        self.right_prior1_border = None;
        self.right_hand_border = None;
        self.cursor_border = None;
        self.selected_border = None;
        self.title_color = None;
        self.subtitle_color = None;
        self.body_color = None;
        self.url_color = None;
        self.debug_badge_bg = None;
        self.debug_badge_text = None;
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            background: None,
            hero_card_bg: None,
            prior_card_bg: None,
            undo_card_bg: None,
            undo_border: None,
            left_hand_border: None,
            left_prior1_border: None,
            left_prior2_border: None,
            left_prior3_border: None,
            right_prior2_border: None,
            right_prior1_border: None,
            right_hand_border: None,
            cursor_border: None,
            selected_border: None,
            title_color: None,
            subtitle_color: None,
            body_color: None,
            url_color: None,
            debug_badge_bg: None,
            debug_badge_text: None,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            display: DisplayConfig::default(),
            colors: ColorConfig::default(),
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

    /// Resolves the active theme and color overrides into the unified `AppPalette`.
    pub fn resolve_palette(&self) -> crate::theme::AppPalette {
        self.colors.resolve_palette()
    }

    /// Saves the current configuration to disk (non-destructively updating theme).
    ///
    /// # Errors
    ///
    /// Returns an error if reading or writing the configuration file fails.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        Self::save_theme_to_file(&self.colors.theme)
    }

    /// Non-destructively updates the active theme in `config.toml` without modifying comments,
    /// display rotation, or other user configuration settings.
    ///
    /// # Errors
    ///
    /// Returns an error if reading or writing the configuration file fails.
    pub fn save_theme_to_file(new_theme: &str) -> Result<(), Box<dyn std::error::Error>> {
        let path = find_config_file_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let updated = update_theme_in_text(&content, new_theme);
            std::fs::write(&path, updated)?;
        } else {
            let new_content = format!("[colors]\ntheme = \"{}\"\n", new_theme);
            std::fs::write(&path, new_content)?;
        }
        Ok(())
    }

    /// Save only the desktop appearance, preserving the display theme and other settings.
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

    /// Non-destructively clears color overrides in `config.toml` by commenting them out,
    /// preserving all user comments and other configuration sections.
    pub fn clear_overrides_in_file() -> Result<(), Box<dyn std::error::Error>> {
        let path = find_config_file_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let updated = clear_overrides_in_text(&content);
            std::fs::write(&path, updated)?;
        }
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

/// Non-destructively updates the `theme = "..."` entry under `[colors]` in TOML text,
/// preserving all existing comments, other sections, indentation, and formatting.
pub fn update_theme_in_text(text: &str, new_theme: &str) -> String {
    update_setting_in_text(text, "colors", "theme", new_theme)
}

/// Update one quoted string setting without rewriting the rest of the TOML file.
fn update_setting_in_text(text: &str, table: &str, setting: &str, value: &str) -> String {
    let mut output = Vec::new();
    let mut in_table = false;
    let mut updated = false;
    let mut table_found = false;

    for line in text.lines() {
        let trimmed = line.trim();
        // Check for table header like [colors] or [display]
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

/// Non-destructively clears overrides under `[colors]` by commenting out non-theme key-values,
/// preserving all comments and other configuration sections.
pub fn clear_overrides_in_text(text: &str) -> String {
    let mut output = Vec::new();
    let mut in_colors = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_colors = trimmed == "[colors]";
            output.push(line.to_string());
            continue;
        }

        if in_colors && !trimmed.starts_with('#') && trimmed.contains('=') {
            let key = trimmed.split('=').next().unwrap_or("").trim();
            if key != "theme" {
                // Comment out this override line
                output.push(format!("# {}", line));
                continue;
            }
        }

        output.push(line.to_string());
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
    fn test_non_destructive_update_theme_preserves_everything() {
        let original = r##"# Main Header Comment
[display]
rotate_180 = true # Must be preserved!
show_debug_handler = false

[colors]
# Theme comment
theme = "apollo"
# An override comment
left_hand_border = "#6366f1"

[audio]
editor = "audacity"
"##;

        let updated = update_theme_in_text(original, "boysenberry");
        assert!(updated.contains("rotate_180 = true # Must be preserved!"));
        assert!(updated.contains("# Main Header Comment"));
        assert!(updated.contains("theme = \"boysenberry\""));
        assert!(!updated.contains("theme = \"apollo\""));
        assert!(updated.contains("left_hand_border = \"#6366f1\""));
        assert!(updated.contains("[audio]\neditor = \"audacity\""));
    }

    #[test]
    fn workbench_appearance_updates_without_touching_display_theme() {
        let original = "[colors]\ntheme = \"zenburn\"\n\n[workbench]\n# editor preference\neditor = \"code\"\n";
        let updated = update_setting_in_text(original, "workbench", "appearance", "light");
        assert!(updated.contains("theme = \"zenburn\""));
        assert!(updated.contains("# editor preference\neditor = \"code\""));
        let config: AppConfig = toml::from_str(&updated).unwrap();
        assert_eq!(config.workbench.appearance, WorkbenchAppearance::Light);
        assert_eq!(config.colors.theme, "zenburn");

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
    fn test_non_destructive_clear_overrides() {
        let original = r##"[display]
rotate_180 = true

[colors]
theme = "apollo"
hero_card_bg = "#181b26"
left_hand_border = "#6366f1"
# Existing comment
"##;

        let cleared = clear_overrides_in_text(original);
        assert!(cleared.contains("rotate_180 = true"));
        assert!(cleared.contains("theme = \"apollo\""));
        assert!(cleared.contains("# hero_card_bg = \"#181b26\""));
        assert!(cleared.contains("# left_hand_border = \"#6366f1\""));
        assert!(cleared.contains("# Existing comment"));
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
