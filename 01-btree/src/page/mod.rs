pub mod free_list;
pub mod header;
pub mod internal;
pub mod leaf;

use crate::io::PageId;
use internal::{INTERNAL_HEADER, InternalNode};
use leaf::LeafNode;

/// Shared 1-byte page-type tag convention (offset 0 of every page).
pub const PAGE_TYPE_HEADER: u8 = 0x2A;
pub const PAGE_TYPE_LEAF: u8 = 0x01;
pub const PAGE_TYPE_INTERNAL: u8 = 0x02;
pub const PAGE_TYPE_FREE_LIST: u8 = 0x03;

/// A decoded B+Tree page: exactly one of the two node kinds a tree page
/// can be. Free-list and header pages are metadata, not tree nodes, and
/// aren't represented here — they're handled directly by
/// `storage::store`.
pub enum Page {
    Leaf(LeafNode),
    Internal(InternalNode),
}

impl Page {
    pub fn decode(id: PageId, bytes: Vec<u8>) -> Self {
        match bytes[0] {
            PAGE_TYPE_LEAF => Page::Leaf(LeafNode::decode(id, bytes)),
            PAGE_TYPE_INTERNAL => Page::Internal(InternalNode::decode(id, bytes)),
            other => panic!("unexpected page type: {other}"),
        }
    }

    pub fn to_bytes(&self, id: PageId, page_size: usize) -> Vec<u8> {
        match self {
            Page::Leaf(l) => l.to_bytes(id, page_size),
            Page::Internal(i) => i.to_bytes(id, page_size),
        }
    }
}

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

/// The largest combined key+value size that's always safe to insert.
///
/// Derived from the binding case: an internal node must always be able
/// to hold 2 max-size separators + 3 children at once, so that when it
/// overflows by one key, `internal_split_with_promotion` always finds a
/// valid split point. Leaf entries share the same bound — a separator is
/// just a bare leaf key with no value, so the internal-node case is the
/// tighter constraint.
pub fn max_kv_size(page_size: usize) -> usize {
    page_size.saturating_sub(INTERNAL_HEADER as usize + 28) / 2
}

pub fn check_limits(page_size: usize, key: &[u8], val: &[u8]) -> Result<(), Error> {
    let max = max_kv_size(page_size);
    let len = key.len() + val.len();
    if len > max {
        return Err(Error::EntryTooLarge { len, max });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_kv_size_matches_derivation() {
        // (page_size - INTERNAL_HEADER(11) - 28) / 2
        assert_eq!(max_kv_size(4096), (4096 - 11 - 28) / 2);
        assert_eq!(max_kv_size(4096), 2028);
        assert_eq!(max_kv_size(256), (256 - 11 - 28) / 2);
        assert_eq!(max_kv_size(256), 108);
    }
}
