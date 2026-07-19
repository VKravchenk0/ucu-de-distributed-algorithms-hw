pub mod memory;
pub mod mmap;

/// Identifies a fixed-size page. `0` is reserved for the meta page and
/// otherwise used as a "null" sentinel (e.g. "no free list yet").
pub type PageId = u64;

/// The only swappable-backend boundary (R3.5): plain byte I/O over
/// fixed-size pages. All B+Tree/COW/free-list/concurrency policy lives
/// once, generically, in `Store<IO>` — not duplicated per backend.
pub trait PageIo: Send + Sync {
    fn page_size(&self) -> usize;
    /// Always returns exactly `page_size()` bytes; an unwritten page
    /// reads back as zeros (matching a freshly `ftruncate`'d file).
    fn read_page(&self, id: PageId) -> Vec<u8>;
    /// `bytes.len()` must equal `page_size()`.
    fn write_page(&self, id: PageId, bytes: &[u8]);
    /// Durably flushes prior writes (a no-op for the in-memory backend).
    fn sync(&self);
}

impl PageIo for Box<dyn PageIo> {
    fn page_size(&self) -> usize {
        (**self).page_size()
    }
    fn read_page(&self, id: PageId) -> Vec<u8> {
        (**self).read_page(id)
    }
    fn write_page(&self, id: PageId, bytes: &[u8]) {
        (**self).write_page(id, bytes)
    }
    fn sync(&self) {
        (**self).sync()
    }
}
