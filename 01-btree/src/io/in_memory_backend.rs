use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{PageId, PageIo};

/// An in-memory `PageIo` backend
#[derive(Clone)]
pub struct MemoryPageIo {
    page_size: usize,
    pages: Arc<Mutex<HashMap<PageId, Vec<u8>>>>,
}

impl MemoryPageIo {
    pub fn new(page_size: usize) -> Self {
        MemoryPageIo {
            page_size,
            pages: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// A handle sharing the same underlying storage — lets tests simulate
    /// closing and reopening a store against the same "disk" ahead of the
    /// real mmap backend existing.
    #[cfg(test)]
    pub fn reopen(&self) -> Self {
        MemoryPageIo {
            page_size: self.page_size,
            pages: Arc::clone(&self.pages),
        }
    }
}

impl PageIo for MemoryPageIo {
    fn page_size(&self) -> usize {
        self.page_size
    }

    fn read_page(&self, id: PageId) -> Vec<u8> {
        self.pages
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| vec![0u8; self.page_size])
    }

    fn write_page(&self, id: PageId, bytes: &[u8]) {
        debug_assert_eq!(bytes.len(), self.page_size);
        self.pages.lock().unwrap().insert(id, bytes.to_vec());
    }

    fn sync(&self) {}
}
