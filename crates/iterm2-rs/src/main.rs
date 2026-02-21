mod clipboard;
mod config;
mod profiles;

use anyhow::Result;
use profiles::ProfileManager;
use renderer::{RendererConfig, RgbColor};

fn main() -> Result<()> {
    env_logger::init();
    log::info!("iterm2-rs v{} starting...", env!("CARGO_PKG_VERSION"));

    let config = config::Config::load();
    log::info!("Loaded config: {:?}", config);

    let profile_manager = ProfileManager::load_from_config(&config);
    log::info!("Loaded {} profile(s): {:?}", profile_manager.profiles.len(), profile_manager.list());

    let renderer_config = build_renderer_config(&config);

    // Create event loop, window, initialize GPU, and run the render loop.
    renderer::window::run(renderer_config)?;

    Ok(())
}

/// Convert the application config into a renderer-specific config.
fn build_renderer_config(config: &config::Config) -> RendererConfig {
    RendererConfig {
        font_family: config.font.family.clone(),
        font_size: config.font.size,
        bg_color: RgbColor::from_hex(&config.colors.background)
            .unwrap_or_else(|| RgbColor::from_hex("#1e1e2e").unwrap()),
        fg_color: RgbColor::from_hex(&config.colors.foreground)
            .unwrap_or_else(|| RgbColor::from_hex("#cdd6f4").unwrap()),
        window_title: "iterm2-rs".to_string(),
        window_width: config.window.width,
        window_height: config.window.height,
        opacity: config.window.opacity,
    }
}
