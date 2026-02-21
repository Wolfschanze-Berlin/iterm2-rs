//! Native OS menu bar using the `muda` crate.
//!
//! Builds the application menu (File, Edit, View) and provides menu item IDs
//! for the event loop to match against.

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, MenuId, PredefinedMenuItem, Submenu};

/// Identifies which menu action was triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    NewTab,
    CloseTab,
    Exit,
    Copy,
    Paste,
    NextTab,
    PrevTab,
}

/// Holds the menu bar and item IDs so the event loop can resolve events.
pub struct AppMenu {
    pub menu_bar: Menu,
    new_tab_id: MenuId,
    close_tab_id: MenuId,
    exit_id: MenuId,
    copy_id: MenuId,
    paste_id: MenuId,
    next_tab_id: MenuId,
    prev_tab_id: MenuId,
}

impl AppMenu {
    /// Build the full application menu bar.
    pub fn new() -> Self {
        let menu_bar = Menu::new();

        // --- File menu ---
        let new_tab = MenuItem::new(
            "New Tab",
            true,
            Some(Accelerator::new(
                Some(Modifiers::CONTROL | Modifiers::SHIFT),
                Code::KeyT,
            )),
        );
        let close_tab = MenuItem::new(
            "Close Tab",
            true,
            Some(Accelerator::new(
                Some(Modifiers::CONTROL | Modifiers::SHIFT),
                Code::KeyW,
            )),
        );
        let exit = MenuItem::new("Exit", true, Some(Accelerator::new(Some(Modifiers::ALT), Code::F4)));

        let file_menu = Submenu::with_items(
            "&File",
            true,
            &[
                &new_tab,
                &close_tab,
                &PredefinedMenuItem::separator(),
                &exit,
            ],
        )
        .expect("failed to create File menu");

        // --- Edit menu ---
        let copy = MenuItem::new(
            "Copy",
            true,
            Some(Accelerator::new(
                Some(Modifiers::CONTROL | Modifiers::SHIFT),
                Code::KeyC,
            )),
        );
        let paste = MenuItem::new(
            "Paste",
            true,
            Some(Accelerator::new(
                Some(Modifiers::CONTROL | Modifiers::SHIFT),
                Code::KeyV,
            )),
        );

        let edit_menu =
            Submenu::with_items("&Edit", true, &[&copy, &paste]).expect("failed to create Edit menu");

        // --- View menu ---
        let next_tab = MenuItem::new(
            "Next Tab",
            true,
            Some(Accelerator::new(Some(Modifiers::CONTROL), Code::Tab)),
        );
        let prev_tab = MenuItem::new(
            "Previous Tab",
            true,
            Some(Accelerator::new(
                Some(Modifiers::CONTROL | Modifiers::SHIFT),
                Code::Tab,
            )),
        );

        let view_menu = Submenu::with_items("&View", true, &[&next_tab, &prev_tab])
            .expect("failed to create View menu");

        menu_bar
            .append_items(&[&file_menu, &edit_menu, &view_menu])
            .expect("failed to append menus to menu bar");

        Self {
            menu_bar,
            new_tab_id: new_tab.id().clone(),
            close_tab_id: close_tab.id().clone(),
            exit_id: exit.id().clone(),
            copy_id: copy.id().clone(),
            paste_id: paste.id().clone(),
            next_tab_id: next_tab.id().clone(),
            prev_tab_id: prev_tab.id().clone(),
        }
    }

    /// Resolve a `MenuEvent` to a known `MenuAction`, or `None` for unknown items.
    pub fn resolve(&self, event: &MenuEvent) -> Option<MenuAction> {
        let id = event.id();
        if *id == self.new_tab_id {
            Some(MenuAction::NewTab)
        } else if *id == self.close_tab_id {
            Some(MenuAction::CloseTab)
        } else if *id == self.exit_id {
            Some(MenuAction::Exit)
        } else if *id == self.copy_id {
            Some(MenuAction::Copy)
        } else if *id == self.paste_id {
            Some(MenuAction::Paste)
        } else if *id == self.next_tab_id {
            Some(MenuAction::NextTab)
        } else if *id == self.prev_tab_id {
            Some(MenuAction::PrevTab)
        } else {
            None
        }
    }
}

/// Install the `MenuEvent` handler that forwards menu events into the winit
/// event loop via the given proxy.
pub fn install_menu_event_handler(proxy: winit::event_loop::EventLoopProxy<AppEvent>) {
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(AppEvent::MenuEvent(event));
    }));
}

/// Attach the menu bar to a Windows HWND. No-op on other platforms.
#[cfg(target_os = "windows")]
pub fn init_for_window(menu: &Menu, window: &winit::window::Window) {
    use winit::raw_window_handle::*;
    if let RawWindowHandle::Win32(handle) = window.window_handle().unwrap().as_raw() {
        unsafe {
            menu.init_for_hwnd(handle.hwnd.get()).expect("failed to init menu for HWND");
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn init_for_window(_menu: &Menu, _window: &winit::window::Window) {
    // Linux/macOS support can be added later.
}

/// Custom event type for the winit event loop.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A menu item was activated.
    MenuEvent(MenuEvent),
    /// PTY data is available (replaces the old `()` user event).
    PtyWake,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_builds_without_panic() {
        let menu = AppMenu::new();
        // Verify menu bar was created and has items.
        assert!(
            !menu.menu_bar.items().is_empty(),
            "menu bar should have at least one submenu"
        );
    }

    #[test]
    fn menu_has_three_submenus() {
        let menu = AppMenu::new();
        assert_eq!(menu.menu_bar.items().len(), 3, "expected File, Edit, View");
    }

    #[test]
    fn resolve_returns_none_for_unknown_id() {
        let menu = AppMenu::new();
        let fake_event = MenuEvent { id: MenuId::new("nonexistent") };
        assert!(menu.resolve(&fake_event).is_none());
    }
}
