use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub colors: ColorConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub workbench: WorkbenchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkbenchConfig {
    /// Preferred text editor command (e.g. "code", "subl", "gedit", "kate", "nano", "nvim")
    /// Defaults to None, which falls back to $VISUAL, $EDITOR, or "xdg-open".
    #[serde(default)]
    pub editor: Option<String>,
}

impl Default for WorkbenchConfig {
    fn default() -> Self {
        Self { editor: None }
    }
}

impl WorkbenchConfig {
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


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSamplePosition {
    Middle,
    Start,
}

impl Default for AudioSamplePosition {
    fn default() -> Self {
        AudioSamplePosition::Middle
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Threshold (in seconds) above which audio clips use a snippet preview instead of full waveform (default: 300 = 5 min)
    #[serde(default = "default_max_full_waveform_sec")]
    pub max_full_waveform_sec: u64,
    /// Duration (in seconds) of the snippet to preview for long audio files (default: 30)
    #[serde(default = "default_snippet_duration_sec")]
    pub snippet_duration_sec: u64,
    /// Where to sample the snippet from for long audio files: "middle" (default) or "start"
    #[serde(default)]
    pub sample_position: AudioSamplePosition,
}

fn default_max_full_waveform_sec() -> u64 {
    300
}

fn default_snippet_duration_sec() -> u64 {
    30
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            max_full_waveform_sec: 300,
            snippet_duration_sec: 30,
            sample_position: AudioSamplePosition::Middle,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// Invert the display 180 degrees (true for standard Trofeo Vision mounting)
    pub rotate_180: bool,
    /// Display active handler badge in bottom-left corner
    pub show_debug_handler: bool,
    /// Minimum interval between animation frame updates to the small USB display in milliseconds (default: 350ms)
    #[serde(default = "default_animation_interval_ms")]
    pub animation_interval_ms: u64,
    /// Character / glyph used for truncating long filenames (default: "▸")
    #[serde(default = "default_elision_char")]
    pub elision_char: String,
    /// Color for the elision character: "border" to dynamically match the slot box's border color (default),
    /// or any hex color code (e.g. "#f59e0b", "#38bdf8")
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorConfig {
    /// Main canvas background
    pub background: String,
    /// Background fill for Left Hand and Right Hand
    pub hero_card_bg: String,
    /// Background fill for L Prior and R Prior cards
    pub prior_card_bg: String,
    /// Background fill for Undo card
    pub undo_card_bg: String,

    // Border Colors
    pub undo_border: String,
    pub left_hand_border: String,
    pub left_prior1_border: String,
    pub left_prior2_border: String,
    pub right_prior2_border: String,
    pub right_prior1_border: String,
    pub right_hand_border: String,
    pub cursor_border: String,
    pub selected_border: String,

    // Text & Element Colors
    pub title_color: String,
    pub subtitle_color: String,
    pub body_color: String,
    pub url_color: String,
    pub debug_badge_bg: String,
    pub debug_badge_text: String,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            background: "#0f1117".to_string(),
            hero_card_bg: "#181b26".to_string(),
            prior_card_bg: "#13151d".to_string(),
            undo_card_bg: "#14161e".to_string(),

            undo_border: "#f59e0b".to_string(),
            left_hand_border: "#6366f1".to_string(),
            left_prior1_border: "#0ea5e9".to_string(),
            left_prior2_border: "#475569".to_string(),
            right_prior2_border: "#475569".to_string(),
            right_prior1_border: "#a855f7".to_string(),
            right_hand_border: "#10b981".to_string(),
            cursor_border: "#ec4899".to_string(),
            selected_border: "#8b5cf6".to_string(),

            title_color: "#f1f5f9".to_string(),
            subtitle_color: "#94a3b8".to_string(),
            body_color: "#cbd5e1".to_string(),
            url_color: "#38bdf8".to_string(),
            debug_badge_bg: "#0f172a".to_string(),
            debug_badge_text: "#38bdf8".to_string(),
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
            if let Ok(config) = Self::load_from_path(local_path) {
                info!("Loaded configuration from local ./config.toml");
                return config;
            }
        }

        if let Some(user_path) = get_user_config_path() {
            if user_path.exists() {
                if let Ok(config) = Self::load_from_path(&user_path) {
                    info!("Loaded configuration from {}", user_path.display());
                    return config;
                }
            } else {
                // Auto-create default config template
                let default_cfg = Self::default();
                if let Some(parent) = user_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(toml_str) = toml::to_string_pretty(&default_cfg) {
                    let _ = std::fs::write(&user_path, toml_str);
                    info!("Created default configuration file at {}", user_path.display());
                }
                return default_cfg;
            }
        }

        warn!("Using in-memory default configuration");
        Self::default()
    }

    pub fn load_from_path(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        Ok(config)
    }
}

pub fn get_user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(xdg).join("little-screen-clip/config.toml"))
    } else if let Ok(home) = std::env::var("HOME") {
        Some(PathBuf::from(home).join(".config/little-screen-clip/config.toml"))
    } else {
        None
    }
}

/// Helper to parse hex string like "#RRGGBB" or "#RGB" into (r, g, b, a).
pub fn parse_hex_color(s: &str) -> (u8, u8, u8, u8) {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(255);
        (r, g, b, 255)
    } else if s.len() == 8 {
        let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(255);
        let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(255);
        let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(255);
        let a = u8::from_str_radix(&s[6..8], 16).unwrap_or(255);
        (r, g, b, a)
    } else if s.len() == 3 {
        let r = u8::from_str_radix(&s[0..1].repeat(2), 16).unwrap_or(255);
        let g = u8::from_str_radix(&s[1..2].repeat(2), 16).unwrap_or(255);
        let b = u8::from_str_radix(&s[2..3].repeat(2), 16).unwrap_or(255);
        (r, g, b, 255)
    } else {
        (255, 255, 255, 255)
    }
}
