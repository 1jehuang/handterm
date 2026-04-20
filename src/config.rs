use crate::color::HexColor;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct AppConfig {
    pub style: StyleConfig,
    pub window: WindowConfig,
    pub performance: PerformanceConfig,
    pub scrollback: ScrollbackConfig,
}

impl AppConfig {
    pub fn load(config_override: Option<&Path>) -> Result<Self> {
        let path = resolve_config_path(config_override)?;

        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed reading config file at {}", path.display()))?;
        let parsed: Self = toml::from_str(&raw)
            .with_context(|| format!("failed parsing config file at {}", path.display()))?;
        Ok(parsed)
    }

    pub fn save_default_if_missing(config_override: Option<&Path>) -> Result<PathBuf> {
        let path = resolve_config_path(config_override)?;
        if path.exists() {
            return Ok(path);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed creating {}", parent.display()))?;
        }

        let content = toml::to_string_pretty(&Self::default())?;
        fs::write(&path, content).with_context(|| format!("failed writing {}", path.display()))?;

        Ok(path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StyleConfig {
    pub background: HexColor,
    pub foreground: HexColor,
    pub cursor: HexColor,
    pub background_opacity: f64,
    pub background_blur: u16,
    pub font_family: String,
    pub font_size: f64,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            // Mirrors current kitty setup.
            background: HexColor::new(0x00, 0x00, 0x00),
            foreground: HexColor::new(0xcd, 0xd6, 0xf4),
            cursor: HexColor::new(0xf5, 0xe0, 0xdc),
            background_opacity: 0.9,
            background_blur: 20,
            font_family: "JetBrainsMono Nerd Font Light".to_string(),
            font_size: 11.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowConfig {
    pub columns: u16,
    pub rows: u16,
    pub remember_window_size: bool,
    pub confirm_os_window_close: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 24,
            remember_window_size: false,
            confirm_os_window_close: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PerformanceConfig {
    pub repaint_delay_ms: u8,
    pub input_delay_ms: u8,
    pub sync_to_monitor: bool,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            repaint_delay_ms: 5,
            input_delay_ms: 0,
            sync_to_monitor: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScrollbackConfig {
    pub lines: usize,
    pub smooth: bool,
    pub smooth_speed: f32,
    pub scrollbar: bool,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self {
            lines: 10_000,
            smooth: true,
            smooth_speed: 3.0,
            scrollbar: true,
        }
    }
}

pub fn resolve_config_path(config_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = config_override {
        return Ok(path.to_path_buf());
    }

    let mut dir = dirs::config_dir().context("failed to resolve system config dir")?;
    dir.push("handterm");
    dir.push(DEFAULT_CONFIG_FILE);
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::Builder;

    fn repo_local_tempdir() -> tempfile::TempDir {
        let base = std::env::current_dir()
            .expect("current dir should resolve")
            .join(".jcode-tmp");
        fs::create_dir_all(&base).expect("repo-local temp root should be creatable");
        Builder::new()
            .prefix("config-test-")
            .tempdir_in(base)
            .expect("repo-local temp dir should be created")
    }

    #[test]
    fn defaults_match_current_kitty_style() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.style.background.to_string(), "#000000");
        assert_eq!(cfg.style.foreground.to_string(), "#cdd6f4");
        assert_eq!(cfg.style.cursor.to_string(), "#f5e0dc");
        assert_eq!(cfg.style.font_family, "JetBrainsMono Nerd Font Light");
        assert_eq!(cfg.style.font_size, 11.0);
        assert_eq!(cfg.style.background_opacity, 0.9);
        assert_eq!(cfg.style.background_blur, 20);
        assert_eq!(cfg.window.columns, 80);
        assert_eq!(cfg.window.rows, 24);
        assert_eq!(cfg.scrollback.lines, 10_000);
        assert!(cfg.scrollback.smooth);
        assert_eq!(cfg.scrollback.smooth_speed, 3.0);
        assert!(cfg.scrollback.scrollbar);
    }

    #[test]
    fn parses_partial_config_and_keeps_defaults() {
        let raw = r##"
            [style]
            background = "#111111"

            [window]
            columns = 100
        "##;

        let cfg: AppConfig = toml::from_str(raw).expect("config should parse");
        assert_eq!(cfg.style.background.to_string(), "#111111");
        assert_eq!(cfg.style.foreground.to_string(), "#cdd6f4");
        assert_eq!(cfg.window.columns, 100);
        assert_eq!(cfg.window.rows, 24);
        assert!(cfg.scrollback.smooth);
        assert_eq!(cfg.scrollback.smooth_speed, 3.0);
        assert!(cfg.scrollback.scrollbar);
    }

    #[test]
    fn parses_smooth_scrollback_flag() {
        let raw = r##"
            [scrollback]
            smooth = true
        "##;

        let cfg: AppConfig = toml::from_str(raw).expect("config should parse");
        assert!(cfg.scrollback.smooth);
        assert_eq!(cfg.scrollback.lines, 10_000);
        assert_eq!(cfg.scrollback.smooth_speed, 3.0);
        assert!(cfg.scrollback.scrollbar);
    }

    #[test]
    fn parses_smooth_scrollback_speed() {
        let raw = r##"
            [scrollback]
            smooth = true
            smooth_speed = 2.5
        "##;

        let cfg: AppConfig = toml::from_str(raw).expect("config should parse");
        assert!(cfg.scrollback.smooth);
        assert_eq!(cfg.scrollback.smooth_speed, 2.5);
    }

    #[test]
    fn parses_scrollbar_toggle() {
        let raw = r##"
            [scrollback]
            scrollbar = false
        "##;

        let cfg: AppConfig = toml::from_str(raw).expect("config should parse");
        assert!(!cfg.scrollback.scrollbar);
    }

    #[test]
    fn writes_default_config() {
        let temp = repo_local_tempdir();
        let path = temp.path().join("config.toml");
        let written =
            AppConfig::save_default_if_missing(Some(&path)).expect("default config should write");
        assert_eq!(written, path);
        let content = fs::read_to_string(&path).expect("config should be readable");
        assert!(content.contains("JetBrainsMono Nerd Font Light"));
        assert!(content.contains("background = \"#000000\""));
    }
}
