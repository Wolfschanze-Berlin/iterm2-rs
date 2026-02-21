use serde::{Deserialize, Serialize};

use crate::config::{ColorConfig, Config, FontConfig, TerminalConfig};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub font: Option<FontConfig>,
    pub colors: Option<ColorConfig>,
    pub terminal: Option<TerminalConfig>,
    pub command: Option<String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            font: None,
            colors: None,
            terminal: None,
            command: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProfileManager {
    pub profiles: Vec<Profile>,
    pub default_profile: String,
}

impl ProfileManager {
    /// Creates a new ProfileManager with a single "Default" profile.
    pub fn new() -> Self {
        Self {
            profiles: vec![Profile::default()],
            default_profile: "Default".to_string(),
        }
    }

    /// Builds a ProfileManager from the main Config.
    ///
    /// The config values become the "Default" profile. Any profiles defined in
    /// `config.profiles` are appended afterwards.
    pub fn load_from_config(config: &Config) -> Self {
        let default_profile = Profile {
            name: "Default".to_string(),
            font: Some(config.font.clone()),
            colors: Some(config.colors.clone()),
            terminal: Some(config.terminal.clone()),
            command: None,
        };

        let mut profiles = vec![default_profile];

        for p in &config.profiles {
            // Skip any user-defined profile also named "Default" to avoid duplicates.
            if p.name == "Default" {
                continue;
            }
            profiles.push(p.clone());
        }

        Self {
            profiles,
            default_profile: "Default".to_string(),
        }
    }

    /// Find a profile by name.
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Get the default profile.
    pub fn get_default(&self) -> &Profile {
        self.get(&self.default_profile)
            .expect("default profile must always exist")
    }

    /// Add a new profile.
    pub fn add(&mut self, profile: Profile) {
        self.profiles.push(profile);
    }

    /// Remove a profile by name. Cannot remove the default profile.
    pub fn remove(&mut self, name: &str) {
        if name == self.default_profile {
            log::warn!("Cannot remove the default profile");
            return;
        }
        self.profiles.retain(|p| p.name != name);
    }

    /// List all profile names.
    pub fn list(&self) -> Vec<&str> {
        self.profiles.iter().map(|p| p.name.as_str()).collect()
    }

    /// Get the effective FontConfig for the given profile, falling back to the
    /// default config values when the profile does not override the field.
    pub fn resolve_font(&self, profile_name: &str, default_config: &Config) -> FontConfig {
        self.get(profile_name)
            .and_then(|p| p.font.clone())
            .unwrap_or_else(|| default_config.font.clone())
    }

    /// Get the effective ColorConfig for the given profile.
    pub fn resolve_colors(&self, profile_name: &str, default_config: &Config) -> ColorConfig {
        self.get(profile_name)
            .and_then(|p| p.colors.clone())
            .unwrap_or_else(|| default_config.colors.clone())
    }

    /// Get the effective TerminalConfig for the given profile.
    pub fn resolve_terminal(
        &self,
        profile_name: &str,
        default_config: &Config,
    ) -> TerminalConfig {
        self.get(profile_name)
            .and_then(|p| p.terminal.clone())
            .unwrap_or_else(|| default_config.terminal.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_manager_has_one_profile() {
        let mgr = ProfileManager::new();
        assert_eq!(mgr.profiles.len(), 1);
        assert_eq!(mgr.profiles[0].name, "Default");
    }

    #[test]
    fn add_and_list_profiles() {
        let mut mgr = ProfileManager::new();
        mgr.add(Profile {
            name: "SSH Prod".to_string(),
            command: Some("ssh prod".to_string()),
            ..Default::default()
        });
        let names = mgr.list();
        assert_eq!(names, vec!["Default", "SSH Prod"]);
    }

    #[test]
    fn remove_profile() {
        let mut mgr = ProfileManager::new();
        mgr.add(Profile {
            name: "Temp".to_string(),
            ..Default::default()
        });
        assert_eq!(mgr.profiles.len(), 2);
        mgr.remove("Temp");
        assert_eq!(mgr.profiles.len(), 1);
    }

    #[test]
    fn cannot_remove_default_profile() {
        let mut mgr = ProfileManager::new();
        mgr.remove("Default");
        assert_eq!(mgr.profiles.len(), 1);
        assert_eq!(mgr.profiles[0].name, "Default");
    }

    #[test]
    fn resolve_font_uses_profile_override() {
        let config = Config::default();
        let mut mgr = ProfileManager::new();
        mgr.add(Profile {
            name: "Big".to_string(),
            font: Some(FontConfig {
                family: "JetBrains Mono".to_string(),
                size: 24.0,
            }),
            ..Default::default()
        });

        let font = mgr.resolve_font("Big", &config);
        assert_eq!(font.family, "JetBrains Mono");
        assert_eq!(font.size, 24.0);
    }

    #[test]
    fn resolve_font_falls_back_to_default_config() {
        let config = Config::default();
        let mgr = ProfileManager::new();

        // The "Default" profile created by new() has no font override.
        let font = mgr.resolve_font("Default", &config);
        assert_eq!(font.family, config.font.family);
        assert_eq!(font.size, config.font.size);
    }

    #[test]
    fn resolve_colors_uses_profile_override() {
        let config = Config::default();
        let mut mgr = ProfileManager::new();
        mgr.add(Profile {
            name: "Red".to_string(),
            colors: Some(ColorConfig {
                background: "#ff0000".to_string(),
                foreground: "#ffffff".to_string(),
            }),
            ..Default::default()
        });

        let colors = mgr.resolve_colors("Red", &config);
        assert_eq!(colors.background, "#ff0000");
    }

    #[test]
    fn resolve_terminal_falls_back_to_default() {
        let config = Config::default();
        let mgr = ProfileManager::new();

        let term = mgr.resolve_terminal("Default", &config);
        assert_eq!(term.cols, config.terminal.cols);
        assert_eq!(term.rows, config.terminal.rows);
    }

    #[test]
    fn resolve_nonexistent_profile_falls_back() {
        let config = Config::default();
        let mgr = ProfileManager::new();

        let font = mgr.resolve_font("NoSuchProfile", &config);
        assert_eq!(font.family, config.font.family);
    }

    #[test]
    fn load_from_config_includes_user_profiles() {
        let mut config = Config::default();
        config.profiles = vec![Profile {
            name: "Custom".to_string(),
            command: Some("zsh".to_string()),
            ..Default::default()
        }];

        let mgr = ProfileManager::load_from_config(&config);
        assert_eq!(mgr.profiles.len(), 2);
        assert!(mgr.get("Default").is_some());
        assert!(mgr.get("Custom").is_some());
        assert_eq!(mgr.get("Custom").unwrap().command.as_deref(), Some("zsh"));
    }

    #[test]
    fn get_default_returns_default_profile() {
        let mgr = ProfileManager::new();
        let def = mgr.get_default();
        assert_eq!(def.name, "Default");
    }
}
