//! GPUI requires exactly Taffy 0.9.0; its API is compatible with upstream 0.9.2.
//! Re-export 0.9.2 so the vulnerable grid 0.18 implementation is never linked.

pub use taffy_fixed::*;

#[cfg(all(test, feature = "taffy_tree"))]
mod tests {
    #[test]
    fn computes_layout_with_the_upstream_patch_release() {
        use super::prelude::*;
        let mut tree: TaffyTree<()> = TaffyTree::new();
        let node = tree.new_leaf(Style::default()).unwrap();
        tree.compute_layout(node, Size::MAX_CONTENT).unwrap();
        assert_eq!(tree.layout(node).unwrap().size.width, 0.0);
    }
}
