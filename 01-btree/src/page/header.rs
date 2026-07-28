use crate::io::PageId;
use crate::page::PAGE_TYPE_HEADER;

const MAGIC: [u8; 8] = *b"HWBTREE1";
const VERSION: u32 = 2;

/// Page 0 is always the header page (R3.4).
pub const HEADER_PAGE_ID: PageId = 0;

/// The header page (R3.3, R3.4): identifies the file, records the
/// configured page size, and points at the current root and free-list
/// chain. Updated in place on each commit (R2.3 — metadata isn't COW).
///
/// Layout: type tag(1B)=0x2A | magic(8B) | version(4B) | page_size(4B) |
/// root_id(8B) | next_page_id(8B) | free_list_head(8B) | free_count(8B)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub page_size: usize,
    pub root_id: PageId,
    pub next_page_id: u64,
    /// Id of the first page in the on-disk free-list chain, `0` = none.
    pub free_list_head: PageId,
    /// Diagnostic: total entries across the whole free-list chain.
    pub free_count: u64,
}

/// Why a page 0 didn't decode into a valid `Header`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// No recognizable header at all — a fresh/empty backend, safe to
    /// bootstrap.
    NotAHeader,
    /// A real header page, but from an incompatible format version — a
    /// pre-existing file that must be rejected, not silently
    /// re-bootstrapped over (that would destroy real data).
    VersionMismatch { found: u32, expected: u32 },
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
        buf[0] = PAGE_TYPE_HEADER;
        buf[1..9].copy_from_slice(&MAGIC);
        buf[9..13].copy_from_slice(&VERSION.to_le_bytes());
        buf[13..17].copy_from_slice(&(self.page_size as u32).to_le_bytes());
        buf[17..25].copy_from_slice(&self.root_id.to_le_bytes());
        buf[25..33].copy_from_slice(&self.next_page_id.to_le_bytes());
        buf[33..41].copy_from_slice(&self.free_list_head.to_le_bytes());
        buf[41..49].copy_from_slice(&self.free_count.to_le_bytes());
        buf
    }

    /// `Err(NotAHeader)` for anything that isn't a header page at all —
    /// a fresh/empty backend included, which the caller treats as
    /// "bootstrap a new store" (R3.4: open-or-create). `Err(VersionMismatch)`
    /// for a real but incompatible-version header, which the caller must
    /// treat as a hard error rather than silently rebuilding.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < 49 || bytes[0] != PAGE_TYPE_HEADER || bytes[1..9] != MAGIC[..] {
            return Err(DecodeError::NotAHeader);
        }
        let version = u32::from_le_bytes(bytes[9..13].try_into().unwrap());
        if version != VERSION {
            return Err(DecodeError::VersionMismatch {
                found: version,
                expected: VERSION,
            });
        }
        Ok(Header {
            page_size: u32::from_le_bytes(bytes[13..17].try_into().unwrap()) as usize,
            root_id: u64::from_le_bytes(bytes[17..25].try_into().unwrap()),
            next_page_id: u64::from_le_bytes(bytes[25..33].try_into().unwrap()),
            free_list_head: u64::from_le_bytes(bytes[33..41].try_into().unwrap()),
            free_count: u64::from_le_bytes(bytes[41..49].try_into().unwrap()),
        })
    }
}
