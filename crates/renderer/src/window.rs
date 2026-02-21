//! Window creation and event loop setup using winit.

use std::sync::Arc;

use anyhow::Result;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use terminal::TerminalBackend;

use crate::gpu::GpuState;
use crate::terminal_renderer;

/// Application state that holds the window, GPU renderer, and tab manager.
pub struct App {
    window: Option<Arc<Window>>,
    gpu_state: Option<GpuState>,
    tabs: Option<terminal::TabManager>,
    modifiers: ModifiersState,
}

impl App {
    /// Create a new `App` with no window or GPU state yet.
    /// The window is created in the `resumed` callback.
    pub fn new() -> Self {
        Self {
            window: None,
            gpu_state: None,
            tabs: None,
            modifiers: ModifiersState::empty(),
        }
    }

    /// Drain PTY output from the active tab, feed it to the terminal backend,
    /// and update the text renderer with the current grid content.
    fn update_terminal(&mut self) {
        let Some(tabs) = self.tabs.as_mut() else { return };
        let Some(gpu) = self.gpu_state.as_mut() else { return };
        let Some(tab) = tabs.active_mut() else { return };

        // Drain all available PTY output.
        while let Some(bytes) = tab.pty.try_recv() {
            tab.backend.process_bytes(&bytes);
        }

        // Always re-render the grid — the shell may update the current line
        // (e.g. history navigation, tab completion) and we need to show it.
        let lines = terminal_renderer::extract_grid_text(&tab.backend);
        let w = gpu.size.width as f32;
        let h = gpu.size.height as f32;
        gpu.text.set_lines(&lines, w, h);
    }

    /// Force a full re-render of the active tab's content (e.g. after switching tabs).
    fn refresh_active_tab(&mut self) {
        let Some(tabs) = self.tabs.as_ref() else { return };
        let Some(gpu) = self.gpu_state.as_mut() else { return };
        let Some(tab) = tabs.active() else { return };

        let lines = terminal_renderer::extract_grid_text(&tab.backend);
        let w = gpu.size.width as f32;
        let h = gpu.size.height as f32;
        gpu.text.set_lines(&lines, w, h);
    }

    /// Check if a key event is a tab management shortcut.
    /// Returns true if the event was consumed.
    fn handle_tab_shortcut(
        &mut self,
        event: &winit::event::KeyEvent,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        use winit::event::ElementState;

        if event.state != ElementState::Pressed {
            return false;
        }

        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();

        if !ctrl {
            return false;
        }

        match &event.logical_key {
            // Ctrl+Shift+T => new tab
            Key::Character(c) if shift && (c.as_ref() == "T" || c.as_ref() == "t") => {
                if let Some(tabs) = self.tabs.as_mut() {
                    match tabs.new_tab(80, 24) {
                        Ok(id) => {
                            log::info!("New tab created (id={id})");
                            self.refresh_active_tab();
                        }
                        Err(e) => log::error!("Failed to create new tab: {e}"),
                    }
                }
                true
            }
            // Ctrl+Shift+W => close current tab
            Key::Character(c) if shift && (c.as_ref() == "W" || c.as_ref() == "w") => {
                if let Some(tabs) = self.tabs.as_mut() {
                    let index = tabs.active_index();
                    let has_tabs = tabs.close_tab(index);
                    if !has_tabs {
                        log::info!("Last tab closed, exiting");
                        event_loop.exit();
                    } else {
                        self.refresh_active_tab();
                    }
                }
                true
            }
            // Ctrl+Tab => next tab, Ctrl+Shift+Tab => prev tab
            Key::Named(NamedKey::Tab) => {
                if let Some(tabs) = self.tabs.as_mut() {
                    if shift {
                        tabs.prev_tab();
                    } else {
                        tabs.next_tab();
                    }
                }
                self.refresh_active_tab();
                true
            }
            _ => false,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            // Already created; this can happen on some platforms.
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("iterm2-rs")
            .with_inner_size(LogicalSize::new(800.0, 600.0))
            .with_min_inner_size(LogicalSize::new(400.0, 300.0))
            .with_resizable(true);

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let scale_factor = window.scale_factor();
        log::info!(
            "Window created: {:?}, scale_factor={scale_factor}",
            window.inner_size()
        );

        // Initialize wgpu (blocking on async).
        match pollster::block_on(GpuState::new(window.clone())) {
            Ok(gpu) => {
                log::info!("GPU initialized successfully");
                self.gpu_state = Some(gpu);
            }
            Err(e) => {
                log::error!("Failed to initialize GPU: {e}");
                event_loop.exit();
                return;
            }
        }

        // Create tab manager and spawn the first tab.
        let cols: u16 = 80;
        let rows: u16 = 24;
        let mut tabs = terminal::TabManager::new();

        match tabs.new_tab(cols, rows) {
            Ok(_id) => {
                log::info!("First tab created ({cols}x{rows})");
            }
            Err(e) => {
                log::error!("Failed to create first tab: {e}");
                event_loop.exit();
                return;
            }
        }

        // Feed an initial empty update so the renderer has content.
        if let Some(tab) = tabs.active() {
            let lines = terminal_renderer::extract_grid_text(&tab.backend);
            if let Some(gpu) = self.gpu_state.as_mut() {
                let w = gpu.size.width as f32;
                let h = gpu.size.height as f32;
                gpu.text.set_lines(&lines, w, h);
            }
        }

        self.tabs = Some(tabs);
        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                log::info!("Close requested, exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(physical_size) => {
                if let Some(gpu) = self.gpu_state.as_mut() {
                    gpu.resize(physical_size);
                }
                // TODO: Resize terminal/PTY to match new window dimensions.
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // The new inner size is applied via the subsequent Resized event.
                if let Some(window) = self.window.as_ref() {
                    log::debug!("Scale factor changed to {}", window.scale_factor());
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.modifiers = new_modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Check for tab management shortcuts first.
                if self.handle_tab_shortcut(&event, event_loop) {
                    return;
                }

                // Otherwise, forward to the active tab's PTY.
                if let Some(bytes) =
                    terminal::input::translate_key_event(&event, &self.modifiers)
                {
                    if let Some(tabs) = self.tabs.as_mut() {
                        if let Some(tab) = tabs.active_mut() {
                            if let Err(e) = tab.pty.write(&bytes) {
                                log::warn!("Failed to write to PTY: {e}");
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(gpu) = self.gpu_state.as_mut() {
                    match gpu.render() {
                        Ok(()) => {}
                        Err(e) => {
                            // If we get a surface error, try to reconfigure.
                            log::warn!("Render error: {e}");
                            if let Some(window) = self.window.as_ref() {
                                let size = window.inner_size();
                                gpu.resize(size);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Drain PTY output and update terminal content.
        self.update_terminal();

        // Request a redraw every frame so we keep rendering.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

/// Create and return a new winit event loop.
pub fn create_event_loop() -> Result<EventLoop<()>> {
    let event_loop = EventLoop::new()?;
    Ok(event_loop)
}

/// Convenience: create event loop, app, and run. This is the simplest entry point.
pub fn run() -> Result<()> {
    let event_loop = create_event_loop()?;
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}
