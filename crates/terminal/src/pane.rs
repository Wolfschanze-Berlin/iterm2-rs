//! Binary tree layout for split panes within a single tab.
//!
//! Each leaf node owns an `AlacrittyBackend` + `PtyHandle` pair.
//! Internal (split) nodes divide space either horizontally (side-by-side)
//! or vertically (top-bottom) at a configurable ratio.

use crate::AlacrittyBackend;
use crate::PtyHandle;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Direction of a pane split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// Side-by-side (left | right).
    Horizontal,
    /// Top / bottom.
    Vertical,
}

/// A node in the binary pane tree.
pub enum PaneNode {
    /// A terminal pane (leaf).
    Leaf {
        id: usize,
        backend: AlacrittyBackend,
        pty: PtyHandle,
    },
    /// An internal split.
    Split {
        direction: SplitDirection,
        /// Position of the divider as a fraction of the available space (0.0 .. 1.0).
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

/// Screen rectangle for a pane, used for layout / rendering.
#[derive(Debug, Clone, Copy)]
pub struct PaneRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub pane_id: usize,
}

// ---------------------------------------------------------------------------
// PaneNode helpers
// ---------------------------------------------------------------------------

impl PaneNode {
    /// Returns `true` if this node is a leaf.
    pub fn is_leaf(&self) -> bool {
        matches!(self, PaneNode::Leaf { .. })
    }

    /// Find a leaf by `id` (immutable).
    pub fn find_leaf(&self, id: usize) -> Option<&PaneNode> {
        match self {
            PaneNode::Leaf { id: lid, .. } if *lid == id => Some(self),
            PaneNode::Split { first, second, .. } => {
                first.find_leaf(id).or_else(|| second.find_leaf(id))
            }
            _ => None,
        }
    }

    /// Find a leaf by `id` (mutable).
    pub fn find_leaf_mut(&mut self, id: usize) -> Option<&mut PaneNode> {
        match self {
            PaneNode::Leaf { id: lid, .. } if *lid == id => Some(self),
            PaneNode::Split { first, second, .. } => {
                if first.find_leaf(id).is_some() {
                    first.find_leaf_mut(id)
                } else {
                    second.find_leaf_mut(id)
                }
            }
            _ => None,
        }
    }

    /// Collect all leaf IDs via in-order traversal.
    pub fn collect_leaf_ids(&self) -> Vec<usize> {
        let mut ids = Vec::new();
        self.collect_leaf_ids_into(&mut ids);
        ids
    }

    fn collect_leaf_ids_into(&self, out: &mut Vec<usize>) {
        match self {
            PaneNode::Leaf { id, .. } => out.push(*id),
            PaneNode::Split { first, second, .. } => {
                first.collect_leaf_ids_into(out);
                second.collect_leaf_ids_into(out);
            }
        }
    }

    /// Recursively calculate screen rectangles for every leaf.
    fn calculate_rects_inner(&self, x: f32, y: f32, w: f32, h: f32, out: &mut Vec<PaneRect>) {
        match self {
            PaneNode::Leaf { id, .. } => {
                out.push(PaneRect {
                    x,
                    y,
                    width: w,
                    height: h,
                    pane_id: *id,
                });
            }
            PaneNode::Split {
                direction,
                ratio,
                first,
                second,
            } => match direction {
                SplitDirection::Horizontal => {
                    let first_w = w * ratio;
                    let second_w = w - first_w;
                    first.calculate_rects_inner(x, y, first_w, h, out);
                    second.calculate_rects_inner(x + first_w, y, second_w, h, out);
                }
                SplitDirection::Vertical => {
                    let first_h = h * ratio;
                    let second_h = h - first_h;
                    first.calculate_rects_inner(x, y, w, first_h, out);
                    second.calculate_rects_inner(x, y + first_h, w, second_h, out);
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------
// PaneLayout
// ---------------------------------------------------------------------------

/// Manages the binary-tree pane layout for a single tab.
pub struct PaneLayout {
    root: PaneNode,
    active_pane_id: usize,
    next_id: usize,
}

impl PaneLayout {
    /// Create a new layout with a single pane running a shell.
    pub fn new(cols: u16, rows: u16) -> anyhow::Result<Self> {
        let backend = AlacrittyBackend::new(cols, rows);
        let pty = PtyHandle::spawn(cols, rows)?;
        Ok(Self {
            root: PaneNode::Leaf {
                id: 0,
                backend,
                pty,
            },
            active_pane_id: 0,
            next_id: 1,
        })
    }

    /// Split the active pane in the given direction, spawning a new shell.
    ///
    /// The active pane becomes the first child of a new split node; the
    /// freshly spawned pane becomes the second child.  Returns the new
    /// pane's ID.
    pub fn split(
        &mut self,
        direction: SplitDirection,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<usize> {
        let new_id = self.next_id;
        self.next_id += 1;

        let new_backend = AlacrittyBackend::new(cols, rows);
        let new_pty = PtyHandle::spawn(cols, rows)?;

        let target_id = self.active_pane_id;
        let new_leaf = PaneNode::Leaf {
            id: new_id,
            backend: new_backend,
            pty: new_pty,
        };

        split_node_at(&mut self.root, target_id, direction, new_leaf);
        self.active_pane_id = new_id;
        Ok(new_id)
    }

    /// Close the active pane.  Returns `false` if it was the last pane.
    pub fn close_active(&mut self) -> bool {
        if self.root.is_leaf() {
            return false;
        }

        let target = self.active_pane_id;
        let ids = self.root.collect_leaf_ids();

        // Pick next focus.
        let pos = ids.iter().position(|&id| id == target).unwrap_or(0);
        let next_focus = if pos + 1 < ids.len() {
            ids[pos + 1]
        } else if pos > 0 {
            ids[pos - 1]
        } else {
            ids[0]
        };

        remove_leaf_node(&mut self.root, target);
        self.active_pane_id = next_focus;
        true
    }

    /// Get the active leaf (immutable).
    pub fn active_pane(&self) -> Option<&PaneNode> {
        self.root.find_leaf(self.active_pane_id)
    }

    /// Get the active leaf (mutable).
    pub fn active_pane_mut(&mut self) -> Option<&mut PaneNode> {
        self.root.find_leaf_mut(self.active_pane_id)
    }

    /// Cycle focus to the next pane (in-order).
    pub fn focus_next(&mut self) {
        let ids = self.root.collect_leaf_ids();
        if ids.is_empty() {
            return;
        }
        let pos = ids
            .iter()
            .position(|&id| id == self.active_pane_id)
            .unwrap_or(0);
        self.active_pane_id = ids[(pos + 1) % ids.len()];
    }

    /// Cycle focus to the previous pane (in-order).
    pub fn focus_prev(&mut self) {
        let ids = self.root.collect_leaf_ids();
        if ids.is_empty() {
            return;
        }
        let pos = ids
            .iter()
            .position(|&id| id == self.active_pane_id)
            .unwrap_or(0);
        if pos == 0 {
            self.active_pane_id = ids[ids.len() - 1];
        } else {
            self.active_pane_id = ids[pos - 1];
        }
    }

    /// Total number of leaf panes.
    pub fn pane_count(&self) -> usize {
        self.root.collect_leaf_ids().len()
    }

    /// All pane IDs in in-order traversal order.
    pub fn collect_pane_ids(&self) -> Vec<usize> {
        self.root.collect_leaf_ids()
    }

    /// Calculate the screen rectangle for each pane given the total area.
    pub fn calculate_rects(&self, total_width: f32, total_height: f32) -> Vec<PaneRect> {
        let mut rects = Vec::new();
        self.root
            .calculate_rects_inner(0.0, 0.0, total_width, total_height, &mut rects);
        rects
    }

    /// Get a reference to the root node.
    pub fn root(&self) -> &PaneNode {
        &self.root
    }

    /// Get the active pane ID.
    pub fn active_pane_id(&self) -> usize {
        self.active_pane_id
    }
}

// ---------------------------------------------------------------------------
// Tree mutation helpers (free functions to work around borrow-checker issues)
// ---------------------------------------------------------------------------

/// Find the leaf with `target_id` and replace it with a Split whose first
/// child is the original leaf and second child is `new_leaf`.
fn split_node_at(
    node: &mut PaneNode,
    target_id: usize,
    direction: SplitDirection,
    new_leaf: PaneNode,
) -> bool {
    let is_target = matches!(node, PaneNode::Leaf { id, .. } if *id == target_id);

    if is_target {
        // Use ptr::read/write to take ownership of the current node, build
        // the Split, and write it back — no placeholder values needed.
        unsafe {
            let old_leaf = std::ptr::read(node);
            let split = PaneNode::Split {
                direction,
                ratio: 0.5,
                first: Box::new(old_leaf),
                second: Box::new(new_leaf),
            };
            std::ptr::write(node, split);
        }
        return true;
    }

    // Recurse into splits. We check immutably first to know which branch
    // to descend into (so we only move `new_leaf` once).
    if let PaneNode::Split { first, second, .. } = node {
        if first.find_leaf(target_id).is_some() {
            return split_node_at(first, target_id, direction, new_leaf);
        }
        return split_node_at(second, target_id, direction, new_leaf);
    }

    false
}

/// Remove the leaf with `target_id`.  When found as a direct child of a
/// Split, that Split is replaced by the surviving sibling.
fn remove_leaf_node(node: &mut PaneNode, target_id: usize) -> bool {
    let is_split = matches!(node, PaneNode::Split { .. });
    if !is_split {
        return false;
    }

    // Check if a direct child is the target.
    let first_is_target = if let PaneNode::Split { first, .. } = node {
        matches!(first.as_ref(), PaneNode::Leaf { id, .. } if *id == target_id)
    } else {
        false
    };

    if first_is_target {
        // Replace the whole Split with the `second` child.
        take_and_replace_with(node, |old| {
            if let PaneNode::Split { second, .. } = old {
                *second
            } else {
                unreachable!()
            }
        });
        return true;
    }

    let second_is_target = if let PaneNode::Split { second, .. } = node {
        matches!(second.as_ref(), PaneNode::Leaf { id, .. } if *id == target_id)
    } else {
        false
    };

    if second_is_target {
        take_and_replace_with(node, |old| {
            if let PaneNode::Split { first, .. } = old {
                *first
            } else {
                unreachable!()
            }
        });
        return true;
    }

    // Recurse.
    if let PaneNode::Split { first, second, .. } = node {
        if first.find_leaf(target_id).is_some() {
            return remove_leaf_node(first, target_id);
        }
        if second.find_leaf(target_id).is_some() {
            return remove_leaf_node(second, target_id);
        }
    }
    false
}

/// Helper: take the value out of `slot`, pass it to `f` which produces a
/// replacement, and write that replacement back into `slot`.
///
/// This avoids needing a "default" or "zeroed" PaneNode.
fn take_and_replace_with<F>(slot: &mut PaneNode, f: F)
where
    F: FnOnce(PaneNode) -> PaneNode,
{
    // Safety: We're using `ptr::read` + `ptr::write` to move the value out
    // of the mutable reference, transform it, and put a new value back.
    // This is safe as long as `f` does not panic.  (If it does, the slot
    // would be left in an invalid state, but we don't unwind here.)
    unsafe {
        let old = std::ptr::read(slot);
        let new = f(old);
        std::ptr::write(slot, new);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helpers for building test trees without real backends/PTYs ----------

    /// A lightweight tree used only in tests to exercise layout and traversal
    /// logic without spawning real PTYs.
    enum TestNode {
        Leaf(usize),
        Split {
            direction: SplitDirection,
            ratio: f32,
            first: Box<TestNode>,
            second: Box<TestNode>,
        },
    }

    impl TestNode {
        fn collect_leaf_ids(&self) -> Vec<usize> {
            match self {
                TestNode::Leaf(id) => vec![*id],
                TestNode::Split { first, second, .. } => {
                    let mut ids = first.collect_leaf_ids();
                    ids.extend(second.collect_leaf_ids());
                    ids
                }
            }
        }

        fn is_leaf(&self) -> bool {
            matches!(self, TestNode::Leaf(_))
        }

        fn find_leaf(&self, target: usize) -> bool {
            match self {
                TestNode::Leaf(id) => *id == target,
                TestNode::Split { first, second, .. } => {
                    first.find_leaf(target) || second.find_leaf(target)
                }
            }
        }

        fn calculate_rects(&self, x: f32, y: f32, w: f32, h: f32, out: &mut Vec<PaneRect>) {
            match self {
                TestNode::Leaf(id) => {
                    out.push(PaneRect {
                        x,
                        y,
                        width: w,
                        height: h,
                        pane_id: *id,
                    });
                }
                TestNode::Split {
                    direction,
                    ratio,
                    first,
                    second,
                } => match direction {
                    SplitDirection::Horizontal => {
                        let fw = w * ratio;
                        first.calculate_rects(x, y, fw, h, out);
                        second.calculate_rects(x + fw, y, w - fw, h, out);
                    }
                    SplitDirection::Vertical => {
                        let fh = h * ratio;
                        first.calculate_rects(x, y, w, fh, out);
                        second.calculate_rects(x, y + fh, w, h - fh, out);
                    }
                },
            }
        }

        fn split(&mut self, target_id: usize, direction: SplitDirection, new_id: usize) -> bool {
            let is_target = matches!(self, TestNode::Leaf(id) if *id == target_id);
            if is_target {
                let old = std::mem::replace(self, TestNode::Leaf(new_id));
                let new_child = std::mem::replace(
                    self,
                    TestNode::Split {
                        direction,
                        ratio: 0.5,
                        first: Box::new(old),
                        second: Box::new(TestNode::Leaf(new_id)),
                    },
                );
                if let TestNode::Split { second, .. } = self {
                    *second = Box::new(new_child);
                }
                return true;
            }
            if let TestNode::Split { first, second, .. } = self {
                if first.find_leaf(target_id) {
                    return first.split(target_id, direction, new_id);
                }
                return second.split(target_id, direction, new_id);
            }
            false
        }

        fn remove(&mut self, target_id: usize) -> bool {
            if let TestNode::Split { first, second, .. } = self {
                if matches!(first.as_ref(), TestNode::Leaf(id) if *id == target_id) {
                    let old = std::mem::replace(self, TestNode::Leaf(0));
                    if let TestNode::Split { second, .. } = old {
                        *self = *second;
                    }
                    return true;
                }
                if matches!(second.as_ref(), TestNode::Leaf(id) if *id == target_id) {
                    let old = std::mem::replace(self, TestNode::Leaf(0));
                    if let TestNode::Split { first, .. } = old {
                        *self = *first;
                    }
                    return true;
                }
                if first.find_leaf(target_id) {
                    return first.remove(target_id);
                }
                if second.find_leaf(target_id) {
                    return second.remove(target_id);
                }
            }
            false
        }
    }

    /// A test-only pane layout that mirrors `PaneLayout` but uses `TestNode`.
    struct TestLayout {
        root: TestNode,
        active_pane_id: usize,
        next_id: usize,
    }

    impl TestLayout {
        fn new() -> Self {
            Self {
                root: TestNode::Leaf(0),
                active_pane_id: 0,
                next_id: 1,
            }
        }

        fn split(&mut self, direction: SplitDirection) -> usize {
            let new_id = self.next_id;
            self.next_id += 1;
            self.root.split(self.active_pane_id, direction, new_id);
            self.active_pane_id = new_id;
            new_id
        }

        fn close_active(&mut self) -> bool {
            if self.root.is_leaf() {
                return false;
            }
            let target = self.active_pane_id;
            let ids = self.root.collect_leaf_ids();
            let pos = ids.iter().position(|&id| id == target).unwrap_or(0);
            let next_focus = if pos + 1 < ids.len() {
                ids[pos + 1]
            } else if pos > 0 {
                ids[pos - 1]
            } else {
                ids[0]
            };
            self.root.remove(target);
            self.active_pane_id = next_focus;
            true
        }

        fn pane_count(&self) -> usize {
            self.root.collect_leaf_ids().len()
        }

        fn collect_pane_ids(&self) -> Vec<usize> {
            self.root.collect_leaf_ids()
        }

        fn focus_next(&mut self) {
            let ids = self.root.collect_leaf_ids();
            if ids.is_empty() {
                return;
            }
            let pos = ids
                .iter()
                .position(|&id| id == self.active_pane_id)
                .unwrap_or(0);
            self.active_pane_id = ids[(pos + 1) % ids.len()];
        }

        fn focus_prev(&mut self) {
            let ids = self.root.collect_leaf_ids();
            if ids.is_empty() {
                return;
            }
            let pos = ids
                .iter()
                .position(|&id| id == self.active_pane_id)
                .unwrap_or(0);
            if pos == 0 {
                self.active_pane_id = ids[ids.len() - 1];
            } else {
                self.active_pane_id = ids[pos - 1];
            }
        }

        fn calculate_rects(&self, w: f32, h: f32) -> Vec<PaneRect> {
            let mut rects = Vec::new();
            self.root.calculate_rects(0.0, 0.0, w, h, &mut rects);
            rects
        }
    }

    // -- Tests --------------------------------------------------------------

    #[test]
    fn single_pane_count() {
        let layout = TestLayout::new();
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.collect_pane_ids(), vec![0]);
    }

    #[test]
    fn split_creates_two_panes() {
        let mut layout = TestLayout::new();
        let new_id = layout.split(SplitDirection::Horizontal);
        assert_eq!(layout.pane_count(), 2);
        assert_eq!(new_id, 1);
        assert_eq!(layout.collect_pane_ids(), vec![0, 1]);
    }

    #[test]
    fn double_split_creates_three_panes() {
        let mut layout = TestLayout::new();
        layout.split(SplitDirection::Horizontal);
        layout.split(SplitDirection::Vertical);
        assert_eq!(layout.pane_count(), 3);
        assert_eq!(layout.collect_pane_ids(), vec![0, 1, 2]);
    }

    #[test]
    fn close_pane_reduces_count() {
        let mut layout = TestLayout::new();
        layout.split(SplitDirection::Horizontal);
        assert_eq!(layout.pane_count(), 2);
        assert!(layout.close_active());
        assert_eq!(layout.pane_count(), 1);
    }

    #[test]
    fn cannot_close_last_pane() {
        let mut layout = TestLayout::new();
        assert!(!layout.close_active());
        assert_eq!(layout.pane_count(), 1);
    }

    #[test]
    fn focus_cycling() {
        let mut layout = TestLayout::new();
        layout.split(SplitDirection::Horizontal);
        layout.split(SplitDirection::Vertical);
        // IDs: [0, 1, 2], active = 2
        assert_eq!(layout.active_pane_id, 2);

        layout.focus_next();
        assert_eq!(layout.active_pane_id, 0); // wraps

        layout.focus_next();
        assert_eq!(layout.active_pane_id, 1);

        layout.focus_next();
        assert_eq!(layout.active_pane_id, 2);

        layout.focus_prev();
        assert_eq!(layout.active_pane_id, 1);

        layout.focus_prev();
        assert_eq!(layout.active_pane_id, 0);

        layout.focus_prev();
        assert_eq!(layout.active_pane_id, 2); // wraps back
    }

    #[test]
    fn calculate_rects_single_pane() {
        let layout = TestLayout::new();
        let rects = layout.calculate_rects(800.0, 600.0);
        assert_eq!(rects.len(), 1);
        let r = &rects[0];
        assert_eq!(r.pane_id, 0);
        assert!((r.x).abs() < f32::EPSILON);
        assert!((r.y).abs() < f32::EPSILON);
        assert!((r.width - 800.0).abs() < f32::EPSILON);
        assert!((r.height - 600.0).abs() < f32::EPSILON);
    }

    #[test]
    fn calculate_rects_horizontal_split() {
        let mut layout = TestLayout::new();
        layout.split(SplitDirection::Horizontal);
        let rects = layout.calculate_rects(800.0, 600.0);
        assert_eq!(rects.len(), 2);

        let left = rects.iter().find(|r| r.pane_id == 0).unwrap();
        assert!((left.x).abs() < f32::EPSILON);
        assert!((left.width - 400.0).abs() < f32::EPSILON);
        assert!((left.height - 600.0).abs() < f32::EPSILON);

        let right = rects.iter().find(|r| r.pane_id == 1).unwrap();
        assert!((right.x - 400.0).abs() < f32::EPSILON);
        assert!((right.width - 400.0).abs() < f32::EPSILON);
        assert!((right.height - 600.0).abs() < f32::EPSILON);
    }

    #[test]
    fn calculate_rects_vertical_split() {
        let mut layout = TestLayout::new();
        layout.split(SplitDirection::Vertical);
        let rects = layout.calculate_rects(800.0, 600.0);
        assert_eq!(rects.len(), 2);

        let top = rects.iter().find(|r| r.pane_id == 0).unwrap();
        assert!((top.y).abs() < f32::EPSILON);
        assert!((top.height - 300.0).abs() < f32::EPSILON);
        assert!((top.width - 800.0).abs() < f32::EPSILON);

        let bottom = rects.iter().find(|r| r.pane_id == 1).unwrap();
        assert!((bottom.y - 300.0).abs() < f32::EPSILON);
        assert!((bottom.height - 300.0).abs() < f32::EPSILON);
        assert!((bottom.width - 800.0).abs() < f32::EPSILON);
    }

    #[test]
    fn close_active_focuses_sibling() {
        let mut layout = TestLayout::new();
        layout.split(SplitDirection::Horizontal);
        layout.close_active();
        assert_eq!(layout.active_pane_id, 0);
        assert_eq!(layout.pane_count(), 1);
    }

    #[test]
    fn close_middle_pane_in_three() {
        let mut layout = TestLayout::new();
        layout.split(SplitDirection::Horizontal); // [0, 1], active=1
        layout.active_pane_id = 0;
        layout.split(SplitDirection::Vertical); // [0, 2, 1], active=2
        assert_eq!(layout.collect_pane_ids(), vec![0, 2, 1]);

        layout.close_active();
        assert_eq!(layout.pane_count(), 2);
        assert_eq!(layout.active_pane_id, 1);
        assert_eq!(layout.collect_pane_ids(), vec![0, 1]);
    }
}
