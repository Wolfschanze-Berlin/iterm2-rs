mod clipboard;
mod config;
mod profiles;

use anyhow::Result;
use profiles::ProfileManager;

fn main() -> Result<()> {
    env_logger::init();
    log::info!("iterm2-rs v{} starting...", env!("CARGO_PKG_VERSION"));

    let config = config::Config::load();
    log::info!("Loaded config: {:?}", config);

    let profile_manager = ProfileManager::load_from_config(&config);
    log::info!("Loaded {} profile(s): {:?}", profile_manager.profiles.len(), profile_manager.list());

    // Create event loop, window, initialize GPU, and run the render loop.
    renderer::window::run()?;

    Ok(())
}
