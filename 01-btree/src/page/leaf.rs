use std::cmp::Ordering;

use crate::io::PageId;
use crate::page::PAGE_TYPE_LEAF;

/// 1B type tag + 8B page id + 2B key count.
pub const LEAF_HEADER: u16 = 11;

/// A leaf page — values live only here. On-disk layout:
///
/// | tag(1B)=0x01 | page id(8B) | key count(2B) | entries... | unused |
///
/// Each entry: `key_len(2B) | key | val_len(2B) | val`, back to back —
/// no offsets/pointer index table. `entry_start` is an in-memory-only
/// cache built once at decode time, so lookups still get O(1) random
/// access / binary search without persisting an index on disk.
pub struct LeafNode {
    id: PageId,
    entries: Vec<u8>,
    entry_start: Vec<u16>, // len = nkeys+1; entry_start[nkeys] == entries.len()
}

impl LeafNode {
    /// A fresh, empty node under construction. 
    /// `capacity_hint` sizes the backing buffer
    /// up front; callers building a possibly-oversized pre-split result
    /// should pass `2*page_size` to avoid reallocation mid-build.
    pub fn empty(capacity_hint: usize) -> Self {
        LeafNode {
            id: 0,
            entries: Vec::with_capacity(capacity_hint),
            entry_start: vec![0],
        }
    }

    pub fn decode(expected_id: PageId, bytes: Vec<u8>) -> Self {
        debug_assert_eq!(bytes[0], PAGE_TYPE_LEAF, "not a leaf page");
        let id = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
        debug_assert_eq!(id, expected_id, "leaf page id mismatch on decode");
        let nkeys = u16::from_le_bytes(bytes[9..11].try_into().unwrap());

        let mut entries = bytes[LEAF_HEADER as usize..].to_vec();
        let mut entry_start = Vec::with_capacity(nkeys as usize + 1);
        entry_start.push(0u16);
        let mut pos = 0u16;
        for _ in 0..nkeys {
            let p = pos as usize;
            let klen = u16::from_le_bytes(entries[p..p + 2].try_into().unwrap());
            let vpos = p + 2 + klen as usize;
            let vlen = u16::from_le_bytes(entries[vpos..vpos + 2].try_into().unwrap());
            pos += 4 + klen + vlen;
            entry_start.push(pos);
        }
        entries.truncate(pos as usize); // drop the trailing zero padding
        LeafNode {
            id,
            entries,
            entry_start,
        }
    }

    pub fn id(&self) -> PageId {
        self.id
    }

    pub fn nkeys(&self) -> u16 {
        (self.entry_start.len() - 1) as u16
    }

    pub fn get_key(&self, idx: u16) -> &[u8] {
        let start = self.entry_start[idx as usize] as usize;
        let klen = u16::from_le_bytes(self.entries[start..start + 2].try_into().unwrap()) as usize;
        &self.entries[start + 2..start + 2 + klen]
    }

    pub fn get_val(&self, idx: u16) -> &[u8] {
        let start = self.entry_start[idx as usize] as usize;
        let klen = u16::from_le_bytes(self.entries[start..start + 2].try_into().unwrap()) as usize;
        let vstart = start + 2 + klen;
        let vlen =
            u16::from_le_bytes(self.entries[vstart..vstart + 2].try_into().unwrap()) as usize;
        &self.entries[vstart + 2..vstart + 2 + vlen]
    }

    pub fn nbytes(&self) -> usize {
        LEAF_HEADER as usize + self.entries.len()
    }

    /// Appends one entry. Keys must be pushed in strictly increasing
    /// order — a cheap guard against a silently-wrong binary search
    /// later.
    pub fn push(&mut self, key: &[u8], val: &[u8]) {
        debug_assert!(
            self.nkeys() == 0 || key > self.get_key(self.nkeys() - 1),
            "leaf keys must be pushed in strictly increasing order"
        );
        self.entries
            .extend_from_slice(&(key.len() as u16).to_le_bytes());
        self.entries.extend_from_slice(key);
        self.entries
            .extend_from_slice(&(val.len() as u16).to_le_bytes());
        self.entries.extend_from_slice(val);
        self.entry_start.push(self.entries.len() as u16);
    }

    pub fn push_range(&mut self, src: &LeafNode, from: u16, to: u16) {
        for i in from..to {
            self.push(src.get_key(i), src.get_val(i));
        }
    }

    pub fn print_pretty(&self, page_size: usize) {
        let header_end = LEAF_HEADER as usize;
        let entries_end = header_end + self.entries.len();
        println!(
            "  type=LEAF id={} nkeys={} used={}B/{page_size}B",
            self.id(),
            self.nkeys(),
            self.nbytes()
        );
        println!("    header:  [0..{header_end})  {header_end}B");
        println!(
            "    entries: [{header_end}..{entries_end})  {}B",
            entries_end - header_end
        );
        println!(
            "    unused:  [{entries_end}..{page_size})  {}B",
            page_size - entries_end
        );
        for i in 0..self.nkeys() {
            println!(
                "  |  [{i}] key={:?} -> val={:?}",
                String::from_utf8_lossy(self.get_key(i)),
                String::from_utf8_lossy(self.get_val(i))
            );
        }
    }

    /// Serializes into a fixed `page_size`-byte page, zero-padded, with
    /// `id` stamped into the self-referential page-id field (not known
    /// until the caller allocates a page for this node).
    pub fn to_bytes(&self, id: PageId, page_size: usize) -> Vec<u8> {
        debug_assert!(self.nbytes() <= page_size);
        let mut buf = vec![0u8; page_size];
        buf[0] = PAGE_TYPE_LEAF;
        buf[1..9].copy_from_slice(&id.to_le_bytes());
        buf[9..11].copy_from_slice(&self.nkeys().to_le_bytes());
        buf[LEAF_HEADER as usize..LEAF_HEADER as usize + self.entries.len()]
            .copy_from_slice(&self.entries);
        buf
    }
}

/// Binary search for `key`. `Ok(idx)`: `node.get_key(idx) == key`.
/// `Err(idx)`: where `key` belongs, in `0..=nkeys()`. Mirrors
/// `slice::binary_search`'s convention.
pub fn leaf_search(node: &LeafNode, key: &[u8]) -> Result<u16, u16> {
    let (mut lo, mut hi): (i32, i32) = (0, node.nkeys() as i32 - 1);
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        match node.get_key(mid as u16).cmp(key) {
            Ordering::Equal => return Ok(mid as u16),
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid - 1,
        }
    }
    Err(lo as u16)
}

/// Builds a new leaf with `(key, val)` inserted at `idx`
pub fn leaf_insert(old: &LeafNode, idx: u16, key: &[u8], val: &[u8]) -> LeafNode {
    let mut new = LeafNode::empty(2 * (old.nbytes() + key.len() + val.len() + 4));
    new.push_range(old, 0, idx);
    new.push(key, val);
    new.push_range(old, idx, old.nkeys());
    new
}

/// Builds a new leaf with the entry at `idx`
pub fn leaf_update(old: &LeafNode, idx: u16, key: &[u8], val: &[u8]) -> LeafNode {
    let mut new = LeafNode::empty(2 * (old.nbytes() + val.len() + 4));
    new.push_range(old, 0, idx);
    new.push(key, val);
    new.push_range(old, idx + 1, old.nkeys());
    new
}

/// Splits an oversized leaf into two that each fit `page_size`, driven
/// by actual serialized size, not an entry-count estimate.
pub fn leaf_split_by_size(old: LeafNode, page_size: usize) -> (LeafNode, LeafNode) {
    debug_assert!(old.nkeys() >= 2);
    debug_assert!(page_size <= u16::MAX as usize);
    let page_size = page_size as u16;

    let mut m = old.nkeys() / 2;
    let left_bytes = |m: u16| -> u16 { LEAF_HEADER + old.entry_start[m as usize] };
    while m >= 1 && left_bytes(m) > page_size {
        m -= 1;
    }
    debug_assert!(
        m >= 1,
        "no single entry can exceed page_size (enforced by check_limits)"
    );

    let right_bytes =
        |m: u16| -> u16 { LEAF_HEADER + (old.entries.len() as u16 - old.entry_start[m as usize]) };
    while m < old.nkeys() - 1 && right_bytes(m) > page_size {
        m += 1;
    }
    debug_assert!(right_bytes(m) <= page_size);
    debug_assert!(m < old.nkeys());

    let mut left = LeafNode::empty(page_size as usize);
    left.push_range(&old, 0, m);
    let mut right = LeafNode::empty(page_size as usize);
    right.push_range(&old, m, old.nkeys());
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(entries: &[(&[u8], &[u8])]) -> LeafNode {
        let mut node = LeafNode::empty(4096);
        for (k, v) in entries {
            node.push(k, v);
        }
        node
    }

    #[test]
    fn round_trip_populated() {
        let node = build(&[(b"a", b"1"), (b"b", b"2"), (b"c", b"3")]);
        let bytes = node.to_bytes(7, 4096);
        let decoded = LeafNode::decode(7, bytes);
        assert_eq!(decoded.id(), 7);
        assert_eq!(decoded.nkeys(), 3);
        assert_eq!(decoded.get_key(0), b"a");
        assert_eq!(decoded.get_val(0), b"1");
        assert_eq!(decoded.get_key(2), b"c");
        assert_eq!(decoded.get_val(2), b"3");
    }

    #[test]
    fn round_trip_empty() {
        let node = LeafNode::empty(64);
        let bytes = node.to_bytes(3, 256);
        let decoded = LeafNode::decode(3, bytes);
        assert_eq!(decoded.nkeys(), 0);
    }

    #[test]
    #[should_panic(expected = "leaf page id mismatch")]
    fn decode_rejects_mismatched_id() {
        let node = build(&[(b"a", b"1")]);
        let bytes = node.to_bytes(5, 256);
        LeafNode::decode(6, bytes);
    }

    #[test]
    fn leaf_search_finds_exact_and_insertion_point() {
        let node = build(&[(b"b", b"1"), (b"d", b"2"), (b"f", b"3")]);
        assert_eq!(leaf_search(&node, b"d"), Ok(1));
        assert_eq!(leaf_search(&node, b"a"), Err(0));
        assert_eq!(leaf_search(&node, b"c"), Err(1));
        assert_eq!(leaf_search(&node, b"z"), Err(3));
    }

    #[test]
    fn split_by_size_preserves_all_entries_and_fits() {
        let page_size = 128usize;
        // Grow one entry at a time until just barely over page_size —
        // the realistic case (one fitting leaf plus one new entry), and
        // the only case a 2-way split can actually handle.
        let mut node = LeafNode::empty(2 * page_size);
        let mut count = 0u32;
        while node.nbytes() <= page_size {
            node.push(format!("key{count:03}").as_bytes(), b"value");
            count += 1;
        }
        let total_keys = node.nkeys();
        assert!(total_keys >= 2, "need at least 2 entries to split");

        let (left, right) = leaf_split_by_size(node, page_size);
        assert!(left.nbytes() <= page_size);
        assert!(right.nbytes() <= page_size);
        assert_eq!(left.nkeys() + right.nkeys(), total_keys);
        for i in 0..left.nkeys() {
            assert_eq!(left.get_key(i), format!("key{i:03}").as_bytes());
        }
        for i in 0..right.nkeys() {
            let orig = i + left.nkeys();
            assert_eq!(right.get_key(i), format!("key{orig:03}").as_bytes());
        }
    }
}
