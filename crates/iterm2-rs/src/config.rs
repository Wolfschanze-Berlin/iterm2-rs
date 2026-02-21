use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::profiles::Profile;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub terminal: TerminalConfig,
    pub colors: ColorConfig,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
    pub opacity: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub shell: Option<String>,
    pub scrollback: usize,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ColorConfig {
    pub background: String,
    pub foreground: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: FontConfig::default(),
            window: WindowConfig::default(),
            terminal: TerminalConfig::default(),
            colors: ColorConfig::default(),
            profiles: Vec::new(),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Cascadia Code".to_string(),
            size: 14.0,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            opacity: 1.0,
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            scrollback: 10_000,
            cols: 80,
            rows: 24,
        }
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            background: "#1e1e2e".to_string(),
            foreground: "#cdd6f4".to_string(),
        }
    }
}

impl Config {
    /// Returns the platform-specific config file path.
    fn config_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            std::env::var("APPDATA").ok().map(|appdata| {
                PathBuf::from(appdata)
                    .join("iterm2-rs")
                    .join("config.toml")
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| PathBuf::from(h).join(".config"))
                })
                .map(|dir| dir.join("iterm2-rs").join("config.toml"))
        }
    }

    /// Load configuration from the default config file path.
    ///
    /// Returns `Config::default()` if the file does not exist or cannot be parsed.
    pub fn load() -> Config {
        let Some(path) = Self::config_path() else {
            log::debug!("Could not determine config directory; using defaults");
            return Config::default();
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("No config file at {}; using defaults", path.display());
                return Config::default();
            }
            Err(e) => {
                log::warn!("Failed to read config file {}: {}", path.display(), e);
                return Config::default();
            }
        };

        match toml::from_str(&content) {
            Ok(config) => config,
            Err(e) => {
                log::warn!(
                    "Failed to parse config file {}; using defaults: {}",
                    path.display(),
                    e
                );
                Config::default()
            }
        }
    }
}
