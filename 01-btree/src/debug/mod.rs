use crate::io::{PageId, PageIo};
use crate::page::Page;
use crate::storage::store::ReadGuard;

/// Recursively prints every page reachable from the tree rooted at
/// `view.root()`, in pre-order (a node before its children) so the
/// output reads top-to-bottom the same way the tree would be drawn.
fn walk<IO: PageIo>(view: &ReadGuard<IO>, id: PageId, depth: usize, raw: bool) {
    let page = view.read(id);
    let indent = "  ".repeat(depth);
    println!("{indent}================ page {id} (depth {depth}) ================");
    if raw {
        print_raw(&page.to_bytes(id, view.page_size()));
    } else {
        match &page {
            Page::Leaf(l) => l.print_pretty(view.page_size()),
            Page::Internal(i) => i.print_pretty(view.page_size()),
        }
    }
    if let Page::Internal(i) = &page {
        // N+1 children per N separators.
        for idx in 0..=i.nkeys() {
            walk(view, i.get_child(idx), depth + 1, raw);
        }
    }
}

/// Pretty-prints the whole tree: each page's parsed leaf/internal
/// layout, page by page.
pub fn print_tree<IO: PageIo>(view: &ReadGuard<IO>) {
    println!("========== TREE (root={}) ==========", view.root());
    walk(view, view.root(), 0, false);
    println!();
}

/// Raw byte dump of every page reachable from the current root.
pub fn print_bytes<IO: PageIo>(view: &ReadGuard<IO>) {
    println!("========== TREE BYTES (root={}) ==========", view.root());
    walk(view, view.root(), 0, true);
}

/// Dumps a page's bytes as a hex/ascii gutter, 16 bytes per line (like
/// `xxd`) — no interpretation, just the raw contents. Shared across
/// page kinds since it's format-agnostic.
fn print_raw(bytes: &[u8]) {
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let offset = i * 16;
        let mut hex = String::with_capacity(48);
        for b in chunk {
            hex.push_str(&format!("{b:02x} "));
        }
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b == b' ' {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        println!("  {offset:06x}  {hex:<48}|{ascii}|");
    }
}
