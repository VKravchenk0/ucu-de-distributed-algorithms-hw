use std::ops::{Deref, DerefMut};

/// Internal nodes hold separator keys and child pointers (R1.1).
pub const NODE_INTERNAL: u16 = 1;
/// Leaf nodes hold the actual key-value pairs (R1.1).
pub const NODE_LEAF: u16 = 2;

pub const HEADER: u16 = 4; // 2B type + 2B nkeys

/// Fixed per-entry overhead: 8B child pointer + 2B offset + 4B (key_len+val_len) header.
const ENTRY_OVERHEAD: usize = 8 + 2 + 4;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    EntryTooLarge { len: usize, max: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::EntryTooLarge { len, max } => write!(
                f,
                "key+value size {len} exceeds the per-entry maximum of {max} for this page size"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// The largest combined key+value size (R1.3) that's always safe to
/// insert.
///
/// This has to satisfy a tighter bound than "fits alone in a page": when
/// a root splits (via `split3`, which caps at 3 pieces), `put` builds a
/// brand-new root holding up to 3 separator keys copied from those
/// pieces — and, unlike every other internal node update, that specific
/// new-root page is never itself re-checked/re-split afterward. So the
/// binding constraint is "3 max-size keys, as bare separators (empty
/// value), must still fit in one page alongside the fixed header."
pub fn max_kv_size(page_size: usize) -> usize {
    page_size.saturating_sub(HEADER as usize + 3 * ENTRY_OVERHEAD) / 3
}

pub fn check_limits(page_size: usize, key: &[u8], val: &[u8]) -> Result<(), Error> {
    let max = max_kv_size(page_size);
    let len = key.len() + val.len();
    if len > max {
        return Err(Error::EntryTooLarge { len, max });
    }
    Ok(())
}

/// A B+Tree node, serialized as a page (R3.3).
///
/// Layout:
/// | type | nkeys | pointers | offsets  | key-values | unused |
/// |  2B  |  2B   | nkeys*8B | nkeys*2B |    ...     |        |
///
/// Each key-value entry: | key_size | val_size | key | val |, 2B/2B/../..
/// The first KV's offset is always 0 and is not stored.
#[derive(Clone)]
pub struct Node(Vec<u8>);

impl Deref for Node {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl DerefMut for Node {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Node {
    pub fn new(size: usize) -> Self {
        Node(vec![0u8; size])
    }

    pub fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    /// Serializes into a fixed `page_size`-byte page, zero-padded — the
    /// boundary at which a `Node` becomes the bytes a `PageIo` stores.
    pub fn into_bytes(mut self, page_size: usize) -> Vec<u8> {
        debug_assert!(self.nbytes() as usize <= page_size);
        self.0.resize(page_size, 0);
        self.0
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Node(bytes)
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

    fn type_name(&self) -> &'static str {
        match self.btype() {
            NODE_LEAF => "LEAF",
            NODE_INTERNAL => "INTERNAL",
            _ => "UNKNOWN",
        }
    }

    /// Dumps every byte of the page as a hex/ascii gutter, 16 bytes per
    /// line (like `xxd`) — no interpretation, just the raw contents.
    pub fn print_raw(&self) {
        for (i, chunk) in self.0.chunks(16).enumerate() {
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

    /// Dumps the parsed layout with an explicit byte range for each
    /// section (header/pointers/offsets/entries/unused), mirroring
    /// DESIGN.md's node-page table. Internal nodes show `key -> child
    /// ptr`; leaves show `key -> val`.
    pub fn print_pretty(&self) {
        let nkeys = self.nkeys();
        let used = self.nbytes();
        let total = self.0.len();
        println!(
            "  type={} nkeys={nkeys} used={used}B page={total}B unused={}B",
            self.type_name(),
            total.saturating_sub(used as usize)
        );

        println!(
            "  |- header    [0..{HEADER})            type={}({}) nkeys={nkeys}",
            self.btype(),
            self.type_name()
        );

        let ptr_start = HEADER;
        let ptr_end = HEADER + 8 * nkeys;
        let ptrs: String = (0..nkeys)
            .map(|i| format!("[{i}]={} ", self.get_ptr(i)))
            .collect();
        println!("  |- pointers  [{ptr_start}..{ptr_end}){:>8}{ptrs}", "");

        let off_start = ptr_end;
        let off_end = off_start + 2 * nkeys;
        let offs: String = (1..nkeys)
            .map(|i| format!("[{i}]={} ", self.get_offset(i)))
            .collect();
        println!("  |- offsets   [{off_start}..{off_end}){:>8}{offs}", "");

        println!("  |- entries   [{off_end}..{used})");
        for i in 0..nkeys {
            let start = self.kv_pos(i);
            let end = self.kv_pos(i + 1);
            let key = String::from_utf8_lossy(self.get_key(i));
            if self.btype() == NODE_INTERNAL {
                println!(
                    "  |    [{i}] [{start}..{end})  key={key:?} -> child_ptr={}",
                    self.get_ptr(i)
                );
            } else {
                let val = String::from_utf8_lossy(self.get_val(i));
                println!("  |    [{i}] [{start}..{end})  key={key:?} -> val={val:?}");
            }
        }

        println!(
            "  `- unused    [{used}..{total})  {}B zero-padded",
            total.saturating_sub(used as usize)
        );
    }
}

pub fn append_kv(new: &mut Node, idx: u16, ptr: u64, key: &[u8], val: &[u8]) {
    new.set_ptr(idx, ptr);
    let pos = new.kv_pos(idx) as usize;
    new[pos..pos + 2].copy_from_slice(&(key.len() as u16).to_le_bytes());
    new[pos + 2..pos + 4].copy_from_slice(&(val.len() as u16).to_le_bytes());
    new[pos + 4..pos + 4 + key.len()].copy_from_slice(key);
    new[pos + 4 + key.len()..pos + 4 + key.len() + val.len()].copy_from_slice(val);
    new.set_offset(
        idx + 1,
        new.get_offset(idx) + 4 + (key.len() + val.len()) as u16,
    );
}

pub fn append_range(new: &mut Node, old: &Node, dst_new: u16, src_old: u16, n: u16) {
    for i in 0..n {
        let (dst, src) = (dst_new + i, src_old + i);
        append_kv(
            new,
            dst,
            old.get_ptr(src),
            old.get_key(src),
            old.get_val(src),
        );
    }
}

pub fn leaf_insert(new: &mut Node, old: &Node, idx: u16, key: &[u8], val: &[u8]) {
    new.set_header(NODE_LEAF, old.nkeys() + 1);
    append_range(new, old, 0, 0, idx);
    append_kv(new, idx, 0, key, val);
    append_range(new, old, idx + 1, idx, old.nkeys() - idx);
}

pub fn leaf_update(new: &mut Node, old: &Node, idx: u16, key: &[u8], val: &[u8]) {
    new.set_header(NODE_LEAF, old.nkeys());
    append_range(new, old, 0, 0, idx);
    append_kv(new, idx, 0, key, val);
    append_range(new, old, idx + 1, idx + 1, old.nkeys() - (idx + 1));
}

pub fn split2(left: &mut Node, right: &mut Node, old: &Node, page_size: usize) {
    debug_assert!(old.nkeys() >= 2);
    debug_assert!(page_size <= u16::MAX as usize);
    let page_size = page_size as u16;

    let mut nleft = old.nkeys() / 2;
    let left_bytes = |nleft: u16| -> u16 { HEADER + 8 * nleft + 2 * nleft + old.get_offset(nleft) };
    while left_bytes(nleft) > page_size {
        nleft -= 1;
    }
    debug_assert!(nleft >= 1);

    let right_bytes = |nleft: u16| -> u16 { old.nbytes() - left_bytes(nleft) + HEADER };
    while right_bytes(nleft) > page_size {
        nleft += 1;
    }
    debug_assert!(nleft < old.nkeys());
    let nright = old.nkeys() - nleft;

    left.set_header(old.btype(), nleft);
    right.set_header(old.btype(), nright);
    append_range(left, old, 0, 0, nleft);
    append_range(right, old, 0, nleft, nright);
    debug_assert!(right.nbytes() <= page_size);
}

/// Splits `old` into 1-3 pages that each fit within `page_size` (R1.6,
/// R6.2). Only the first `count` (the returned u16) entries of the
/// array are meaningful; the rest are unused empty placeholders.
pub fn split3(mut old: Node, page_size: usize) -> (u16, [Node; 3]) {
    if old.nbytes() as usize <= page_size {
        old.truncate(page_size);
        return (1, [old, Node(Vec::new()), Node(Vec::new())]);
    }

    let mut left = Node::new(2 * page_size);
    let mut right = Node::new(page_size);
    split2(&mut left, &mut right, &old, page_size);
    if left.nbytes() as usize <= page_size {
        left.truncate(page_size);
        return (2, [left, right, Node(Vec::new())]);
    }

    let mut leftleft = Node::new(page_size);
    let mut middle = Node::new(page_size);
    split2(&mut leftleft, &mut middle, &left, page_size);
    debug_assert!(leftleft.nbytes() as usize <= page_size);
    (3, [leftleft, middle, right])
}
