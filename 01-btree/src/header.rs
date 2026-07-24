use crate::page_io::PageId;

const MAGIC: [u8; 8] = *b"HWBTREE1";
const VERSION: u32 = 1;

/// Page 0 is always the header page (R3.4).
pub const HEADER_PAGE_ID: PageId = 0;

/// The header page (R3.3, R3.4): identifies the file, records the
/// configured page size, and points at the current root and free list.
/// Updated in place on each commit (R2.3 — metadata isn't COW).
///
/// Layout: magic(8B) | version(4B) | page_size(4B) | root_id(8B) |
/// next_page_id(8B) | free_list_head(8B) | free_count(8B)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub page_size: usize,
    pub root_id: PageId,
    pub next_page_id: u64,
    pub free_list_head: PageId,
    pub free_count: u64,
}

impl Header {
    pub fn new(page_size: usize, root_id: PageId, next_page_id: u64) -> Self {
        Header {
            page_size,
            root_id,
            next_page_id,
            free_list_head: 0,
            free_count: 0,
        }
    }

    pub fn to_bytes(self, page_size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; page_size];
        buf[0..8].copy_from_slice(&MAGIC);
        buf[8..12].copy_from_slice(&VERSION.to_le_bytes());
        buf[12..16].copy_from_slice(&(self.page_size as u32).to_le_bytes());
        buf[16..24].copy_from_slice(&self.root_id.to_le_bytes());
        buf[24..32].copy_from_slice(&self.next_page_id.to_le_bytes());
        buf[32..40].copy_from_slice(&self.free_list_head.to_le_bytes());
        buf[40..48].copy_from_slice(&self.free_count.to_le_bytes());
        buf
    }

    /// Returns `None` for anything that isn't a valid, matching-version
    /// header page — a fresh/empty backend included, which the caller
    /// treats as "bootstrap a new store" (R3.4: open-or-create).
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 48 || bytes[0..8] != MAGIC {
            return None;
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
        if version != VERSION {
            return None;
        }
        Some(Header {
            page_size: u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize,
            root_id: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            next_page_id: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
            free_list_head: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
            free_count: u64::from_le_bytes(bytes[40..48].try_into().ok()?),
        })
    }
}
