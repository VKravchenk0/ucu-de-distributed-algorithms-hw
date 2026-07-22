use crate::node::NODE_INTERNAL;
use crate::page_io::{PageId, PageIo};
use crate::store::ReadGuard;

/// Recursively prints every page reachable from the tree rooted at
/// `view.root()`, in pre-order (a node before its children) so the
/// output reads top-to-bottom the same way the tree would be drawn.
fn walk<IO: PageIo>(view: &ReadGuard<IO>, id: PageId, depth: usize, raw: bool) {
    let node = view.read(id);
    let indent = "  ".repeat(depth);
    println!("{indent}================ page {id} (depth {depth}) ================");
    if raw {
        node.print_raw();
    } else {
        node.print_pretty();
    }
    if node.btype() == NODE_INTERNAL {
        for i in 0..node.nkeys() {
            walk(view, node.get_ptr(i), depth + 1, raw);
        }
    }
}

/// Pretty-prints the whole tree: page boundaries and, within each page,
/// the boundaries of the header/pointers/offsets/entries/unused sections.
pub fn print_tree<IO: PageIo>(view: &ReadGuard<IO>) {
    println!("========== TREE (root={}) ==========", view.root());
    walk(view, view.root(), 0, false);
}

/// Raw byte dump of every page reachable from the current root.
pub fn print_bytes<IO: PageIo>(view: &ReadGuard<IO>) {
    println!("========== TREE BYTES (root={}) ==========", view.root());
    walk(view, view.root(), 0, true);
}
