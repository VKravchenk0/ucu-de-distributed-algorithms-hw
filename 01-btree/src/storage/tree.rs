use crate::io::PageIo;
use crate::page::internal::{
    InternalNode, internal_child_index, internal_insert_split_child, internal_replace_child,
    internal_split_with_promotion,
};
use crate::page::leaf::{LeafNode, leaf_insert, leaf_search, leaf_split_by_size, leaf_update};
use crate::page::{Error, Page, check_limits};
use crate::storage::store::{ReadGuard, WriteTxn};

/// The result of inserting into a node: either it still fits in one
/// page, or it overflowed and had to split. A `Split` always carries
/// exactly one promoted separator key — the caller either absorbs it
/// into a copy of the parent or, if that overflows too, splits again and
/// keeps propagating upward.
enum InsertResult {
    Single(Page),
    Split {
        left: Page,
        sep_key: Vec<u8>,
        right: Page,
    },
}

fn insert_leaf(node: &LeafNode, key: &[u8], val: &[u8], page_size: usize) -> InsertResult {
    let built = match leaf_search(node, key) {
        Ok(idx) => leaf_update(node, idx, key, val),
        Err(idx) => leaf_insert(node, idx, key, val),
    };
    if built.nbytes() <= page_size {
        InsertResult::Single(Page::Leaf(built))
    } else {
        let (left, right) = leaf_split_by_size(built, page_size);
        // Copied, not removed — leaves own all their data; the
        // separator is just a navigation aid pointing at it.
        let sep_key = right.get_key(0).to_vec();
        InsertResult::Split {
            left: Page::Leaf(left),
            sep_key,
            right: Page::Leaf(right),
        }
    }
}

fn insert_internal<IO: PageIo>(
    txn: &mut WriteTxn<IO>,
    node: &InternalNode,
    key: &[u8],
    val: &[u8],
    page_size: usize,
) -> InsertResult {
    let idx = internal_child_index(node, key);
    let child_id = node.get_child(idx);
    let result = match txn.read(child_id) {
        Page::Leaf(l) => insert_leaf(&l, key, val, page_size),
        Page::Internal(i) => insert_internal(txn, &i, key, val, page_size),
    };
    txn.free(child_id); // always orphaned by COW, whether the child split or not

    match result {
        InsertResult::Single(new_child) => {
            let id = txn.alloc();
            txn.write(id, new_child);
            InsertResult::Single(Page::Internal(internal_replace_child(node, idx, id)))
        }
        InsertResult::Split {
            left,
            sep_key,
            right,
        } => {
            let lid = txn.alloc();
            txn.write(lid, left);
            let rid = txn.alloc();
            txn.write(rid, right);
            let grown = internal_insert_split_child(node, idx, &sep_key, lid, rid);
            if grown.nbytes() <= page_size {
                InsertResult::Single(Page::Internal(grown))
            } else {
                let (l2, promoted, r2) = internal_split_with_promotion(grown, page_size);
                InsertResult::Split {
                    left: Page::Internal(l2),
                    sep_key: promoted,
                    right: Page::Internal(r2),
                }
            }
        }
    }
}

/// Inserts or upserts a key, copying every node on the root-to-leaf path
/// and growing the tree by one level if the root splits. An empty root
/// leaf needs no special-casing: `leaf_search` on zero entries returns
/// `Err(0)`, the only possible insertion point, so `insert_leaf` builds
/// a correct 1-entry leaf on the very first `put` through the same path
/// every later `put` uses.
pub fn put<IO: PageIo>(txn: &mut WriteTxn<IO>, key: &[u8], val: &[u8]) -> Result<(), Error> {
    let page_size = txn.page_size();
    check_limits(page_size, key, val)?;

    let root_id = txn.root();
    let result = match txn.read(root_id) {
        Page::Leaf(l) => insert_leaf(&l, key, val, page_size),
        Page::Internal(i) => insert_internal(txn, &i, key, val, page_size),
    };
    txn.free(root_id);

    let new_root = match result {
        InsertResult::Single(node) => {
            let id = txn.alloc();
            txn.write(id, node);
            id
        }
        InsertResult::Split {
            left,
            sep_key,
            right,
        } => {
            // The root split: grow the tree by one level with a brand
            // new, always-minimal root (1 key, 2 children).
            let lid = txn.alloc();
            txn.write(lid, left);
            let rid = txn.alloc();
            txn.write(rid, right);
            let mut root = InternalNode::empty(page_size);
            root.push_key(&sep_key);
            root.push_child(lid);
            root.push_child(rid);
            let id = txn.alloc();
            txn.write(id, Page::Internal(root));
            id
        }
    };

    txn.commit(new_root);
    Ok(())
}

/// Returns the value for `key`, or `None` if absent or the store is
/// empty (an empty root leaf's `leaf_search` naturally returns
/// `Err(0)`, which maps to `None`). Never blocks on a writer — `view`
/// only ever performs wait-free atomic reads plus plain page fetches.
pub fn get<IO: PageIo>(view: &ReadGuard<IO>, key: &[u8]) -> Option<Vec<u8>> {
    let mut page = view.read(view.root());
    loop {
        match page {
            Page::Leaf(l) => return leaf_search(&l, key).ok().map(|idx| l.get_val(idx).to_vec()),
            Page::Internal(i) => {
                let idx = internal_child_index(&i, key);
                page = view.read(i.get_child(idx));
            }
        }
    }
}
