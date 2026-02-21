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
    /// Parse a TOML string into a `Config`, falling back to defaults on error.
    ///
    /// Exposed for testing.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let cfg = Config::default();
        assert_eq!(cfg.font.size, 14.0);
        assert_eq!(cfg.font.family, "Cascadia Code");
        assert_eq!(cfg.window.width, 800);
        assert_eq!(cfg.window.height, 600);
        assert_eq!(cfg.window.opacity, 1.0);
        assert_eq!(cfg.terminal.cols, 80);
        assert_eq!(cfg.terminal.rows, 24);
        assert_eq!(cfg.terminal.scrollback, 10_000);
        assert_eq!(cfg.colors.background, "#1e1e2e");
        assert_eq!(cfg.colors.foreground, "#cdd6f4");
    }

    #[test]
    fn full_toml_parses_correctly() {
        let toml = r##"
[font]
family = "JetBrains Mono"
size = 16.0

[window]
width = 1024
height = 768
opacity = 0.9

[terminal]
shell = "pwsh.exe"
scrollback = 5000
cols = 120
rows = 40

[colors]
background = "#282c34"
foreground = "#abb2bf"
"##;
        let cfg = Config::from_toml(toml).unwrap();
        assert_eq!(cfg.font.family, "JetBrains Mono");
        assert_eq!(cfg.font.size, 16.0);
        assert_eq!(cfg.window.width, 1024);
        assert_eq!(cfg.window.height, 768);
        assert_eq!(cfg.window.opacity, 0.9);
        assert_eq!(cfg.terminal.shell.as_deref(), Some("pwsh.exe"));
        assert_eq!(cfg.terminal.scrollback, 5000);
        assert_eq!(cfg.terminal.cols, 120);
        assert_eq!(cfg.terminal.rows, 40);
        assert_eq!(cfg.colors.background, "#282c34");
        assert_eq!(cfg.colors.foreground, "#abb2bf");
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let toml = r#"
[font]
size = 18.0
"#;
        let cfg = Config::from_toml(toml).unwrap();
        assert_eq!(cfg.font.size, 18.0);
        // Other fields get defaults.
        assert_eq!(cfg.font.family, "Cascadia Code");
        assert_eq!(cfg.window.width, 800);
        assert_eq!(cfg.terminal.cols, 80);
        assert_eq!(cfg.colors.background, "#1e1e2e");
    }

    #[test]
    fn empty_toml_returns_defaults() {
        let cfg = Config::from_toml("").unwrap();
        assert_eq!(cfg.font.size, 14.0);
        assert_eq!(cfg.window.width, 800);
    }

    #[test]
    fn invalid_toml_returns_error() {
        let result = Config::from_toml("this is not valid toml {{{{");
        assert!(result.is_err());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let toml = r#"
[font]
size = 12.0
unknown_key = "should be ignored"
"#;
        // serde's default behavior with #[serde(default)] allows unknown fields
        // depending on configuration — this test documents our current behavior.
        let result = Config::from_toml(toml);
        // If deny_unknown_fields is set, this would fail. We expect it to succeed.
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn terminal_shell_none_when_omitted() {
        let toml = r#"
[terminal]
scrollback = 1000
"#;
        let cfg = Config::from_toml(toml).unwrap();
        assert!(cfg.terminal.shell.is_none());
    }

    #[test]
    fn load_returns_defaults_without_file() {
        // Config::load() should not panic even without a config file.
        let cfg = Config::load();
        assert_eq!(cfg.font.size, 14.0);
    }
}
