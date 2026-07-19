use std::cmp::Ordering;
use std::ops::{Deref, DerefMut};

pub const BNODE_NODE: u16 = 1; // internal nodes with pointers
pub const BNODE_LEAF: u16 = 2; // leaf nodes with values

pub const HEADER: u16 = 4; // 2B type + 2B nkeys

pub const BTREE_PAGE_SIZE: usize = 4096;
pub const BTREE_MAX_KEY_SIZE: usize = 1000;
pub const BTREE_MAX_VAL_SIZE: usize = 3000;

/// A B+Tree page, dumped to disk as-is.
///
/// Layout:
/// | type | nkeys | pointers | offsets  | key-values | unused |
/// |  2B  |  2B   | nkeys*8B | nkeys*2B |    ...     |        |
///
/// Each key-value entry: | key_size | val_size | key | val |, 2B/2B/../..
/// The first KV's offset is always 0 and is not stored.
#[derive(Clone)]
pub struct BNode(Vec<u8>);

impl Deref for BNode {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl DerefMut for BNode {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl BNode {
    pub fn new(size: usize) -> Self {
        BNode(vec![0u8; size])
    }

    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    pub fn btype(&self) -> u16 {
        u16::from_le_bytes(self[0..2].try_into().unwrap())
    }

    pub fn nkeys(&self) -> u16 {
        u16::from_le_bytes(self[2..4].try_into().unwrap())
    }

    pub fn get_ptr(&self, idx: u16) -> u64 {
        debug_assert!(idx < self.nkeys());
        let pos = (HEADER + 8 * idx) as usize;
        u64::from_le_bytes(self[pos..pos + 8].try_into().unwrap())
    }

    pub fn get_offset(&self, idx: u16) -> u16 {
        if idx == 0 {
            // The first offset is always 0 and is not stored.
            return 0;
        }
        let pos = (HEADER + 8 * self.nkeys() + 2 * (idx - 1)) as usize;
        u16::from_le_bytes(self[pos..pos + 2].try_into().unwrap())
    }

    pub fn kv_pos(&self, idx: u16) -> u16 {
        debug_assert!(idx <= self.nkeys());
        HEADER + 8 * self.nkeys() + 2 * self.nkeys() + self.get_offset(idx)
    }

    pub fn get_key(&self, idx: u16) -> &[u8] {
        debug_assert!(idx < self.nkeys());
        let pos = self.kv_pos(idx) as usize;
        let klen = u16::from_le_bytes(self[pos..pos + 2].try_into().unwrap()) as usize;
        &self[pos + 4..pos + 4 + klen]
    }

    pub fn get_val(&self, idx: u16) -> &[u8] {
        debug_assert!(idx < self.nkeys());
        let pos = self.kv_pos(idx) as usize;
        let klen = u16::from_le_bytes(self[pos..pos + 2].try_into().unwrap()) as usize;
        let vlen = u16::from_le_bytes(self[pos + 2..pos + 4].try_into().unwrap()) as usize;
        &self[pos + 4 + klen..pos + 4 + klen + vlen]
    }

    pub fn nbytes(&self) -> u16 {
        self.kv_pos(self.nkeys())
    }

    pub fn set_header(&mut self, btype: u16, nkeys: u16) {
        self[0..2].copy_from_slice(&btype.to_le_bytes());
        self[2..4].copy_from_slice(&nkeys.to_le_bytes());
    }

    pub fn set_ptr(&mut self, idx: u16, val: u64) {
        debug_assert!(idx < self.nkeys());
        let pos = (HEADER + 8 * idx) as usize;
        self[pos..pos + 8].copy_from_slice(&val.to_le_bytes());
    }

    pub fn set_offset(&mut self, idx: u16, val: u16) {
        let pos = (HEADER + 8 * self.nkeys() + 2 * (idx - 1)) as usize;
        self[pos..pos + 2].copy_from_slice(&val.to_le_bytes());
    }
}

fn node_append_kv(new: &mut BNode, idx: u16, ptr: u64, key: &[u8], val: &[u8]) {
    new.set_ptr(idx, ptr);
    let pos = new.kv_pos(idx) as usize;
    new[pos..pos + 2].copy_from_slice(&(key.len() as u16).to_le_bytes());
    new[pos + 2..pos + 4].copy_from_slice(&(val.len() as u16).to_le_bytes());
    new[pos + 4..pos + 4 + key.len()].copy_from_slice(key);
    new[pos + 4 + key.len()..pos + 4 + key.len() + val.len()].copy_from_slice(val);
    new.set_offset(idx + 1, new.get_offset(idx) + 4 + (key.len() + val.len()) as u16);
}

fn node_append_range(new: &mut BNode, old: &BNode, dst_new: u16, src_old: u16, n: u16) {
    for i in 0..n {
        let (dst, src) = (dst_new + i, src_old + i);
        node_append_kv(new, dst, old.get_ptr(src), old.get_key(src), old.get_val(src));
    }
}

/// Linear scan for the largest index whose key is <= `key` (an exact
/// match returns immediately). The book defers binary search to a later
/// chapter, so this stays a straight scan for now.
fn node_lookup_le(node: &BNode, key: &[u8]) -> u16 {
    let nkeys = node.nkeys();
    let mut i: u16 = 0;
    while i < nkeys {
        match node.get_key(i).cmp(key) {
            Ordering::Equal => return i,
            Ordering::Greater => return i - 1,
            Ordering::Less => {}
        }
        i += 1;
    }
    i - 1
}

fn leaf_insert(new: &mut BNode, old: &BNode, idx: u16, key: &[u8], val: &[u8]) {
    new.set_header(BNODE_LEAF, old.nkeys() + 1);
    node_append_range(new, old, 0, 0, idx);
    node_append_kv(new, idx, 0, key, val);
    node_append_range(new, old, idx + 1, idx, old.nkeys() - idx);
}

fn leaf_update(new: &mut BNode, old: &BNode, idx: u16, key: &[u8], val: &[u8]) {
    new.set_header(BNODE_LEAF, old.nkeys());
    node_append_range(new, old, 0, 0, idx);
    node_append_kv(new, idx, 0, key, val);
    node_append_range(new, old, idx + 1, idx + 1, old.nkeys() - (idx + 1));
}

/// Removes the entry at `idx` from a leaf.
///
/// The book leaves this function's body as an exercise (only the
/// signature is given); it mirrors `leaf_update`'s shape minus the
/// inserted KV in the middle, with `nkeys` shrinking by one.
fn leaf_delete(new: &mut BNode, old: &BNode, idx: u16) {
    new.set_header(BNODE_LEAF, old.nkeys() - 1);
    node_append_range(new, old, 0, 0, idx);
    node_append_range(new, old, idx, idx + 1, old.nkeys() - (idx + 1));
}

fn node_split2(left: &mut BNode, right: &mut BNode, old: &BNode) {
    debug_assert!(old.nkeys() >= 2);

    let mut nleft = old.nkeys() / 2;
    let left_bytes =
        |nleft: u16| -> u16 { HEADER + 8 * nleft + 2 * nleft + old.get_offset(nleft) };
    while left_bytes(nleft) > BTREE_PAGE_SIZE as u16 {
        nleft -= 1;
    }
    debug_assert!(nleft >= 1);

    let right_bytes = |nleft: u16| -> u16 { old.nbytes() - left_bytes(nleft) + HEADER };
    while right_bytes(nleft) > BTREE_PAGE_SIZE as u16 {
        nleft += 1;
    }
    debug_assert!(nleft < old.nkeys());
    let nright = old.nkeys() - nleft;

    left.set_header(old.btype(), nleft);
    right.set_header(old.btype(), nright);
    node_append_range(left, old, 0, 0, nleft);
    node_append_range(right, old, 0, nleft, nright);
    debug_assert!(right.nbytes() <= BTREE_PAGE_SIZE as u16);
}

/// Splits `old` into 1-3 pages that each fit within BTREE_PAGE_SIZE.
/// Only the first `count` (the returned u16) entries of the array are
/// meaningful; the rest are unused empty placeholders, mirroring Go's
/// nil-slice zero value for the unused tail of a `[3]BNode`.
fn node_split3(mut old: BNode) -> (u16, [BNode; 3]) {
    if old.nbytes() as usize <= BTREE_PAGE_SIZE {
        old.truncate(BTREE_PAGE_SIZE);
        return (1, [old, BNode(Vec::new()), BNode(Vec::new())]);
    }

    let mut left = BNode::new(2 * BTREE_PAGE_SIZE);
    let mut right = BNode::new(BTREE_PAGE_SIZE);
    node_split2(&mut left, &mut right, &old);
    if left.nbytes() as usize <= BTREE_PAGE_SIZE {
        left.truncate(BTREE_PAGE_SIZE);
        return (2, [left, right, BNode(Vec::new())]);
    }

    let mut leftleft = BNode::new(BTREE_PAGE_SIZE);
    let mut middle = BNode::new(BTREE_PAGE_SIZE);
    node_split2(&mut leftleft, &mut middle, &left);
    debug_assert!(leftleft.nbytes() as usize <= BTREE_PAGE_SIZE);
    (3, [leftleft, middle, right])
}

/// Merges `left` and `right` (assumed siblings) into one node.
///
/// The book leaves this function's body as an exercise; it's the
/// inverse of `node_split2` — concatenate left's entries then right's.
fn node_merge(new: &mut BNode, left: &BNode, right: &BNode) {
    new.set_header(left.btype(), left.nkeys() + right.nkeys());
    node_append_range(new, left, 0, 0, left.nkeys());
    node_append_range(new, right, left.nkeys(), 0, right.nkeys());
}

pub struct BTree {
    root: u64,
    get: Box<dyn Fn(u64) -> BNode>,
    new: Box<dyn FnMut(BNode) -> u64>,
    del: Box<dyn FnMut(u64)>,
}

fn tree_insert(tree: &mut BTree, node: &BNode, key: &[u8], val: &[u8]) -> BNode {
    let mut new = BNode::new(2 * BTREE_PAGE_SIZE);
    let idx = node_lookup_le(node, key);
    match node.btype() {
        BNODE_LEAF => {
            if node.get_key(idx) == key {
                leaf_update(&mut new, node, idx, key, val);
            } else {
                leaf_insert(&mut new, node, idx + 1, key, val);
            }
        }
        BNODE_NODE => {
            let kptr = node.get_ptr(idx);
            let knode = tree_insert(tree, &(tree.get)(kptr), key, val);
            let (nsplit, split) = node_split3(knode);
            (tree.del)(kptr);
            node_replace_kid_n(tree, &mut new, node, idx, &split[..nsplit as usize]);
        }
        other => panic!("unexpected node type: {other}"),
    }
    new
}

fn node_replace_kid_n(tree: &mut BTree, new: &mut BNode, old: &BNode, idx: u16, kids: &[BNode]) {
    let inc = kids.len() as u16;
    new.set_header(BNODE_NODE, old.nkeys() + inc - 1);
    node_append_range(new, old, 0, 0, idx);
    for (i, kid) in kids.iter().enumerate() {
        let ptr = (tree.new)(kid.clone());
        node_append_kv(new, idx + i as u16, ptr, kid.get_key(0), &[]);
    }
    node_append_range(new, old, idx + inc, idx + 1, old.nkeys() - (idx + 1));
}

/// Replaces the 2 adjacent children at `idx` and `idx+1` with a single
/// merged child (`ptr`, `key`).
///
/// The book leaves this function's body as an exercise; it mirrors
/// `leaf_update`'s shape for an internal node, skipping both old
/// children (`idx+1..idx+2`) instead of just one.
fn node_replace_2kid(new: &mut BNode, old: &BNode, idx: u16, ptr: u64, key: &[u8]) {
    new.set_header(BNODE_NODE, old.nkeys() - 1);
    node_append_range(new, old, 0, 0, idx);
    node_append_kv(new, idx, ptr, key, &[]);
    node_append_range(new, old, idx + 1, idx + 2, old.nkeys() - (idx + 2));
}

/// Decides whether `updated` (a node whose child just shrank) should be
/// merged with a sibling: returns (-1, left sibling), (+1, right
/// sibling), or (0, unused placeholder) if no merge is needed.
///
/// BTREE_PAGE_SIZE/4 is a soft minimum: nodes smaller than a quarter
/// page are eagerly merged with a sibling if the combined result still
/// fits one page, keeping the tree from accumulating many near-empty
/// nodes after repeated deletes.
fn should_merge(tree: &BTree, node: &BNode, idx: u16, updated: &BNode) -> (i8, BNode) {
    if updated.nbytes() > (BTREE_PAGE_SIZE / 4) as u16 {
        return (0, BNode(Vec::new()));
    }

    if idx > 0 {
        let sibling = (tree.get)(node.get_ptr(idx - 1));
        let merged = sibling.nbytes() + updated.nbytes() - HEADER;
        if merged <= BTREE_PAGE_SIZE as u16 {
            return (-1, sibling); // left
        }
    }
    if idx + 1 < node.nkeys() {
        let sibling = (tree.get)(node.get_ptr(idx + 1));
        let merged = sibling.nbytes() + updated.nbytes() - HEADER;
        if merged <= BTREE_PAGE_SIZE as u16 {
            return (1, sibling); // right
        }
    }
    (0, BNode(Vec::new()))
}

fn node_delete(tree: &mut BTree, node: &BNode, idx: u16, key: &[u8]) -> BNode {
    // recurse into the kid
    let kptr = node.get_ptr(idx);
    let updated = tree_delete(tree, &(tree.get)(kptr), key);
    if updated.is_empty() {
        return BNode(Vec::new()); // not found
    }
    (tree.del)(kptr);

    let mut new = BNode::new(BTREE_PAGE_SIZE);
    let (merge_dir, sibling) = should_merge(tree, node, idx, &updated);
    if merge_dir < 0 {
        let mut merged = BNode::new(BTREE_PAGE_SIZE);
        node_merge(&mut merged, &sibling, &updated);
        (tree.del)(node.get_ptr(idx - 1));
        let key0 = merged.get_key(0).to_vec();
        let ptr = (tree.new)(merged);
        node_replace_2kid(&mut new, node, idx - 1, ptr, &key0);
    } else if merge_dir > 0 {
        let mut merged = BNode::new(BTREE_PAGE_SIZE);
        node_merge(&mut merged, &updated, &sibling);
        (tree.del)(node.get_ptr(idx + 1));
        let key0 = merged.get_key(0).to_vec();
        let ptr = (tree.new)(merged);
        node_replace_2kid(&mut new, node, idx, ptr, &key0);
    } else if updated.nkeys() == 0 {
        debug_assert!(node.nkeys() == 1 && idx == 0);
        new.set_header(BNODE_NODE, 0); // the parent becomes empty too
    } else {
        node_replace_kid_n(tree, &mut new, node, idx, std::slice::from_ref(&updated));
    }
    new
}

/// Recursive delete, mirroring `tree_insert`'s structure with merging
/// in place of splitting.
///
/// The book leaves this function's body as an exercise (only the
/// signature is given); `nodeDelete`'s given code fixes its contract:
/// called with (tree, node, idx, key), and expected to recurse back
/// into `tree_delete` for the child at `idx`.
fn tree_delete(tree: &mut BTree, node: &BNode, key: &[u8]) -> BNode {
    let idx = node_lookup_le(node, key);
    match node.btype() {
        BNODE_LEAF => {
            if node.get_key(idx) != key {
                return BNode(Vec::new()); // not found
            }
            let mut new = BNode::new(BTREE_PAGE_SIZE);
            leaf_delete(&mut new, node, idx);
            new
        }
        BNODE_NODE => node_delete(tree, node, idx, key),
        other => panic!("unexpected node type: {other}"),
    }
}

/// Validates a key/value pair against the node format's size limits.
/// The book leaves this function's body as an exercise; this is the
/// only way `insert`/`delete` can fail.
fn check_limit(key: &[u8], val: &[u8]) -> Result<(), String> {
    if key.len() > BTREE_MAX_KEY_SIZE {
        return Err(format!("key too long: {} > {BTREE_MAX_KEY_SIZE}", key.len()));
    }
    if val.len() > BTREE_MAX_VAL_SIZE {
        return Err(format!("value too long: {} > {BTREE_MAX_VAL_SIZE}", val.len()));
    }
    Ok(())
}

impl BTree {
    pub fn insert(&mut self, key: &[u8], val: &[u8]) -> Result<(), String> {
        check_limit(key, val)?;

        if self.root == 0 {
            // create the first node: a dummy empty key covers the whole
            // key space, so a lookup can always find a containing node.
            let mut root = BNode::new(BTREE_PAGE_SIZE);
            root.set_header(BNODE_LEAF, 2);
            node_append_kv(&mut root, 0, 0, &[], &[]);
            node_append_kv(&mut root, 1, 0, key, val);
            self.root = (self.new)(root);
            return Ok(());
        }

        let node = tree_insert(self, &(self.get)(self.root), key, val);
        let (nsplit, split) = node_split3(node);
        (self.del)(self.root);
        if nsplit > 1 {
            // the root split: grow the tree by adding a new root level
            let mut root = BNode::new(BTREE_PAGE_SIZE);
            root.set_header(BNODE_NODE, nsplit);
            for (i, knode) in split[..nsplit as usize].iter().enumerate() {
                let ptr = (self.new)(knode.clone());
                node_append_kv(&mut root, i as u16, ptr, knode.get_key(0), &[]);
            }
            self.root = (self.new)(root);
        } else {
            self.root = (self.new)(split[0].clone());
        }
        Ok(())
    }

    /// The book only gives `Delete`'s signature, not its body. This
    /// mirrors `insert`'s root handling in reverse: where a split grows
    /// the tree by adding a root level, a lone remaining child shrinks
    /// it by removing one — otherwise the tree would keep an
    /// ever-thinner chain of single-child internal nodes after deletes.
    pub fn delete(&mut self, key: &[u8]) -> Result<bool, String> {
        check_limit(key, &[])?;

        if self.root == 0 {
            return Ok(false);
        }

        let updated = tree_delete(self, &(self.get)(self.root), key);
        if updated.is_empty() {
            return Ok(false); // not found
        }

        (self.del)(self.root);
        if updated.btype() == BNODE_NODE && updated.nkeys() == 1 {
            // the root has a single child left: remove a tree level
            self.root = updated.get_ptr(0);
        } else {
            self.root = (self.new)(updated);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    /// A fake in-memory page store, standing in for the real disk-backed
    /// one deferred to a later chapter. Go's book uses the node's memory
    /// address (via `unsafe.Pointer`) as a fake page id; an incrementing
    /// counter is the safe Rust equivalent, with no loss of test fidelity.
    struct FakeStore {
        pages: HashMap<u64, BNode>,
        next_id: u64,
    }

    fn new_test_btree() -> BTree {
        let store = Rc::new(RefCell::new(FakeStore { pages: HashMap::new(), next_id: 1 }));

        let get_store = Rc::clone(&store);
        let get = move |ptr: u64| -> BNode {
            get_store.borrow().pages.get(&ptr).expect("missing page").clone()
        };

        let new_store = Rc::clone(&store);
        let new_fn = move |node: BNode| -> u64 {
            debug_assert!(node.nbytes() as usize <= BTREE_PAGE_SIZE);
            let mut store = new_store.borrow_mut();
            let ptr = store.next_id;
            store.next_id += 1;
            store.pages.insert(ptr, node);
            ptr
        };

        let del = move |ptr: u64| {
            store.borrow_mut().pages.remove(&ptr).expect("missing page");
        };

        BTree { root: 0, get: Box::new(get), new: Box::new(new_fn), del: Box::new(del) }
    }

    /// Walks the tree from the root to find `key`. There's no high-level
    /// `BTree::get` yet (deferred, same as in the book), so tests read
    /// through the low-level node accessors directly.
    fn tree_get(tree: &BTree, key: &[u8]) -> Option<Vec<u8>> {
        if tree.root == 0 {
            return None;
        }
        let mut node = (tree.get)(tree.root);
        loop {
            let idx = node_lookup_le(&node, key);
            match node.btype() {
                BNODE_LEAF => {
                    return if node.get_key(idx) == key {
                        Some(node.get_val(idx).to_vec())
                    } else {
                        None
                    };
                }
                BNODE_NODE => {
                    let kptr = node.get_ptr(idx);
                    node = (tree.get)(kptr);
                }
                other => panic!("unexpected node type: {other}"),
            }
        }
    }

    /// Mirrors the book's `C` test harness: a tree plus a reference
    /// `HashMap` to cross-check against.
    struct TestTree {
        tree: BTree,
        reference: HashMap<String, String>,
    }

    impl TestTree {
        fn new() -> Self {
            TestTree { tree: new_test_btree(), reference: HashMap::new() }
        }

        fn add(&mut self, key: &str, val: &str) {
            self.tree.insert(key.as_bytes(), val.as_bytes()).unwrap();
            self.reference.insert(key.to_string(), val.to_string());
        }

        fn del(&mut self, key: &str) -> bool {
            let found = self.tree.delete(key.as_bytes()).unwrap();
            if found {
                self.reference.remove(key);
            }
            found
        }

        fn verify(&self) {
            for (key, val) in &self.reference {
                assert_eq!(tree_get(&self.tree, key.as_bytes()), Some(val.clone().into_bytes()));
            }
        }
    }

    #[test]
    fn insert_and_lookup_round_trip() {
        let mut t = TestTree::new();
        t.add("hello", "world");
        t.add("foo", "bar");
        t.verify();
        assert_eq!(tree_get(&t.tree, b"missing"), None);
    }

    #[test]
    fn update_existing_key_overwrites_value() {
        let mut t = TestTree::new();
        t.add("key", "v1");
        t.add("key", "v2");
        t.verify();
    }

    #[test]
    fn many_inserts_trigger_split_and_stay_lookupable() {
        let mut t = TestTree::new();
        for i in 0..300 {
            t.add(&format!("key{i:05}"), &format!("val{i}"));
        }
        t.verify();
    }

    #[test]
    fn delete_removes_key() {
        let mut t = TestTree::new();
        t.add("a", "1");
        t.add("b", "2");
        assert!(t.del("a"));
        t.verify();
        assert_eq!(tree_get(&t.tree, b"a"), None);
    }

    #[test]
    fn delete_missing_key_returns_false() {
        let mut t = TestTree::new();
        t.add("a", "1");
        assert!(!t.del("nonexistent"));
    }

    #[test]
    fn many_inserts_then_deletes_trigger_merge_and_stay_consistent() {
        let mut t = TestTree::new();
        for i in 0..300 {
            t.add(&format!("key{i:05}"), &format!("val{i}"));
        }
        for i in (0..300).step_by(2) {
            assert!(t.del(&format!("key{i:05}")));
        }
        t.verify();
        for i in (0..300).step_by(2) {
            assert_eq!(tree_get(&t.tree, format!("key{i:05}").as_bytes()), None);
        }
    }
}
