use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{PageId, PageIo};
use crate::mmap_ffi::RawMmap;

/// Reserved virtual address space, chosen once at open time. The mapping
/// is never remapped/resized for the store's lifetime (R3.2) — growth
/// only ever extends the backing file within this ceiling. Generous by
/// default since unused reserved virtual space is free on 64-bit Linux;
/// exceeding it is a documented Phase 2 limitation (no online resize).
const DEFAULT_MAX_SIZE: u64 = 1 << 30; // 1 GiB

/// The real, file-backed `PageIo` (R3.1-R3.4): a fixed-size-page file,
/// cached via `mmap`.
pub struct MmapPageIo {
    file: File,
    mmap: RawMmap,
    page_size: usize,
    /// How much of the reserved mapping is actually backed by the file
    /// so far (`ftruncate`'d). Only ever grows; writes past this extend
    /// the file first, since touching unbacked mapped bytes is `SIGBUS`.
    committed_len: AtomicU64,
}

impl MmapPageIo {
    /// Opens (or creates) `path` as a page file (R3.4: open-or-create).
    pub fn open(path: impl AsRef<Path>, page_size: usize) -> io::Result<Self> {
        Self::open_with_max_size(path, page_size, DEFAULT_MAX_SIZE)
    }

    pub fn open_with_max_size(
        path: impl AsRef<Path>,
        page_size: usize,
        max_size: u64,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let current_len = file.metadata()?.len();

        // Page 0 (header) must always be addressable, even for a brand-new
        // empty file, so the initial commit covers at least one page.
        let initial_len = current_len.max(page_size as u64);
        if initial_len != current_len {
            file.set_len(initial_len)?;
        }

        let mmap_len = max_size.max(initial_len) as usize;
        let mmap = RawMmap::new(&file, mmap_len)?;

        Ok(MmapPageIo {
            file,
            mmap,
            page_size,
            committed_len: AtomicU64::new(initial_len),
        })
    }

    fn ensure_committed(&self, needed: u64) {
        let committed = self.committed_len.load(Ordering::SeqCst);
        if needed > committed {
            debug_assert!(
                needed <= self.mmap.len() as u64,
                "grew past the reserved mmap ceiling"
            );
            self.file
                .set_len(needed)
                .expect("failed to grow backing file");
            self.committed_len.store(needed, Ordering::SeqCst);
        }
    }
}

impl PageIo for MmapPageIo {
    fn page_size(&self) -> usize {
        self.page_size
    }

    fn read_page(&self, id: PageId) -> Vec<u8> {
        let offset = id as usize * self.page_size;
        self.mmap.read_at(offset, self.page_size)
    }

    fn write_page(&self, id: PageId, bytes: &[u8]) {
        debug_assert_eq!(bytes.len(), self.page_size);
        let offset = id as usize * self.page_size;
        self.ensure_committed((offset + self.page_size) as u64);
        self.mmap.write_at(offset, bytes);
    }

    fn sync(&self) {
        self.mmap.sync().expect("msync failed");
    }
}
