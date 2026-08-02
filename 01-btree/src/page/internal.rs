use crate::io::PageId;
use crate::page::PAGE_TYPE_INTERNAL;

/// 1B type tag + 8B page id + 2B key count.
pub const INTERNAL_HEADER: u16 = 11;

/// An internal page: N separator keys, N+1 child pointers. 
/// On-disk layout:
///
/// | tag(1B)=0x02 | page id(8B) | key count N(2B) | keys... | children... | unused |
///
/// Keys: `key_len(2B) | key`, repeated N times, no values. Children:
/// `child page id(8B)`, repeated N+1 times, fixed-width and contiguous
/// right after the keys.
///
/// Convention: `child[0]` covers keys `< key[0]`; `child[i]` (`0<i<N`)
/// covers `[key[i-1], key[i])`; `child[N]` covers `>= key[N-1]`.
pub struct InternalNode {
    id: PageId,
    keys: Vec<u8>,
    key_start: Vec<u16>,   // len = nkeys+1; key_start[nkeys] == keys.len()
    children: Vec<PageId>, // len = nkeys+1
}

impl InternalNode {
    pub fn empty(capacity_hint: usize) -> Self {
        InternalNode {
            id: 0,
            keys: Vec::with_capacity(capacity_hint),
            key_start: vec![0],
            children: Vec::new(),
        }
    }

    pub fn decode(expected_id: PageId, bytes: Vec<u8>) -> Self {
        debug_assert_eq!(bytes[0], PAGE_TYPE_INTERNAL, "not an internal page");
        let id = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
        debug_assert_eq!(id, expected_id, "internal page id mismatch on decode");
        let nkeys = u16::from_le_bytes(bytes[9..11].try_into().unwrap());

        let mut key_start = Vec::with_capacity(nkeys as usize + 1);
        key_start.push(0u16);
        let mut pos = INTERNAL_HEADER as usize;
        for _ in 0..nkeys {
            let klen = u16::from_le_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2 + klen;
            key_start.push((pos - INTERNAL_HEADER as usize) as u16);
        }
        let keys_end = pos;
        let keys = bytes[INTERNAL_HEADER as usize..keys_end].to_vec();

        let mut children = Vec::with_capacity(nkeys as usize + 1);
        let mut cpos = keys_end;
        for _ in 0..=nkeys {
            children.push(u64::from_le_bytes(
                bytes[cpos..cpos + 8].try_into().unwrap(),
            ));
            cpos += 8;
        }

        InternalNode {
            id,
            keys,
            key_start,
            children,
        }
    }

    pub fn id(&self) -> PageId {
        self.id
    }

    pub fn nkeys(&self) -> u16 {
        (self.key_start.len() - 1) as u16
    }

    pub fn nchildren(&self) -> u16 {
        self.nkeys() + 1
    }

    pub fn get_key(&self, idx: u16) -> &[u8] {
        let start = self.key_start[idx as usize] as usize;
        let end = self.key_start[idx as usize + 1] as usize;
        &self.keys[start + 2..end]
    }

    pub fn get_child(&self, idx: u16) -> PageId {
        self.children[idx as usize]
    }

    pub fn nbytes(&self) -> usize {
        INTERNAL_HEADER as usize + self.keys.len() + 8 * self.children.len()
    }

    pub fn push_key(&mut self, key: &[u8]) {
        debug_assert!(
            self.nkeys() == 0 || key > self.get_key(self.nkeys() - 1),
            "internal separator keys must be pushed in strictly increasing order"
        );
        self.keys
            .extend_from_slice(&(key.len() as u16).to_le_bytes());
        self.keys.extend_from_slice(key);
        self.key_start.push(self.keys.len() as u16);
    }

    pub fn push_child(&mut self, child: PageId) {
        self.children.push(child);
    }

    pub fn print_pretty(&self, page_size: usize) {
        let header_end = INTERNAL_HEADER as usize;
        let keys_end = header_end + self.keys.len();
        let children_end = keys_end + 8 * self.children.len();
        println!(
            "  type=INTERNAL id={} nkeys={} nchildren={} used={}B/{page_size}B",
            self.id(),
            self.nkeys(),
            self.nchildren(),
            self.nbytes()
        );
        println!("    header:   [0..{header_end})  {header_end}B");
        println!(
            "    keys:     [{header_end}..{keys_end})  {}B",
            keys_end - header_end
        );
        println!(
            "    children: [{keys_end}..{children_end})  {}B",
            children_end - keys_end
        );
        println!(
            "    unused:   [{children_end}..{page_size})  {}B",
            page_size - children_end
        );
        for i in 0..self.nkeys() {
            println!(
                "  |  [{i}] key={:?}",
                String::from_utf8_lossy(self.get_key(i))
            );
        }
        let children: Vec<PageId> = (0..self.nchildren()).map(|i| self.get_child(i)).collect();
        println!("  `- children: {children:?}");
    }

    pub fn to_bytes(&self, id: PageId, page_size: usize) -> Vec<u8> {
        debug_assert!(self.nbytes() <= page_size);
        debug_assert_eq!(self.children.len(), self.nkeys() as usize + 1);
        let mut buf = vec![0u8; page_size];
        buf[0] = PAGE_TYPE_INTERNAL;
        buf[1..9].copy_from_slice(&id.to_le_bytes());
        buf[9..11].copy_from_slice(&self.nkeys().to_le_bytes());
        let keys_end = INTERNAL_HEADER as usize + self.keys.len();
        buf[INTERNAL_HEADER as usize..keys_end].copy_from_slice(&self.keys);
        let mut cpos = keys_end;
        for &child in &self.children {
            buf[cpos..cpos + 8].copy_from_slice(&child.to_le_bytes());
            cpos += 8;
        }
        buf
    }
}

pub fn internal_child_index(node: &InternalNode, key: &[u8]) -> u16 {
    let (mut lo, mut hi) = (0u16, node.nkeys());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if node.get_key(mid) <= key {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

pub fn internal_replace_child(node: &InternalNode, idx: u16, new_child: PageId) -> InternalNode {
    let mut new = InternalNode::empty(node.keys.len());
    for i in 0..node.nkeys() {
        new.push_key(node.get_key(i));
    }
    for i in 0..node.nchildren() {
        new.push_child(if i == idx {
            new_child
        } else {
            node.get_child(i)
        });
    }
    new
}

/// Splices a split child's `(left, sep_key, right)` in at `child_idx`
/// (the index that used to point at the now-split child). Result has
/// N+1 keys / N+2 children — may overflow `page_size`; the caller checks
/// `nbytes()` and splits again via `internal_split_with_promotion` if so.
///
/// Example: keys `[k0,k1]`, children `[c0,c1,c2]`, `child_idx=1` (`c1`
/// split) -> keys `[k0,sep,k1]`, children `[c0,left,right,c2]`. The
/// `child_idx=0` and `child_idx=N` edges splice in the same way, no
/// special-casing needed.
pub fn internal_insert_split_child(
    node: &InternalNode,
    child_idx: u16,
    sep_key: &[u8],
    left: PageId,
    right: PageId,
) -> InternalNode {
    let mut new = InternalNode::empty(node.keys.len() + 2 + sep_key.len());
    for i in 0..child_idx {
        new.push_key(node.get_key(i));
    }
    new.push_key(sep_key);
    for i in child_idx..node.nkeys() {
        new.push_key(node.get_key(i));
    }

    for i in 0..child_idx {
        new.push_child(node.get_child(i));
    }
    new.push_child(left);
    new.push_child(right);
    for i in (child_idx + 1)..node.nchildren() {
        new.push_child(node.get_child(i));
    }
    new
}

/// Median-key promotion: the chosen split key is removed from both
/// halves and returned separately, unlike a leaf split where the
/// separator is copied. Returns `(left, promoted_key, right)`.
///
/// `left` gets keys `[0..m)` + children `[0..=m]`; `right` gets keys
/// `(m..np)` + children `(m..=np]` — disjoint ranges, so children
/// conserve exactly with none duplicated.
pub fn internal_split_with_promotion(
    grown: InternalNode,
    page_size: usize,
) -> (InternalNode, Vec<u8>, InternalNode) {
    debug_assert!(grown.nkeys() >= 2);
    debug_assert!(page_size <= u16::MAX as usize);
    let page_size = page_size as u16;
    let np = grown.nkeys();

    let mut m = np / 2;
    let left_bytes =
        |m: u16| -> u16 { INTERNAL_HEADER + grown.key_start[m as usize] + 8 * (m + 1) };
    while m >= 1 && left_bytes(m) > page_size {
        m -= 1;
    }
    debug_assert!(
        m >= 1,
        "2 max-size separators + 3 children always fit (max_kv_size derivation)"
    );

    let right_bytes = |m: u16| -> u16 {
        INTERNAL_HEADER
            + (grown.keys.len() as u16 - grown.key_start[(m + 1) as usize])
            + 8 * (np - m)
    };
    while m < np - 1 && right_bytes(m) > page_size {
        m += 1;
    }
    debug_assert!(
        m <= np - 2,
        "no valid split point found — max_kv_size bound violated"
    );
    debug_assert!(left_bytes(m) <= page_size && right_bytes(m) <= page_size);

    let mut left = InternalNode::empty(page_size as usize);
    for i in 0..m {
        left.push_key(grown.get_key(i));
    }
    for i in 0..=m {
        left.push_child(grown.get_child(i));
    }

    let promoted = grown.get_key(m).to_vec();

    let mut right = InternalNode::empty(page_size as usize);
    for i in (m + 1)..np {
        right.push_key(grown.get_key(i));
    }
    for i in (m + 1)..=np {
        right.push_child(grown.get_child(i));
    }

    (left, promoted, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(keys: &[&[u8]], children: &[PageId]) -> InternalNode {
        let mut node = InternalNode::empty(4096);
        for k in keys {
            node.push_key(k);
        }
        for &c in children {
            node.push_child(c);
        }
        node
    }

    #[test]
    fn round_trip() {
        let node = build(&[b"b", b"d", b"f"], &[10, 20, 30, 40]);
        let bytes = node.to_bytes(9, 4096);
        let decoded = InternalNode::decode(9, bytes);
        assert_eq!(decoded.id(), 9);
        assert_eq!(decoded.nkeys(), 3);
        assert_eq!(decoded.nchildren(), 4);
        assert_eq!(decoded.get_key(0), b"b");
        assert_eq!(decoded.get_key(2), b"f");
        assert_eq!(decoded.get_child(0), 10);
        assert_eq!(decoded.get_child(3), 40);
    }

    #[test]
    #[should_panic(expected = "internal page id mismatch")]
    fn decode_rejects_mismatched_id() {
        let node = build(&[b"a"], &[1, 2]);
        let bytes = node.to_bytes(5, 256);
        InternalNode::decode(6, bytes);
    }

    #[test]
    fn child_index_boundary_table() {
        let node = build(&[b"b", b"d", b"f"], &[0, 1, 2, 3]);
        let cases: &[(&[u8], u16)] = &[
            (b"a", 0),
            (b"b", 1),
            (b"c", 1),
            (b"d", 2),
            (b"e", 2),
            (b"f", 3),
            (b"z", 3),
        ];
        for (key, expected) in cases {
            assert_eq!(
                internal_child_index(&node, key),
                *expected,
                "key {:?}",
                String::from_utf8_lossy(key)
            );
        }
    }

    #[test]
    fn insert_split_child_worked_example() {
        let node = build(&[b"d", b"f"], &[100, 101, 102]);
        let grown = internal_insert_split_child(&node, 1, b"e", 200, 201);
        assert_eq!(grown.nkeys(), 3);
        assert_eq!(grown.get_key(0), b"d");
        assert_eq!(grown.get_key(1), b"e");
        assert_eq!(grown.get_key(2), b"f");
        assert_eq!(
            (0..grown.nchildren())
                .map(|i| grown.get_child(i))
                .collect::<Vec<_>>(),
            vec![100, 200, 201, 102]
        );
    }

    #[test]
    fn insert_split_child_boundary_zero() {
        let node = build(&[b"d", b"f"], &[100, 101, 102]);
        let grown = internal_insert_split_child(&node, 0, b"b", 200, 201);
        assert_eq!(
            (0..grown.nkeys())
                .map(|i| grown.get_key(i).to_vec())
                .collect::<Vec<_>>(),
            vec![b"b".to_vec(), b"d".to_vec(), b"f".to_vec()]
        );
        assert_eq!(
            (0..grown.nchildren())
                .map(|i| grown.get_child(i))
                .collect::<Vec<_>>(),
            vec![200, 201, 101, 102]
        );
    }

    #[test]
    fn insert_split_child_boundary_end() {
        let node = build(&[b"d", b"f"], &[100, 101, 102]);
        let grown = internal_insert_split_child(&node, 2, b"h", 200, 201);
        assert_eq!(
            (0..grown.nkeys())
                .map(|i| grown.get_key(i).to_vec())
                .collect::<Vec<_>>(),
            vec![b"d".to_vec(), b"f".to_vec(), b"h".to_vec()]
        );
        assert_eq!(
            (0..grown.nchildren())
                .map(|i| grown.get_child(i))
                .collect::<Vec<_>>(),
            vec![100, 101, 200, 201]
        );
    }

    #[test]
    fn split_with_promotion_removes_median_from_both_halves() {
        let page_size = 128usize;
        let mut node = InternalNode::empty(4096);
        let keys: Vec<String> = (0..12).map(|i| format!("key{i:03}")).collect();
        for k in &keys {
            node.push_key(k.as_bytes());
        }
        for i in 0..=keys.len() as u64 {
            node.push_child(i + 100);
        }
        let total_children_before = node.nchildren();

        let (left, promoted, right) = internal_split_with_promotion(node, page_size);
        assert!(left.nbytes() <= page_size);
        assert!(right.nbytes() <= page_size);

        // No key lost or duplicated: left keys + [promoted] + right keys == original keys, in order.
        let mut all_keys: Vec<Vec<u8>> = (0..left.nkeys())
            .map(|i| left.get_key(i).to_vec())
            .collect();
        all_keys.push(promoted.clone());
        all_keys.extend((0..right.nkeys()).map(|i| right.get_key(i).to_vec()));
        let expected: Vec<Vec<u8>> = keys.iter().map(|k| k.as_bytes().to_vec()).collect();
        assert_eq!(all_keys, expected);
        assert!(
            !left.get_key(left.nkeys() - 1).eq(promoted.as_slice()),
            "promoted key must not remain in left"
        );
        assert!(
            !(0..right.nkeys()).any(|i| right.get_key(i) == promoted.as_slice()),
            "promoted key must not remain in right"
        );

        // Children conserved exactly, no duplication.
        assert_eq!(left.nchildren() + right.nchildren(), total_children_before);
    }

    #[test]
    fn two_max_size_separators_fit_without_splitting() {
        let page_size = 256usize;
        let max = crate::page::max_kv_size(page_size);
        let mut node = InternalNode::empty(page_size);
        node.push_key(&vec![b'a'; max]);
        node.push_key(&vec![b'z'; max]);
        node.push_child(1);
        node.push_child(2);
        node.push_child(3);
        assert!(
            node.nbytes() <= page_size,
            "2 max-size separators + 3 children ({}) must fit page_size ({page_size})",
            node.nbytes()
        );
    }
}
