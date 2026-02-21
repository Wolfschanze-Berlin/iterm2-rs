//! Tab management: each tab owns an AlacrittyBackend + PtyHandle pair.

use crate::AlacrittyBackend;
use crate::PtyHandle;

/// Represents a single terminal tab with its own backend and PTY.
pub struct Tab {
    pub id: usize,
    pub title: String,
    pub backend: AlacrittyBackend,
    pub pty: PtyHandle,
}

/// Manages multiple tabs with one active tab at a time.
pub struct TabManager {
    tabs: Vec<Tab>,
    active_index: usize,
    next_id: usize,
}

impl TabManager {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active_index: 0,
            next_id: 0,
        }
    }

    /// Create a new tab with the given terminal size, spawning a shell.
    /// Returns the id of the newly created tab.
    pub fn new_tab(&mut self, cols: u16, rows: u16) -> anyhow::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;

        let backend = AlacrittyBackend::new(cols, rows);
        let pty = PtyHandle::spawn(cols, rows)?;
        let title = format!("Tab {}", id + 1);

        let tab = Tab {
            id,
            title,
            backend,
            pty,
        };

        self.tabs.push(tab);
        // Switch to the newly created tab.
        self.active_index = self.tabs.len() - 1;

        log::info!("Created tab {id} ({cols}x{rows}), total tabs: {}", self.tabs.len());
        Ok(id)
    }

    /// Close tab at the given index. Returns true if there are still tabs left.
    pub fn close_tab(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() {
            return !self.tabs.is_empty();
        }

        let removed = self.tabs.remove(index);
        log::info!("Closed tab {} (id={}), remaining: {}", index, removed.id, self.tabs.len());

        if self.tabs.is_empty() {
            self.active_index = 0;
            return false;
        }

        // Adjust active index if needed.
        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        } else if self.active_index > index {
            self.active_index -= 1;
        }

        true
    }

    /// Get the active tab.
    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active_index)
    }

    /// Get the active tab mutably.
    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_index)
    }

    /// Switch to the next tab (wraps around).
    pub fn next_tab(&mut self) {
        if self.tabs.len() > 1 {
            self.active_index = (self.active_index + 1) % self.tabs.len();
            log::debug!("Switched to tab index {}", self.active_index);
        }
    }

    /// Switch to the previous tab (wraps around).
    pub fn prev_tab(&mut self) {
        if self.tabs.len() > 1 {
            if self.active_index == 0 {
                self.active_index = self.tabs.len() - 1;
            } else {
                self.active_index -= 1;
            }
            log::debug!("Switched to tab index {}", self.active_index);
        }
    }

    /// Switch to a specific tab by index.
    pub fn switch_to(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_index = index;
            log::debug!("Switched to tab index {}", self.active_index);
        }
    }

    /// Get tab count.
    pub fn count(&self) -> usize {
        self.tabs.len()
    }

    /// Get active tab index.
    pub fn active_index(&self) -> usize {
        self.active_index
    }

    /// Iterate over all tabs mutably (for resize operations).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    /// Get tab titles for display.
    /// Returns (id, title, is_active) for each tab.
    pub fn tab_titles(&self) -> Vec<(usize, &str, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| (tab.id, tab.title.as_str(), i == self.active_index))
            .collect()
    }
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests cannot spawn real PTYs in CI, so we test the
    // non-PTY logic only. Integration tests with real PTYs belong elsewhere.

    #[test]
    fn new_manager_is_empty() {
        let mgr = TabManager::new();
        assert_eq!(mgr.count(), 0);
        assert_eq!(mgr.active_index(), 0);
        assert!(mgr.active().is_none());
    }

    #[test]
    fn default_is_same_as_new() {
        let mgr = TabManager::default();
        assert_eq!(mgr.count(), 0);
    }

    #[test]
    fn tab_titles_empty() {
        let mgr = TabManager::new();
        assert!(mgr.tab_titles().is_empty());
    }

    #[test]
    fn close_tab_on_empty_returns_false() {
        let mut mgr = TabManager::new();
        assert!(!mgr.close_tab(0));
    }

    #[test]
    fn close_tab_out_of_bounds_on_empty() {
        let mut mgr = TabManager::new();
        assert!(!mgr.close_tab(99));
    }

    #[test]
    fn next_prev_on_empty_does_not_panic() {
        let mut mgr = TabManager::new();
        mgr.next_tab();
        mgr.prev_tab();
        mgr.switch_to(0);
        assert_eq!(mgr.active_index(), 0);
    }
}
