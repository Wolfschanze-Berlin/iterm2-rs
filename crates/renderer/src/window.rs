//! Window creation and event loop setup using winit.

use std::sync::Arc;

use anyhow::Result;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use terminal::TerminalBackend;

use crate::config::RendererConfig;
use crate::gpu::{GpuState, RenderError};
use crate::menu::{self, AppEvent, AppMenu, MenuAction};
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
    /// Dirty flag: when true, the next `about_to_wait` will re-extract the grid
    /// and request a GPU redraw. Set by PTY output, resize, scroll, and tab switches.
    needs_redraw: bool,
    /// Event loop proxy used by PTY reader threads to wake the event loop.
    event_loop_proxy: EventLoopProxy<AppEvent>,
    /// Native OS menu bar.
    app_menu: AppMenu,
    /// Clipboard manager for Copy/Paste.
    clipboard: Option<arboard::Clipboard>,
    /// Last known cursor position in physical pixels (for mouse click handling).
    cursor_position: (f64, f64),
}

impl App {
    /// Create a new `App` with the given renderer configuration, event loop proxy,
    /// and pre-built application menu.
    pub fn new(
        renderer_config: RendererConfig,
        proxy: EventLoopProxy<AppEvent>,
        app_menu: AppMenu,
    ) -> Self {
        let clipboard = match arboard::Clipboard::new() {
            Ok(cb) => Some(cb),
            Err(e) => {
                log::warn!("Failed to initialize clipboard: {e}");
                None
            }
        };
        Self {
            window: None,
            gpu_state: None,
            tabs: None,
            modifiers: ModifiersState::empty(),
            term_size: (renderer_config.default_cols, renderer_config.default_rows),
            renderer_config,
            char_width: 0.0,
            needs_redraw: true,
            event_loop_proxy: proxy,
            app_menu,
            clipboard,
            cursor_position: (0.0, 0.0),
        }
    }

    /// Drain PTY output from all panes in the active tab and mark the app dirty
    /// if new data arrived. The expensive grid extraction and GPU upload only
    /// happen when `needs_redraw` is set.
    fn poll_pty(&mut self) {
        let Some(tabs) = self.tabs.as_mut() else { return };
        let Some(tab) = tabs.active_mut() else { return };

        let mut got_data = false;
        tab.for_each_pane_mut(|_id, backend, pty| {
            while let Some(bytes) = pty.try_recv() {
                backend.process_bytes(&bytes);
                got_data = true;
            }
        });
        if got_data {
            self.needs_redraw = true;
        }
        if self.needs_redraw {
            if let Some(backend) = tab.active_backend_mut() {
                backend.reset_scroll();
            }
        }
    }

    /// Re-extract the terminal grid and upload it to the GPU.
    /// Only called when `needs_redraw` is true.
    fn sync_grid_to_gpu(&mut self) {
        let Some(tabs) = self.tabs.as_ref() else { return };
        let Some(gpu) = self.gpu_state.as_mut() else { return };
        let Some(tab) = tabs.active() else { return };

        // Update tab bar (hidden when only one tab).
        let tab_titles = tabs.tab_titles();
        gpu.set_tab_bar(&tab_titles);

        let Some(active_backend) = tab.active_backend() else { return };
        let grid = terminal_renderer::extract_grid(active_backend);

        let w = gpu.size.width as f32;
        let h = gpu.size.height as f32;
        let tab_bar_h = gpu.tab_bar_height();

        gpu.text.set_styled_lines(&grid.styled_lines, w, h - tab_bar_h);
        gpu.set_cursor(grid.cursor);

        let line_height = gpu.text.line_height();
        gpu.set_backgrounds(&grid.bg_rects, self.char_width, line_height, tab_bar_h);
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
        // Subtract tab bar height from available vertical space.
        let effective_height = (height as f32 - gpu.tab_bar_height()).max(0.0) as u32;
        compute_grid_size_from_metrics(width, effective_height, char_width, line_height)
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
                tab.for_each_pane_mut(|_id, backend, pty| {
                    backend.resize(cols, rows);
                    if let Err(e) = pty.resize(cols, rows) {
                        log::warn!("Failed to resize PTY: {e}");
                    }
                });
            }
        }
    }

    /// Mark the display as needing a full re-render (e.g. after switching tabs).
    fn refresh_active_tab(&mut self) {
        self.needs_redraw = true;
    }

    /// Spawn a new tab: creates a fresh PTY + shell at the OS level (not inside
    /// the current shell), installs a wake callback, and switches to it.
    fn new_tab(&mut self) {
        let Some(tabs) = self.tabs.as_mut() else { return };
        let (cols, rows) = self.term_size;
        match tabs.new_tab(cols, rows) {
            Ok(id) => {
                log::info!("New tab created (id={id})");
                // Install wake callback on the new tab's PTY.
                if let Some(tab) = tabs.active_mut() {
                    if let Some(pty) = tab.active_pty_mut() {
                        let proxy = self.event_loop_proxy.clone();
                        let _ = pty.set_wake_callback(Box::new(move || {
                            let _ = proxy.send_event(AppEvent::PtyWake);
                        }));
                    }
                }
                self.refresh_active_tab();
            }
            Err(e) => log::error!("Failed to create new tab: {e}"),
        }
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
            // Ctrl+Shift+T => new tab (spawns a fresh OS-level shell, like Chrome's new tab)
            Key::Character(c) if shift && (c.as_ref() == "T" || c.as_ref() == "t") => {
                self.new_tab();
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

    /// Handle an action triggered by a native menu item.
    fn handle_menu_action(&mut self, action: MenuAction, event_loop: &ActiveEventLoop) {
        match action {
            MenuAction::NewTab => self.new_tab(),
            MenuAction::CloseTab => {
                if let Some(tabs) = self.tabs.as_mut() {
                    let index = tabs.active_index();
                    let has_tabs = tabs.close_tab(index);
                    if !has_tabs {
                        log::info!("Last tab closed via menu, exiting");
                        event_loop.exit();
                    } else {
                        self.refresh_active_tab();
                    }
                }
            }
            MenuAction::Exit => {
                log::info!("Exit requested via menu");
                event_loop.exit();
            }
            MenuAction::Copy => {
                // TODO: Copy selected text once mouse selection is implemented.
                // Currently selection support is not yet in AlacrittyBackend.
                log::debug!("Copy requested — selection not yet implemented");
            }
            MenuAction::Paste => {
                // Paste clipboard text into the active PTY.
                let text = self
                    .clipboard
                    .as_mut()
                    .and_then(|cb| cb.get_text().ok());
                if let Some(text) = text {
                    if let Some(tabs) = self.tabs.as_mut() {
                        if let Some(tab) = tabs.active_mut() {
                            if let Some(pty) = tab.active_pty_mut() {
                                if let Err(e) = pty.write(text.as_bytes()) {
                                    log::warn!("Failed to paste to PTY: {e}");
                                }
                            }
                        }
                    }
                }
            }
            MenuAction::NextTab => {
                if let Some(tabs) = self.tabs.as_mut() {
                    tabs.next_tab();
                }
                self.refresh_active_tab();
            }
            MenuAction::PrevTab => {
                if let Some(tabs) = self.tabs.as_mut() {
                    tabs.prev_tab();
                }
                self.refresh_active_tab();
            }
        }
    }
}

// Note: App no longer implements Default because it requires an EventLoopProxy.

impl ApplicationHandler<AppEvent> for App {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyWake => {
                // PTY reader thread sent data — wake will be handled in about_to_wait.
            }
            AppEvent::MenuEvent(menu_event) => {
                if let Some(action) = self.app_menu.resolve(&menu_event) {
                    self.handle_menu_action(action, event_loop);
                }
            }
        }
    }

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
            .with_resizable(true)
            .with_transparent(self.renderer_config.opacity < 1.0);

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        // Attach native menu bar to the window.
        menu::init_for_window(&self.app_menu.menu_bar, &window);

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
                // Install wake callback so PTY output wakes the event loop.
                if let Some(tab) = tabs.active_mut() {
                    if let Some(pty) = tab.active_pty_mut() {
                        let proxy = self.event_loop_proxy.clone();
                        let _ = pty.set_wake_callback(Box::new(move || {
                            let _ = proxy.send_event(AppEvent::PtyWake);
                        }));
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to create first tab: {e}");
                event_loop.exit();
                return;
            }
        }

        // Mark dirty so the first about_to_wait renders initial content.
        self.needs_redraw = true;

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
                self.needs_redraw = true;
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
                                    if let Some(backend) = tab.active_backend_mut() {
                                        let (_cols, rows) = backend.size();
                                        let page_size = (rows as i32).saturating_sub(2).max(1);
                                        backend.scroll(-page_size);
                                    }
                                }
                            }
                            true
                        }
                        Key::Named(NamedKey::PageDown) => {
                            if let Some(tabs) = self.tabs.as_mut() {
                                if let Some(tab) = tabs.active_mut() {
                                    if let Some(backend) = tab.active_backend_mut() {
                                        let (_cols, rows) = backend.size();
                                        let page_size = (rows as i32).saturating_sub(2).max(1);
                                        backend.scroll(page_size);
                                    }
                                }
                            }
                            true
                        }
                        Key::Named(NamedKey::Home) => {
                            if let Some(tabs) = self.tabs.as_mut() {
                                if let Some(tab) = tabs.active_mut() {
                                    if let Some(backend) = tab.active_backend_mut() {
                                        backend.scroll(i32::MIN / 2);
                                    }
                                }
                            }
                            true
                        }
                        Key::Named(NamedKey::End) => {
                            if let Some(tabs) = self.tabs.as_mut() {
                                if let Some(tab) = tabs.active_mut() {
                                    if let Some(backend) = tab.active_backend_mut() {
                                        backend.reset_scroll();
                                    }
                                }
                            }
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        self.needs_redraw = true;
                        return;
                    }
                }

                if let Some(bytes) =
                    terminal::input::translate_key_event(&event, &self.modifiers)
                {
                    if let Some(tabs) = self.tabs.as_mut() {
                        if let Some(tab) = tabs.active_mut() {
                            if let Some(pty) = tab.active_pty_mut() {
                                if let Err(e) = pty.write(&bytes) {
                                    log::warn!("Failed to write to PTY: {e}");
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                let (mx, my) = self.cursor_position;
                let mx = mx as f32;
                let my = my as f32;

                // Check tab bar click first.
                if let Some(gpu) = self.gpu_state.as_ref() {
                    if let Some(tab_index) = gpu.tab_bar_hit_test(mx, my) {
                        if let Some(tabs) = self.tabs.as_mut() {
                            tabs.switch_to(tab_index);
                            self.refresh_active_tab();
                            log::debug!("Tab bar click → switched to tab index {tab_index}");
                        }
                        return;
                    }
                }

                // Check pane click (for multi-pane focus switching).
                if let Some(gpu) = self.gpu_state.as_ref() {
                    let tab_bar_h = gpu.tab_bar_height();
                    if let Some(tabs) = self.tabs.as_mut() {
                        if let Some(tab) = tabs.active_mut() {
                            let inner = self.window.as_ref().map(|w| w.inner_size());
                            if let Some(size) = inner {
                                let pane_area_h = size.height as f32 - tab_bar_h;
                                let rects = tab.pane_rects(size.width as f32, pane_area_h);
                                // Adjust click y for tab bar offset.
                                let pane_y = my - tab_bar_h;
                                for rect in &rects {
                                    if mx >= rect.x
                                        && mx < rect.x + rect.width
                                        && pane_y >= rect.y
                                        && pane_y < rect.y + rect.height
                                    {
                                        if tab.focus_pane(rect.pane_id) {
                                            self.needs_redraw = true;
                                            log::debug!(
                                                "Pane click → focused pane {}",
                                                rect.pane_id
                                            );
                                        }
                                        break;
                                    }
                                }
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Drain PTY output; this sets `needs_redraw` if new bytes arrived.
        self.poll_pty();

        if self.needs_redraw {
            self.sync_grid_to_gpu();
            self.needs_redraw = false;

            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }

        // Sleep until the next event (PTY wake, key press, resize, etc.).
        // The PTY reader thread wakes us via EventLoopProxy::send_event.
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

/// Convenience: create event loop with native menus, app, and run with the given config.
pub fn run(renderer_config: RendererConfig) -> Result<()> {
    let app_menu = AppMenu::new();

    let mut event_loop_builder = EventLoop::<AppEvent>::with_user_event();

    // On Windows, install a message hook so that menu accelerator keys
    // (e.g. Ctrl+Shift+T shown in the File menu) are translated by Win32.
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        let menu_bar = app_menu.menu_bar.clone();
        event_loop_builder.with_msg_hook(move |msg| {
            use windows_sys::Win32::UI::WindowsAndMessaging::{TranslateAcceleratorW, MSG};
            unsafe {
                let msg = msg as *const MSG;
                let translated = TranslateAcceleratorW((*msg).hwnd, menu_bar.haccel() as _, msg);
                translated == 1
            }
        });
    }

    let event_loop = event_loop_builder.build()?;
    let proxy = event_loop.create_proxy();

    // Route native menu events into the winit event loop.
    menu::install_menu_event_handler(proxy.clone());

    let mut app = App::new(renderer_config, proxy, app_menu);
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Compute terminal grid size (cols, rows) from pixel dimensions and font metrics.
///
/// This is a pure function extracted from `App::compute_grid_size` to enable unit testing
/// without requiring GPU state. Subtracts 4px padding on each side, then divides by
/// character/line metrics. Enforces minimums of 10 columns and 2 rows.
pub fn compute_grid_size_from_metrics(
    width: u32,
    height: u32,
    char_width: f32,
    line_height: f32,
) -> (u16, u16) {
    let usable_w = (width as f32 - 8.0).max(1.0);
    let usable_h = (height as f32 - 8.0).max(1.0);
    let cols = (usable_w / char_width).floor() as u16;
    let rows = (usable_h / line_height).floor() as u16;
    (cols.max(10), rows.max(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAR_W: f32 = 8.4;
    const LINE_H: f32 = 19.6;

    #[test]
    fn standard_window_size() {
        let (cols, rows) = compute_grid_size_from_metrics(800, 600, CHAR_W, LINE_H);
        // (800 - 8) / 8.4 = 94.28 → 94, (600 - 8) / 19.6 = 30.2 → 30
        assert_eq!(cols, 94);
        assert_eq!(rows, 30);
    }

    #[test]
    fn very_small_window_hits_minimums() {
        let (cols, rows) = compute_grid_size_from_metrics(50, 30, CHAR_W, LINE_H);
        assert!(cols >= 10, "cols={cols} should be at least 10");
        assert!(rows >= 2, "rows={rows} should be at least 2");
    }

    #[test]
    fn zero_dimensions_do_not_panic() {
        let (cols, rows) = compute_grid_size_from_metrics(0, 0, CHAR_W, LINE_H);
        assert!(cols >= 10);
        assert!(rows >= 2);
    }

    #[test]
    fn one_pixel_dimensions_do_not_panic() {
        let (cols, rows) = compute_grid_size_from_metrics(1, 1, CHAR_W, LINE_H);
        assert!(cols >= 10);
        assert!(rows >= 2);
    }

    #[test]
    fn large_window_scales_correctly() {
        let (cols, rows) = compute_grid_size_from_metrics(3840, 2160, CHAR_W, LINE_H);
        // (3840 - 8) / 8.4 = 456.19 → 456
        assert!(cols > 400, "cols={cols} expected > 400 for 4K");
        assert!(rows > 100, "rows={rows} expected > 100 for 4K");
    }

    #[test]
    fn minimums_enforced_at_boundary() {
        // Width gives exactly 10 cols: 10 * 8.4 + 8 = 92
        let (cols, _) = compute_grid_size_from_metrics(92, 600, CHAR_W, LINE_H);
        assert_eq!(cols, 10);

        // Width gives 9 cols: 9 * 8.4 + 8 = 83.6 → would be 9, clamped to 10
        let (cols, _) = compute_grid_size_from_metrics(83, 600, CHAR_W, LINE_H);
        assert_eq!(cols, 10);
    }
}
