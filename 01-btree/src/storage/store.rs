use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::io::{PageId, PageIo};
use crate::page::Page;
use crate::page::free_list::{free_list_capacity, free_list_decode, free_list_encode};
use crate::page::header::{DecodeError, HEADER_PAGE_ID, Header};
use crate::page::leaf::LeafNode;

const INITIAL_ROOT_PAGE_ID: PageId = 1;

#[derive(Debug)]
pub enum OpenError {
    PageSizeMismatch { requested: usize, persisted: usize },
    VersionMismatch { found: u32, expected: u32 },
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
            OpenError::VersionMismatch { found, expected } => write!(
                f,
                "file format version {found} is incompatible with this build's version {expected}"
            ),
        }
    }
}

impl std::error::Error for OpenError {}

struct WriterState {
    next_page_id: u64,
    free_list: Vec<PageId>,
    pending_free: Vec<PageId>,
    /// The dedicated, permanent chain of pages that store the free list
    /// itself on disk — always bump-allocated, never popped from
    /// `free_list` (see `WriteTxn::persist_free_list`).
    free_list_page_ids: Vec<PageId>,
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

/// Walks the on-disk free-list chain starting at `head` (`0` = empty),
/// collecting every free page id and every chain-storage page id
/// visited (so they're recognized as reusable storage, not leaked, on
/// the next commit).
fn load_free_list_chain<IO: PageIo>(io: &IO, head: PageId) -> (Vec<PageId>, Vec<PageId>) {
    let (mut free_list, mut page_ids) = (Vec::new(), Vec::new());
    let mut cur = head;
    while cur != 0 {
        let (next, entries) = free_list_decode(cur, io.read_page(cur));
        free_list.extend(entries);
        page_ids.push(cur);
        cur = next;
    }
    (free_list, page_ids)
}

impl<IO: PageIo> Store<IO> {
    /// Reads (and validates) the header page, or bootstraps a fresh store
    /// with an empty leaf root (R3.4: "a new file starts with an empty
    /// leaf root"). Works identically for a brand-new backend and a
    /// real reopened file — the memory backend goes through the exact
    /// same path so this logic is exercised long before mmap exists.
    ///
    /// A backend with no valid header page at all (fresh/empty) is always
    /// safe to bootstrap. A backend with a *valid* header page whose
    /// page_size disagrees with the caller's request, or whose format
    /// version doesn't match this build's, is a real error, not a fresh
    /// backend — silently re-bootstrapping there would clobber existing
    /// data instead of reporting the mismatch (R3.4).
    pub fn open_or_create(io: IO) -> Result<Self, OpenError> {
        let page_size = io.page_size();
        let header = match Header::from_bytes(&io.read_page(HEADER_PAGE_ID)) {
            Ok(h) if h.page_size == page_size => h,
            Ok(h) => {
                return Err(OpenError::PageSizeMismatch {
                    requested: page_size,
                    persisted: h.page_size,
                });
            }
            Err(DecodeError::VersionMismatch { found, expected }) => {
                return Err(OpenError::VersionMismatch { found, expected });
            }
            Err(DecodeError::NotAHeader) => {
                let root = LeafNode::empty(page_size);
                io.write_page(
                    INITIAL_ROOT_PAGE_ID,
                    &root.to_bytes(INITIAL_ROOT_PAGE_ID, page_size),
                );
                let h = Header::new(page_size, INITIAL_ROOT_PAGE_ID, INITIAL_ROOT_PAGE_ID + 1);
                io.write_page(HEADER_PAGE_ID, &h.to_bytes(page_size));
                io.sync();
                h
            }
        };

        let (free_list, free_list_page_ids) = load_free_list_chain(&io, header.free_list_head);

        Ok(Store {
            root: AtomicU64::new(header.root_id),
            readers: AtomicUsize::new(0),
            write_lock: Mutex::new(WriterState {
                next_page_id: header.next_page_id,
                free_list,
                pending_free: Vec::new(),
                free_list_page_ids,
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

    /// A snapshot of the reusable free list's contents — used by
    /// reclamation tests that need to confirm a *specific* recovered id
    /// gets reused, not just that the count matches.
    #[cfg(test)]
    pub fn free_list_snapshot(&self) -> Vec<PageId> {
        self.write_lock.lock().unwrap().free_list.clone()
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

    pub fn read(&self, id: PageId) -> Page {
        Page::decode(id, self.store.io.read_page(id))
    }

    pub fn page_size(&self) -> usize {
        self.store.io.page_size()
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
    pub fn read(&self, id: PageId) -> Page {
        if let Some((_, bytes)) = self.writes.iter().rev().find(|(pid, _)| *pid == id) {
            return Page::decode(id, bytes.clone());
        }
        Page::decode(id, self.store.io.read_page(id))
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

    pub fn write(&mut self, id: PageId, page: Page) {
        self.writes.push((id, page.to_bytes(id, self.page_size())));
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
    /// 4. Persist the free list to its on-disk chain.
    /// 5. Persist metadata (R2.3: updated in place, not COW).
    /// 6. Sync, then publish the new root via a single atomic store.
    pub fn commit(&mut self, new_root: PageId) {
        for (id, bytes) in &self.writes {
            self.store.io.write_page(*id, bytes);
        }
        self.guard.pending_free.append(&mut self.freed);

        if self.store.readers.load(Ordering::SeqCst) == 0 {
            let mut promoted = std::mem::take(&mut self.guard.pending_free);
            self.guard.free_list.append(&mut promoted);
        }

        let (free_list_head, free_count) = self.persist_free_list();

        let header = Header {
            page_size: self.page_size(),
            root_id: new_root,
            next_page_id: self.guard.next_page_id,
            free_list_head,
            free_count,
        };
        self.store
            .io
            .write_page(HEADER_PAGE_ID, &header.to_bytes(self.page_size()));
        self.store.io.sync();
        self.store.root.store(new_root, Ordering::SeqCst);
    }

    /// Persists the free list to its dedicated on-disk chain (R2.3:
    /// metadata, updated in place — not COW). Writes the UNION of
    /// `free_list` and `pending_free`, not just `free_list`: safe
    /// because a fresh process start always begins with `readers==0`
    /// (R4.2's "a reader that could have observed it" cannot survive a
    /// process boundary), so anything still pending at the last
    /// shutdown is unconditionally reclaimable by whoever reopens next.
    /// This process's own in-memory split — still gated by the live
    /// quiescence check in `commit` — is untouched; only what's written
    /// to disk is widened. This fully closes the free-list durability
    /// gap (pages leaked if the process exited mid-batch), not just
    /// narrows it.
    ///
    /// Storage pages for the chain are bump-allocated only, never
    /// popped from `free_list` — popping the very list being serialized
    /// this commit would be a fixed-point problem (popping changes the
    /// list's length, which changes how many storage pages are needed,
    /// which changes what you'd need to pop...). Once allocated, a
    /// storage page is reused (overwritten in place) forever after,
    /// even if the list later shrinks — a small, bounded, permanent
    /// cost in exchange for a design simple enough to obviously be
    /// correct.
    fn persist_free_list(&mut self) -> (PageId, u64) {
        let page_size = self.page_size();
        let cap = free_list_capacity(page_size).max(1);
        let payload: Vec<PageId> = self
            .guard
            .free_list
            .iter()
            .copied()
            .chain(self.guard.pending_free.iter().copied())
            .collect();
        let pages_needed = payload.len().div_ceil(cap);

        while self.guard.free_list_page_ids.len() < pages_needed {
            let id = self.guard.next_page_id;
            self.guard.next_page_id += 1;
            self.guard.free_list_page_ids.push(id);
        }
        for i in 0..pages_needed {
            let id = self.guard.free_list_page_ids[i];
            let next = if i + 1 < pages_needed {
                self.guard.free_list_page_ids[i + 1]
            } else {
                0
            };
            let chunk = &payload[i * cap..((i + 1) * cap).min(payload.len())];
            self.store
                .io
                .write_page(id, &free_list_encode(id, next, chunk, page_size));
        }
        let head = if pages_needed > 0 {
            self.guard.free_list_page_ids[0]
        } else {
            0
        };
        (head, payload.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::in_memory_backend::MemoryPageIo;
    use crate::storage::tree;

    #[test]
    fn open_or_create_bootstraps_empty_leaf_root() {
        let store = Store::open_or_create(MemoryPageIo::new(4096)).unwrap();
        let view = store.enter_read();
        match view.read(view.root()) {
            Page::Leaf(l) => assert_eq!(l.nkeys(), 0),
            Page::Internal(_) => panic!("expected a fresh leaf root"),
        }
    }

    #[test]
    fn reopening_the_same_backend_recovers_root_and_data() {
        let io = MemoryPageIo::new(4096);
        let store = Store::open_or_create(io.reopen()).unwrap();
        crate::debug::print_tree(&store.enter_read());
        for i in 0..20 {
            let mut txn = store.begin_write();
            tree::put(&mut txn, format!("k{i}").as_bytes(), b"v").unwrap();
        }
        crate::debug::print_tree(&store.enter_read());

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

        crate::debug::print_tree(&store.enter_read());
        for k in &keys {
            let mut txn = store.begin_write();
            tree::put(&mut txn, k.as_bytes(), b"v1").unwrap();
        }
        crate::debug::print_tree(&store.enter_read());
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
        crate::debug::print_tree(&store.enter_read());
        let pages_after_many_passes = store.allocated_pages();

        assert!(
            pages_after_many_passes < pages_after_first_pass * 2,
            "page count grew from {pages_after_first_pass} to {pages_after_many_passes} after 20 \
             more overwrite passes — free-list reuse doesn't seem to be working"
        );
    }

    /// Closes the durability gap DESIGN.md used to document: the free
    /// list must survive a close/reopen cycle, not reset to empty, and
    /// the recovered ids must actually be usable (not just present in a
    /// count).
    #[test]
    fn free_list_survives_reopen() {
        let io = MemoryPageIo::new(4096);
        let store = Store::open_or_create(io.reopen()).unwrap();
        let keys: Vec<String> = (0..50).map(|i| format!("key{i:03}")).collect();
        for k in &keys {
            let mut txn = store.begin_write();
            tree::put(&mut txn, k.as_bytes(), b"v1").unwrap();
        }
        for k in &keys {
            let mut txn = store.begin_write();
            tree::put(&mut txn, k.as_bytes(), b"v2").unwrap();
        }
        let free_before_reopen = store.free_list_len();
        assert!(
            free_before_reopen > 0,
            "expected some pages freed by path-copying"
        );

        let reopened = Store::open_or_create(io.reopen()).unwrap();
        assert_eq!(
            reopened.free_list_len(),
            free_before_reopen,
            "free list should be fully recovered across reopen"
        );

        let free_ids_before_alloc = reopened.free_list_snapshot();
        let allocated_id = {
            let mut txn = reopened.begin_write();
            txn.alloc()
        };
        assert!(
            free_ids_before_alloc.contains(&allocated_id),
            "alloc() right after reopen should pop a page id recovered from the on-disk free \
             list, not bump next_page_id"
        );

        let view = reopened.enter_read();
        for k in &keys {
            assert_eq!(tree::get(&view, k.as_bytes()), Some(b"v2".to_vec()));
        }
    }

    /// Forces the free list to outgrow one free-list page's capacity,
    /// so recovery must actually walk the `next_page_id` chain — catches
    /// a "forgot to follow `next`" bug specifically (a length-capped-at
    /// one-page recovery would silently under-count instead).
    #[test]
    fn free_list_spans_multiple_chained_pages() {
        // Sequential puts alone can't build a backlog bigger than one
        // free-list page: each put frees ~tree-height pages but also
        // *allocates* ~tree-height pages (preferring the free list
        // first), so free_list_len() naturally plateaus at a small,
        // roughly constant size — that's the reclamation scheme working
        // as intended (see repeated_overwrites_reuse_pages_via_free_list).
        // To force a real backlog, hold a reader open: the quiescence
        // gate then can't promote anything, so every commit's frees
        // pile up in pending_free instead of being drained by the next
        // commit's allocs.
        let page_size = 128;
        let cap = free_list_capacity(page_size);
        let io = MemoryPageIo::new(page_size);
        let store = Store::open_or_create(io.reopen()).unwrap();

        let keys: Vec<String> = (0..8).map(|i| format!("key{i:02}")).collect();
        for k in &keys {
            let mut txn = store.begin_write();
            tree::put(&mut txn, k.as_bytes(), b"v0").unwrap();
        }

        let held_reader = store.enter_read();
        for _ in 0..(cap + 5) {
            for k in &keys {
                let mut txn = store.begin_write();
                tree::put(&mut txn, k.as_bytes(), b"v1").unwrap();
            }
        }
        drop(held_reader);

        // One more commit with no readers held promotes the whole
        // accumulated batch from pending_free into the reusable free list.
        {
            let mut txn = store.begin_write();
            tree::put(&mut txn, b"trigger", b"v").unwrap();
        }

        let free_before = store.free_list_len();
        assert!(
            free_before > cap,
            "expected the free list ({free_before}) to exceed one page's capacity ({cap})"
        );

        let reopened = Store::open_or_create(io.reopen()).unwrap();
        assert_eq!(
            reopened.free_list_len(),
            free_before,
            "chained free-list pages must all be walked on reopen, not just the head"
        );
        let view = reopened.enter_read();
        for k in &keys {
            assert_eq!(tree::get(&view, k.as_bytes()), Some(b"v1".to_vec()));
        }
        assert_eq!(tree::get(&view, b"trigger"), Some(b"v".to_vec()));
    }
}
