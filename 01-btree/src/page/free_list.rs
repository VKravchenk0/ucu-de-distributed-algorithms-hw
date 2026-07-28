use crate::io::PageId;
use crate::page::PAGE_TYPE_FREE_LIST;

/// 1B type tag + 8B page id + 8B next page id + 2B count.
pub const FREE_LIST_HEADER: u16 = 19;

/// On-disk layout:
///
/// | tag(1B)=0x03 | page id(8B) | next page id(8B) | count(2B) | free page ids... | unused |
///
/// `next page id == 0` marks the end of the chain. Entries are
/// fixed-width `u64`s, so no offset cache is needed (unlike leaf/internal
/// pages).
pub fn free_list_capacity(page_size: usize) -> usize {
    (page_size - FREE_LIST_HEADER as usize) / 8
}

pub fn free_list_encode(id: PageId, next: PageId, ids: &[PageId], page_size: usize) -> Vec<u8> {
    debug_assert!(ids.len() <= free_list_capacity(page_size));
    let mut buf = vec![0u8; page_size];
    buf[0] = PAGE_TYPE_FREE_LIST;
    buf[1..9].copy_from_slice(&id.to_le_bytes());
    buf[9..17].copy_from_slice(&next.to_le_bytes());
    buf[17..19].copy_from_slice(&(ids.len() as u16).to_le_bytes());
    let mut pos = FREE_LIST_HEADER as usize;
    for &pid in ids {
        buf[pos..pos + 8].copy_from_slice(&pid.to_le_bytes());
        pos += 8;
    }
    buf
}

/// Returns `(next page id, free page ids stored on this page)`.
pub fn free_list_decode(expected_id: PageId, bytes: Vec<u8>) -> (PageId, Vec<PageId>) {
    debug_assert_eq!(bytes[0], PAGE_TYPE_FREE_LIST, "not a free-list page");
    let id = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
    debug_assert_eq!(id, expected_id, "free-list page id mismatch on decode");
    let next = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
    let count = u16::from_le_bytes(bytes[17..19].try_into().unwrap()) as usize;
    let mut ids = Vec::with_capacity(count);
    let mut pos = FREE_LIST_HEADER as usize;
    for _ in 0..count {
        ids.push(u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap()));
        pos += 8;
    }
    (next, ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let bytes = free_list_encode(7, 42, &[100, 200, 300], 256);
        let (next, ids) = free_list_decode(7, bytes);
        assert_eq!(next, 42);
        assert_eq!(ids, vec![100, 200, 300]);
    }

    #[test]
    fn round_trip_end_of_chain() {
        let bytes = free_list_encode(7, 0, &[], 256);
        let (next, ids) = free_list_decode(7, bytes);
        assert_eq!(next, 0);
        assert!(ids.is_empty());
    }

    #[test]
    fn capacity_matches_encode_limit() {
        let page_size = 256;
        let cap = free_list_capacity(page_size);
        let ids: Vec<PageId> = (0..cap as u64).collect();
        let bytes = free_list_encode(1, 0, &ids, page_size);
        let (_, decoded) = free_list_decode(1, bytes);
        assert_eq!(decoded, ids);
    }
}
