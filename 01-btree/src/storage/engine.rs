use std::path::Path;

use crate::io::PageIo;
use crate::io::file_backend::MmapPageIo;
use crate::io::in_memory_backend::MemoryPageIo;
use crate::page::Error;
use crate::storage::store::Store;
use crate::storage::tree;

/// A persistent, ordered key-value store backed by a copy-on-write
/// B+Tree (task.md). The only public operations are `put`/`get` (R1.2);
/// deletion is intentionally not implemented.
pub struct StorageEngine {
    store: Store<Box<dyn PageIo>>,
}

/// Why opening a store can fail: either the usual file-I/O reasons
/// (permissions, missing parent directory, ...), or reopening an
/// existing file with a `page_size` that doesn't match what it was
/// created with (R3.4).
#[derive(Debug)]
pub enum OpenError {
    Io(std::io::Error),
    Store(crate::storage::store::OpenError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Io(e) => write!(f, "{e}"),
            OpenError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<std::io::Error> for OpenError {
    fn from(e: std::io::Error) -> Self {
        OpenError::Io(e)
    }
}

impl From<crate::storage::store::OpenError> for OpenError {
    fn from(e: crate::storage::store::OpenError) -> Self {
        OpenError::Store(e)
    }
}

impl StorageEngine {
    /// An in-memory store (R3.5, R7.1) — no persistence, useful for
    /// tests or ephemeral use. Always succeeds: a fresh `MemoryPageIo`
    /// can never hit the page-size-mismatch case `open` guards against.
    pub fn in_memory(page_size: usize) -> Self {
        let io: Box<dyn PageIo> = Box::new(MemoryPageIo::new(page_size));
        StorageEngine {
            store: Store::open_or_create(io).expect("a fresh in-memory backend never mismatches"),
        }
    }

    /// Opens (or creates) a real, mmap-backed file at `path` (R3.1-R3.4).
    /// Reopening an existing file created with a different `page_size`
    /// is an error rather than silently discarding its data.
    pub fn open(path: impl AsRef<Path>, page_size: usize) -> Result<Self, OpenError> {
        let io: Box<dyn PageIo> = Box::new(MmapPageIo::open(path, page_size)?);
        let store = Store::open_or_create(io)?;
        Ok(StorageEngine { store })
    }

    /// Inserts `key`/`val`, or overwrites `key`'s value if it already
    /// exists (R1.4 upsert).
    pub fn put(&self, key: &[u8], val: &[u8]) -> Result<(), Error> {
        let mut txn = self.store.begin_write();
        tree::put(&mut txn, key, val)
    }

    /// Returns `key`'s value, or `None` if it isn't present (R1.4).
    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let view = self.store.enter_read();
        tree::get(&view, key)
    }

    /// Pretty-prints every page in the tree to the console, annotated
    /// with page boundaries and the byte ranges of each node section
    /// (header/pointers/offsets/entries/unused).
    pub fn print_tree(&self) {
        crate::debug::print_tree(&self.store.enter_read());
    }

    /// Dumps every page in the tree to the console as raw bytes.
    pub fn print_bytes(&self) {
        crate::debug::print_bytes(&self.store.enter_read());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page;

    #[test]
    fn put_and_get_round_trip() {
        let store = StorageEngine::in_memory(4096);
        store.put(b"hello", b"world").unwrap();
        store.print_tree();
        store.put(b"foo", b"bar").unwrap();
        store.print_tree();
        assert_eq!(store.get(b"hello"), Some(b"world".to_vec()));
        assert_eq!(store.get(b"foo"), Some(b"bar".to_vec()));
    }

    #[test]
    fn put_upserts_existing_key() {
        let store = StorageEngine::in_memory(4096);
        store.put(b"key", b"v1").unwrap();
        store.print_tree();
        store.put(b"key", b"v2").unwrap();
        store.print_tree();
        assert_eq!(store.get(b"key"), Some(b"v2".to_vec()));
    }

    #[test]
    fn get_missing_key_returns_none() {
        let store = StorageEngine::in_memory(4096);
        store.put(b"a", b"1").unwrap();
        store.print_tree();
        assert_eq!(store.get(b"missing"), None);
    }

    #[test]
    fn get_on_empty_store_returns_none() {
        let store = StorageEngine::in_memory(4096);
        assert_eq!(store.get(b"anything"), None);
    }

    /// Counts leaf pages reachable from the current root, recursing
    /// through internal nodes' children.
    fn count_leaves(store: &StorageEngine) -> usize {
        fn walk(view: &crate::storage::store::ReadGuard<Box<dyn crate::io::PageIo>>, id: crate::io::PageId) -> usize {
            match view.read(id) {
                page::Page::Leaf(_) => 1,
                page::Page::Internal(i) => (0..=i.nkeys()).map(|idx| walk(view, i.get_child(idx))).sum(),
            }
        }
        let view = store.store.enter_read();
        walk(&view, view.root())
    }

    #[test]
    fn bulk_insert_forces_leaf_split_and_stays_lookupable() {
        let store = StorageEngine::in_memory(4096);
        store.print_tree();
        let leaves_before = count_leaves(&store);
        for i in 0..500 {
            store
                .put(
                    format!("key{i:05}").as_bytes(),
                    format!("val{i}").as_bytes(),
                )
                .unwrap();
        }
        store.print_tree();
        let leaves_after = count_leaves(&store);
        assert!(
            leaves_after > leaves_before,
            "expected bulk insert to split leaves: {leaves_before} -> {leaves_after}"
        );
        for i in 0..500 {
            assert_eq!(
                store.get(format!("key{i:05}").as_bytes()),
                Some(format!("val{i}").into_bytes())
            );
        }
    }

    #[test]
    fn small_page_size_forces_multi_level_root_growth() {
        // A small page size forces internal-node splits (not just leaf
        // splits) and multi-level tree growth without needing a huge
        // key count (R1.6).
        let store = StorageEngine::in_memory(256);
        store.print_tree();
        for i in 0..300 {
            store.put(format!("k{i:04}").as_bytes(), b"v").unwrap();
        }
        store.print_tree();
        for i in 0..300 {
            assert_eq!(
                store.get(format!("k{i:04}").as_bytes()),
                Some(b"v".to_vec())
            );
        }
    }

    #[test]
    fn boundary_entry_size_is_enforced() {
        let page_size = 4096;
        let max = page::max_kv_size(page_size);

        let store = StorageEngine::in_memory(page_size);
        let key_at_max = vec![b'k'; max];
        assert!(store.put(&key_at_max, b"").is_ok());
        store.print_tree();
        assert_eq!(store.get(&key_at_max), Some(Vec::new()));

        let key_over_max = vec![b'k'; max + 1];
        assert!(store.put(&key_over_max, b"").is_err());
    }
}
