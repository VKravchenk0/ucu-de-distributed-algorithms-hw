//! R7.2: integration tests against the real mmap file backend, exercised
//! only through the public API (`StorageEngine::open`/`put`/`get`).

mod common;

use common::TempPath;
use hw_btree::StorageEngine;

#[test]
fn fresh_file_starts_empty() {
    let tmp = TempPath::new("fresh");
    let store = StorageEngine::open(tmp.as_path(), 4096).unwrap();
    assert_eq!(store.get(b"anything"), None);
}

#[test]
fn put_persists_across_close_and_reopen() {
    let tmp = TempPath::new("durability");
    {
        let store = StorageEngine::open(tmp.as_path(), 4096).unwrap();
        for i in 0..50 {
            store
                .put(
                    format!("key{i:03}").as_bytes(),
                    format!("val{i}").as_bytes(),
                )
                .unwrap();
        }
    } // dropped: mmap unmapped, file closed

    let reopened = StorageEngine::open(tmp.as_path(), 4096).unwrap();
    for i in 0..50 {
        assert_eq!(
            reopened.get(format!("key{i:03}").as_bytes()),
            Some(format!("val{i}").into_bytes())
        );
    }
}

#[test]
fn large_split_forcing_dataset_survives_reopen() {
    let tmp = TempPath::new("large");
    {
        // A small page size forces many splits (and multi-level growth)
        // within a manageable key count.
        let store = StorageEngine::open(tmp.as_path(), 256).unwrap();
        for i in 0..300 {
            store
                .put(format!("k{i:04}").as_bytes(), format!("v{i}").as_bytes())
                .unwrap();
        }
    }

    let reopened = StorageEngine::open(tmp.as_path(), 256).unwrap();
    for i in 0..300 {
        assert_eq!(
            reopened.get(format!("k{i:04}").as_bytes()),
            Some(format!("v{i}").into_bytes())
        );
    }
}

#[test]
fn reopening_with_a_mismatched_page_size_errors_without_touching_data() {
    let tmp = TempPath::new("mismatch");
    {
        let store = StorageEngine::open(tmp.as_path(), 4096).unwrap();
        store.put(b"a", b"1").unwrap();
    }

    assert!(StorageEngine::open(tmp.as_path(), 8192).is_err());

    // the mismatch must be rejected, not silently re-bootstrapped over
    // the existing data
    let reopened = StorageEngine::open(tmp.as_path(), 4096).unwrap();
    assert_eq!(reopened.get(b"a"), Some(b"1".to_vec()));
}

#[test]
fn upsert_persists_the_latest_value_across_reopen() {
    let tmp = TempPath::new("upsert");
    {
        let store = StorageEngine::open(tmp.as_path(), 4096).unwrap();
        store.put(b"key", b"v1").unwrap();
        store.put(b"key", b"v2").unwrap();
    }

    let reopened = StorageEngine::open(tmp.as_path(), 4096).unwrap();
    assert_eq!(reopened.get(b"key"), Some(b"v2".to_vec()));
}
