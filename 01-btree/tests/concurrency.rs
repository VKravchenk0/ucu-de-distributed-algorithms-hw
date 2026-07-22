//! R7.3: thread-safety tests against the real mmap file backend.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use common::TempPath;
use hw_btree::StorageEngine;

/// R4.1/R7.3: repeatedly overwriting the same keys must not grow the
/// file unboundedly — the free list must actually be getting reused.
#[test]
fn repeated_overwrites_keep_file_size_bounded() {
    let tmp = TempPath::new("bounded_growth");
    let store = StorageEngine::open(tmp.as_path(), 4096).unwrap();

    let keys: Vec<String> = (0..50).map(|i| format!("key{i:03}")).collect();
    store.print_tree();
    for k in &keys {
        store.put(k.as_bytes(), b"v0").unwrap();
    }
    store.print_tree();
    let baseline = std::fs::metadata(tmp.as_path()).unwrap().len();

    for pass in 0..30 {
        for k in &keys {
            store
                .put(k.as_bytes(), format!("v{pass}").as_bytes())
                .unwrap();
        }
    }
    store.print_tree();
    let after_many_passes = std::fs::metadata(tmp.as_path()).unwrap().len();

    assert!(
        after_many_passes <= baseline * 3,
        "file grew from {baseline} to {after_many_passes} bytes after 30 more overwrite \
         passes over the same 50 keys — free-list reuse doesn't seem to be working"
    );
}

/// R5.1/R5.2/R7.3: many concurrent readers running while a writer keeps
/// updating the same keys must never observe a torn/garbled value (COW
/// correctness), and must complete a large number of reads rather than
/// stalling — proving they aren't blocked by the writer.
#[test]
fn concurrent_readers_never_see_torn_writes_and_are_not_blocked() {
    let tmp = TempPath::new("concurrent_readers");
    let store = Arc::new(StorageEngine::open(tmp.as_path(), 4096).unwrap());

    let keys: [&'static [u8]; 4] = [b"a", b"b", b"c", b"d"];
    for k in keys {
        store.put(k, b"v0").unwrap();
    }
    store.print_tree();

    let stop = Arc::new(AtomicBool::new(false));

    let writer = {
        let store = Arc::clone(&store);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let mut version = 0u64;
            while !stop.load(Ordering::Relaxed) {
                version += 1;
                for k in keys {
                    store.put(k, format!("v{version}").as_bytes()).unwrap();
                }
            }
        })
    };

    let readers: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut reads = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    for k in keys {
                        let val = store
                            .get(k)
                            .expect("key was written before any thread started");
                        let text = String::from_utf8(val)
                            .expect("value must be valid utf8 (never torn/garbled)");
                        assert!(
                            text.strip_prefix('v')
                                .is_some_and(|n| n.parse::<u64>().is_ok()),
                            "observed a garbled value: {text:?}"
                        );
                        reads += 1;
                    }
                }
                reads
            })
        })
        .collect();

    let start = Instant::now();
    thread::sleep(Duration::from_millis(300));
    stop.store(true, Ordering::Relaxed);

    writer.join().unwrap();
    let total_reads: u64 = readers.into_iter().map(|h| h.join().unwrap()).sum();
    let elapsed = start.elapsed();

    // Over 300ms with 8 reader threads, a large number of reads should
    // complete; a handful would indicate readers stuck waiting on the
    // writer's lock rather than proceeding wait-free (R5.2).
    assert!(
        total_reads > 1000,
        "only {total_reads} reads completed in {elapsed:?} across 8 threads — readers may be \
         blocking on the writer"
    );
}
