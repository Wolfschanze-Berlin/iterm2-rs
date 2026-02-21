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

use crate::config::RendererConfig;
use crate::gpu::{GpuState, RenderError};
use crate::terminal_renderer;

/// Application state that holds the window, GPU renderer, and tab manager.
pub struct App {
    window: Option<Arc<Window>>,
    gpu_state: Option<GpuState>,
    tabs: Option<terminal::TabManager>,
    modifiers: ModifiersState,
    /// Current terminal grid size in cells (cols, rows).
    term_size: (u16, u16),
    /// Renderer configuration (font, colors, window).
    renderer_config: RendererConfig,
    /// Cached character width in pixels (computed once after GPU init).
    char_width: f32,
}

impl App {
    /// Create a new `App` with the given renderer configuration.
    /// The window is created in the `resumed` callback.
    pub fn new(renderer_config: RendererConfig) -> Self {
        Self {
            window: None,
            gpu_state: None,
            tabs: None,
            modifiers: ModifiersState::empty(),
            term_size: (80, 24),
            renderer_config,
            char_width: 0.0,
        }
    }

    /// Drain PTY output from the active tab, feed it to the terminal backend,
    /// and update the text renderer with the current grid content.
    fn update_terminal(&mut self) {
        let Some(tabs) = self.tabs.as_mut() else { return };
        let Some(gpu) = self.gpu_state.as_mut() else { return };
        let Some(tab) = tabs.active_mut() else { return };

        // Drain all available PTY output and snap scroll to bottom.
        let mut received_output = false;
        while let Some(bytes) = tab.pty.try_recv() {
            tab.backend.process_bytes(&bytes);
            received_output = true;
        }
        if received_output {
            tab.backend.reset_scroll();
        }

        // Extract the full grid content: styled cells, cursor, and bg rects.
        let grid = terminal_renderer::extract_grid(&tab.backend);

        let w = gpu.size.width as f32;
        let h = gpu.size.height as f32;

        // Update text with per-cell colors.
        gpu.text.set_styled_lines(&grid.styled_lines, w, h);

        // Update cursor.
        gpu.set_cursor(grid.cursor);

        // Update background rectangles + cursor quad.
        let line_height = gpu.text.line_height();
        gpu.set_backgrounds(&grid.bg_rects, self.char_width, line_height);
    }

    /// Compute terminal grid size (cols, rows) from pixel dimensions and font metrics.
    fn compute_grid_size(&self, width: u32, height: u32) -> (u16, u16) {
        let Some(gpu) = self.gpu_state.as_ref() else {
            return self.term_size;
        };
        let char_width = if self.char_width > 0.0 {
            self.char_width
        } else {
            gpu.text.font_size() * 0.6
        };
        let line_height = gpu.text.line_height();
        // Subtract a small padding (4px each side)
        let usable_w = (width as f32 - 8.0).max(1.0);
        let usable_h = (height as f32 - 8.0).max(1.0);
        let cols = (usable_w / char_width).floor() as u16;
        let rows = (usable_h / line_height).floor() as u16;
        (cols.max(10), rows.max(2))
    }

    /// Resize all tabs' backends and PTYs to match new dimensions.
    fn resize_all_tabs(&mut self, cols: u16, rows: u16) {
        if cols == self.term_size.0 && rows == self.term_size.1 {
            return;
        }
        self.term_size = (cols, rows);
        log::info!("Terminal resized to {cols}x{rows}");
        if let Some(tabs) = self.tabs.as_mut() {
            for tab in tabs.iter_mut() {
                tab.backend.resize(cols, rows);
                if let Err(e) = tab.pty.resize(cols, rows) {
                    log::warn!("Failed to resize PTY: {e}");
                }
            }
        }
    }

    /// Force a full re-render of the active tab's content (e.g. after switching tabs).
    fn refresh_active_tab(&mut self) {
        let Some(tabs) = self.tabs.as_ref() else { return };
        let Some(gpu) = self.gpu_state.as_mut() else { return };
        let Some(tab) = tabs.active() else { return };

        let grid = terminal_renderer::extract_grid(&tab.backend);
        let w = gpu.size.width as f32;
        let h = gpu.size.height as f32;

        gpu.text.set_styled_lines(&grid.styled_lines, w, h);
        gpu.set_cursor(grid.cursor);

        let line_height = gpu.text.line_height();
        gpu.set_backgrounds(&grid.bg_rects, self.char_width, line_height);
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
                    let (cols, rows) = self.term_size;
                    match tabs.new_tab(cols, rows) {
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
        Self::new(RendererConfig::default())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title(&self.renderer_config.window_title)
            .with_inner_size(LogicalSize::new(
                self.renderer_config.window_width as f64,
                self.renderer_config.window_height as f64,
            ))
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
        match pollster::block_on(GpuState::new(window.clone(), &self.renderer_config)) {
            Ok(mut gpu) => {
                log::info!("GPU initialized successfully");
                // Compute and cache character width.
                self.char_width = gpu.text.char_width();
                log::info!("Measured char_width={:.1}px", self.char_width);
                self.gpu_state = Some(gpu);
            }
            Err(e) => {
                log::error!("Failed to initialize GPU: {e}");
                event_loop.exit();
                return;
            }
        }

        // Compute terminal grid size from actual window dimensions.
        let inner = window.inner_size();
        let (cols, rows) = self.compute_grid_size(inner.width, inner.height);
        self.term_size = (cols, rows);
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

        // Feed an initial render so the renderer has content.
        if let Some(tab) = tabs.active() {
            let grid = terminal_renderer::extract_grid(&tab.backend);
            if let Some(gpu) = self.gpu_state.as_mut() {
                let w = gpu.size.width as f32;
                let h = gpu.size.height as f32;
                gpu.text.set_styled_lines(&grid.styled_lines, w, h);
                gpu.set_cursor(grid.cursor);
                let line_height = gpu.text.line_height();
                gpu.set_backgrounds(&grid.bg_rects, self.char_width, line_height);
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
                let (cols, rows) = self.compute_grid_size(physical_size.width, physical_size.height);
                self.resize_all_tabs(cols, rows);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = self.window.as_ref() {
                    log::debug!("Scale factor changed to {}", window.scale_factor());
                }
            }
            WindowEvent::ModifiersChanged(new_modifiers) => {
                self.modifiers = new_modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if self.handle_tab_shortcut(&event, event_loop) {
                    return;
                }

                // Check for scroll shortcuts (Shift + PageUp/Down/Home/End).
                if event.state == winit::event::ElementState::Pressed
                    && self.modifiers.shift_key()
                {
                    let handled = match &event.logical_key {
                        Key::Named(NamedKey::PageUp) => {
                            if let Some(tabs) = self.tabs.as_mut() {
                                if let Some(tab) = tabs.active_mut() {
                                    let (_cols, rows) = tab.backend.size();
                                    let page_size = (rows as i32).saturating_sub(2).max(1);
                                    tab.backend.scroll(-page_size);
                                }
                            }
                            true
                        }
                        Key::Named(NamedKey::PageDown) => {
                            if let Some(tabs) = self.tabs.as_mut() {
                                if let Some(tab) = tabs.active_mut() {
                                    let (_cols, rows) = tab.backend.size();
                                    let page_size = (rows as i32).saturating_sub(2).max(1);
                                    tab.backend.scroll(page_size);
                                }
                            }
                            true
                        }
                        Key::Named(NamedKey::Home) => {
                            if let Some(tabs) = self.tabs.as_mut() {
                                if let Some(tab) = tabs.active_mut() {
                                    tab.backend.scroll(i32::MIN / 2);
                                }
                            }
                            true
                        }
                        Key::Named(NamedKey::End) => {
                            if let Some(tabs) = self.tabs.as_mut() {
                                if let Some(tab) = tabs.active_mut() {
                                    tab.backend.reset_scroll();
                                }
                            }
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        return;
                    }
                }

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
                        Err(RenderError::Surface(
                            wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated,
                        )) => {
                            log::debug!("Surface lost/outdated, reconfiguring");
                            if let Some(window) = self.window.as_ref() {
                                gpu.resize(window.inner_size());
                            }
                        }
                        Err(RenderError::Surface(wgpu::SurfaceError::Timeout)) => {
                            log::warn!("Surface timeout, skipping frame");
                        }
                        Err(RenderError::Surface(wgpu::SurfaceError::OutOfMemory)) => {
                            log::error!("GPU out of memory, exiting");
                            event_loop.exit();
                        }
                        Err(RenderError::Surface(other)) => {
                            log::error!("Surface error: {other}");
                        }
                        Err(RenderError::Other(e)) => {
                            log::error!("Render error: {e}");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.update_terminal();

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

/// Convenience: create event loop, app, and run with the given config.
pub fn run(renderer_config: RendererConfig) -> Result<()> {
    let event_loop = create_event_loop()?;
    let mut app = App::new(renderer_config);
    event_loop.run_app(&mut app)?;
    Ok(())
}
