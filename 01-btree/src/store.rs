use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::meta::{META_PAGE_ID, Meta};
use crate::node::{NODE_LEAF, Node};
use crate::page_io::{PageId, PageIo};

const INITIAL_ROOT_PAGE_ID: PageId = 1;

#[derive(Debug)]
pub enum OpenError {
    PageSizeMismatch { requested: usize, persisted: usize },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::PageSizeMismatch {
                requested,
                persisted,
            } => write!(
                f,
                "requested page size {requested} doesn't match this store's page size {persisted}"
            ),
        }
    }
}

impl std::error::Error for OpenError {}

struct WriterState {
    next_page_id: u64,
    free_list: Vec<PageId>,
    pending_free: Vec<PageId>,
}

/// Owns a `PageIo` backend and the COW commit/reclamation/concurrency
/// state built on top of it (R2, R4, R5) — generic over the backend so
/// this logic is written and tested once, not duplicated per backend.
pub struct Store<IO: PageIo> {
    io: IO,
    root: AtomicU64,                // R2.2: the single atomically-published root
    readers: AtomicUsize,           // R4.2/R5.2: lock-free reader quiescence gate
    write_lock: Mutex<WriterState>, // R5.3: single-writer serialization
}

impl<IO: PageIo> Store<IO> {
    /// Reads (and validates) the meta page, or bootstraps a fresh store
    /// with an empty leaf root (R3.4: "a new file starts with an empty
    /// leaf root"). Works identically for a brand-new backend and a
    /// real reopened file — the memory backend goes through the exact
    /// same path so this logic is exercised long before mmap exists.
    ///
    /// A backend with no valid meta page at all (fresh/empty) is always
    /// safe to bootstrap. A backend with a *valid* meta page whose
    /// page_size disagrees with the caller's request is a real error,
    /// not a fresh backend — silently re-bootstrapping there would
    /// clobber existing data instead of reporting the mismatch (R3.4).
    pub fn open_or_create(io: IO) -> Result<Self, OpenError> {
        let page_size = io.page_size();
        let meta = Meta::from_bytes(&io.read_page(META_PAGE_ID));

        let meta = match meta {
            Some(m) if m.page_size == page_size => m,
            Some(m) => {
                return Err(OpenError::PageSizeMismatch {
                    requested: page_size,
                    persisted: m.page_size,
                });
            }
            None => {
                let mut root = Node::new(page_size);
                root.set_header(NODE_LEAF, 0);
                io.write_page(INITIAL_ROOT_PAGE_ID, &root.into_bytes(page_size));
                let m = Meta::new(page_size, INITIAL_ROOT_PAGE_ID, INITIAL_ROOT_PAGE_ID + 1);
                io.write_page(META_PAGE_ID, &m.to_bytes(page_size));
                io.sync();
                m
            }
        };

        Ok(Store {
            root: AtomicU64::new(meta.root_id),
            readers: AtomicUsize::new(0),
            write_lock: Mutex::new(WriterState {
                next_page_id: meta.next_page_id,
                free_list: Vec::new(),
                pending_free: Vec::new(),
            }),
            io,
        })
    }

    pub fn enter_read(&self) -> ReadGuard<'_, IO> {
        self.readers.fetch_add(1, Ordering::SeqCst);
        ReadGuard { store: self }
    }

    pub fn begin_write(&self) -> WriteTxn<'_, IO> {
        let guard = self.write_lock.lock().unwrap();
        let base_root = self.root.load(Ordering::SeqCst);
        WriteTxn {
            store: self,
            guard,
            writes: Vec::new(),
            freed: Vec::new(),
            base_root,
        }
    }

    /// Number of pages currently sitting in the reusable free list —
    /// used by reclamation tests to confirm space is actually reused.
    #[cfg(test)]
    pub fn free_list_len(&self) -> usize {
        self.write_lock.lock().unwrap().free_list.len()
    }

    /// The bump-allocator high-water mark — used by reclamation tests to
    /// confirm the page count stays bounded under repeated overwrites.
    #[cfg(test)]
    pub fn allocated_pages(&self) -> u64 {
        self.write_lock.lock().unwrap().next_page_id
    }
}

/// A lock-free, wait-free read snapshot (R5.2): never touches
/// `write_lock`, so it can never block on or be blocked by a writer.
pub struct ReadGuard<'a, IO: PageIo> {
    store: &'a Store<IO>,
}

impl<IO: PageIo> Drop for ReadGuard<'_, IO> {
    fn drop(&mut self) {
        self.store.readers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<IO: PageIo> ReadGuard<'_, IO> {
    pub fn root(&self) -> PageId {
        self.store.root.load(Ordering::SeqCst)
    }

    pub fn read(&self, id: PageId) -> Node {
        Node::from_bytes(self.store.io.read_page(id))
    }
}

/// A single serialized write transaction (R5.3). Buffers page
/// writes/frees and only makes them visible on `commit`.
pub struct WriteTxn<'a, IO: PageIo> {
    store: &'a Store<IO>,
    guard: MutexGuard<'a, WriterState>,
    writes: Vec<(PageId, Vec<u8>)>,
    freed: Vec<PageId>,
    base_root: PageId,
}

impl<IO: PageIo> WriteTxn<'_, IO> {
    pub fn page_size(&self) -> usize {
        self.store.io.page_size()
    }

    pub fn root(&self) -> PageId {
        self.base_root
    }

    /// Reads a page, checking this transaction's own not-yet-flushed
    /// writes first (read-your-own-writes).
    pub fn read(&self, id: PageId) -> Node {
        if let Some((_, bytes)) = self.writes.iter().rev().find(|(pid, _)| *pid == id) {
            return Node::from_bytes(bytes.clone());
        }
        Node::from_bytes(self.store.io.read_page(id))
    }

    /// Allocates a page id, preferring free-list reuse over extending
    /// the file (R4.3).
    pub fn alloc(&mut self) -> PageId {
        if let Some(id) = self.guard.free_list.pop() {
            return id;
        }
        let id = self.guard.next_page_id;
        self.guard.next_page_id += 1;
        id
    }

    pub fn write(&mut self, id: PageId, node: Node) {
        self.writes.push((id, node.into_bytes(self.page_size())));
    }

    /// Marks a page as orphaned by this write (R2.1: the old page along
    /// the copied path). Not immediately reusable — see `commit`.
    pub fn free(&mut self, id: PageId) {
        self.freed.push(id);
    }

    /// The copy-on-write commit protocol (R2.2, R2.3, R4.2):
    /// 1. Apply buffered page writes.
    /// 2. Fold this transaction's frees into `pending_free`.
    /// 3. Quiescence gate: only promote `pending_free` into the
    ///    reusable `free_list` if no reader is currently active — a
    ///    page can only be reused once no reader could still observe
    ///    it, and an observed reader count of zero is a sufficient
    ///    (if conservative) proof of that.
    /// 4. Persist metadata (R2.3: updated in place, not COW).
    /// 5. Sync, then publish the new root via a single atomic store.
    pub fn commit(&mut self, new_root: PageId) {
        for (id, bytes) in &self.writes {
            self.store.io.write_page(*id, bytes);
        }
        self.guard.pending_free.append(&mut self.freed);

        if self.store.readers.load(Ordering::SeqCst) == 0 {
            let mut promoted = std::mem::take(&mut self.guard.pending_free);
            self.guard.free_list.append(&mut promoted);
        }

        let meta = Meta {
            page_size: self.page_size(),
            root_id: new_root,
            next_page_id: self.guard.next_page_id,
            free_list_head: 0, // Phase 1: free list lives in memory only, not yet persisted
            free_count: self.guard.free_list.len() as u64,
        };
        self.store
            .io
            .write_page(META_PAGE_ID, &meta.to_bytes(self.page_size()));
        self.store.io.sync();
        self.store.root.store(new_root, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_io::memory::MemoryPageIo;
    use crate::tree;

    #[test]
    fn open_or_create_bootstraps_empty_leaf_root() {
        let store = Store::open_or_create(MemoryPageIo::new(4096)).unwrap();
        let view = store.enter_read();
        let root = view.read(view.root());
        assert_eq!(root.btype(), NODE_LEAF);
        assert_eq!(root.nkeys(), 0);
    }

    #[test]
    fn reopening_the_same_backend_recovers_root_and_data() {
        let io = MemoryPageIo::new(4096);
        let store = Store::open_or_create(io.reopen()).unwrap();
        crate::dump::print_tree(&store.enter_read());
        for i in 0..20 {
            let mut txn = store.begin_write();
            tree::put(&mut txn, format!("k{i}").as_bytes(), b"v").unwrap();
        }
        crate::dump::print_tree(&store.enter_read());

        // simulate closing and reopening: a fresh Store wrapping the
        // same underlying "disk".
        let reopened = Store::open_or_create(io.reopen()).unwrap();
        let view = reopened.enter_read();
        for i in 0..20 {
            assert_eq!(
                tree::get(&view, format!("k{i}").as_bytes()),
                Some(b"v".to_vec())
            );
        }
    }

    #[test]
    fn repeated_overwrites_reuse_pages_via_free_list() {
        let store = Store::open_or_create(MemoryPageIo::new(4096)).unwrap();
        let keys: Vec<String> = (0..50).map(|i| format!("key{i:03}")).collect();

        crate::dump::print_tree(&store.enter_read());
        for k in &keys {
            let mut txn = store.begin_write();
            tree::put(&mut txn, k.as_bytes(), b"v1").unwrap();
        }
        crate::dump::print_tree(&store.enter_read());
        let pages_after_first_pass = store.allocated_pages();
        assert!(
            store.free_list_len() > 0,
            "expected some pages freed by path-copying already"
        );

        for _ in 0..20 {
            for k in &keys {
                let mut txn = store.begin_write();
                tree::put(&mut txn, k.as_bytes(), b"v2").unwrap();
            }
        }
        crate::dump::print_tree(&store.enter_read());
        let pages_after_many_passes = store.allocated_pages();

        assert!(
            pages_after_many_passes < pages_after_first_pass * 2,
            "page count grew from {pages_after_first_pass} to {pages_after_many_passes} after 20 \
             more overwrite passes — free-list reuse doesn't seem to be working"
        );
    }
}
