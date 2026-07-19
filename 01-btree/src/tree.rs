use std::cmp::Ordering;

use crate::node::{
    Error, NODE_INTERNAL, NODE_LEAF, Node, append_kv, check_limits, leaf_insert, leaf_update,
    split3,
};
use crate::page_io::PageIo;
use crate::store::{ReadGuard, WriteTxn};

/// Binary search (R1.5) for the largest index whose key is <= `key`. An
/// exact match returns immediately. Keys aren't a plain contiguous
/// slice (each requires `get_key(idx)` decoding), so this is hand-rolled
/// over the index range rather than using a stdlib slice search.
fn lookup_le(node: &Node, key: &[u8]) -> u16 {
    let (mut lo, mut hi): (i32, i32) = (0, node.nkeys() as i32 - 1);
    let mut result: u16 = 0;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        match node.get_key(mid as u16).cmp(key) {
            Ordering::Equal => return mid as u16,
            Ordering::Less => {
                result = mid as u16;
                lo = mid + 1;
            }
            Ordering::Greater => hi = mid - 1,
        }
    }
    result
}

fn insert_recursive<IO: PageIo>(
    txn: &mut WriteTxn<IO>,
    node: &Node,
    key: &[u8],
    val: &[u8],
) -> Node {
    let page_size = txn.page_size();
    let mut new = Node::new(2 * page_size);
    let idx = lookup_le(node, key);
    match node.btype() {
        NODE_LEAF => {
            if node.get_key(idx) == key {
                leaf_update(&mut new, node, idx, key, val);
            } else {
                leaf_insert(&mut new, node, idx + 1, key, val);
            }
        }
        NODE_INTERNAL => {
            let kptr = node.get_ptr(idx);
            let child = txn.read(kptr);
            let updated_child = insert_recursive(txn, &child, key, val);
            let (nsplit, split) = split3(updated_child, page_size);
            txn.free(kptr);
            replace_kid_n(txn, &mut new, node, idx, &split[..nsplit as usize]);
        }
        other => panic!("unexpected node type: {other}"),
    }
    new
}

fn replace_kid_n<IO: PageIo>(
    txn: &mut WriteTxn<IO>,
    new: &mut Node,
    old: &Node,
    idx: u16,
    kids: &[Node],
) {
    let inc = kids.len() as u16;
    new.set_header(NODE_INTERNAL, old.nkeys() + inc - 1);
    crate::node::append_range(new, old, 0, 0, idx);
    for (i, kid) in kids.iter().enumerate() {
        let key0 = kid.get_key(0).to_vec();
        let ptr = txn.alloc();
        txn.write(ptr, kid.clone());
        append_kv(new, idx + i as u16, ptr, &key0, &[]);
    }
    crate::node::append_range(new, old, idx + inc, idx + 1, old.nkeys() - (idx + 1));
}

/// put() (R1.2, R1.4): insert or upsert a key, copying every node on
/// the root-to-leaf path (R2.1) and growing the tree by one level if
/// the root splits (R1.6).
pub fn put<IO: PageIo>(txn: &mut WriteTxn<IO>, key: &[u8], val: &[u8]) -> Result<(), Error> {
    let page_size = txn.page_size();
    check_limits(page_size, key, val)?;

    let root_id = txn.root();
    let root = txn.read(root_id);

    // On the very first put, promote the empty leaf root into a 1-entry
    // leaf with a dummy empty-key sentinel covering the whole key
    // space, so a lookup can always find a containing node. Every
    // later put reuses this same insert_recursive path uniformly.
    let node = if root.nkeys() == 0 {
        let mut bootstrapped = Node::new(page_size);
        bootstrapped.set_header(NODE_LEAF, 1);
        append_kv(&mut bootstrapped, 0, 0, &[], &[]);
        bootstrapped
    } else {
        root
    };

    let updated = insert_recursive(txn, &node, key, val);
    let (nsplit, split) = split3(updated, page_size);
    txn.free(root_id);

    let new_root = if nsplit > 1 {
        let mut new_root_node = Node::new(page_size);
        new_root_node.set_header(NODE_INTERNAL, nsplit);
        for (i, knode) in split[..nsplit as usize].iter().enumerate() {
            let key0 = knode.get_key(0).to_vec();
            let ptr = txn.alloc();
            txn.write(ptr, knode.clone());
            append_kv(&mut new_root_node, i as u16, ptr, &key0, &[]);
        }
        let id = txn.alloc();
        txn.write(id, new_root_node);
        id
    } else {
        let id = txn.alloc();
        txn.write(id, split[0].clone());
        id
    };

    txn.commit(new_root);
    Ok(())
}

/// get() (R1.4): returns the value for `key`, or `None` if absent or
/// the store is empty. Never blocks on a writer (R5.2) — `view` only
/// ever performs wait-free atomic reads plus plain page fetches.
pub fn get<IO: PageIo>(view: &ReadGuard<IO>, key: &[u8]) -> Option<Vec<u8>> {
    let mut node = view.read(view.root());
    loop {
        if node.nkeys() == 0 {
            return None; // empty store: no put() has happened yet
        }
        let idx = lookup_le(&node, key);
        match node.btype() {
            NODE_LEAF => {
                return if node.get_key(idx) == key {
                    Some(node.get_val(idx).to_vec())
                } else {
                    None
                };
            }
            NODE_INTERNAL => {
                let kptr = node.get_ptr(idx);
                node = view.read(kptr);
            }
            other => panic!("unexpected node type: {other}"),
        }
    }
}
