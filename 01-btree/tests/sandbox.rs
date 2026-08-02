mod common;

use hw_btree::StorageEngine;

use common::TempPath;

#[test]
fn sandbox_test() {
    println!("Hello");
    let tmp = TempPath::new("bounded_growth");
    let store = StorageEngine::open(tmp.as_path(), 4096).unwrap();

    println!("Tree:");
    store.print_tree();

    println!(" ");
    println!("Bytes:");
    store.print_bytes();

    store.put(b"key2", b"value2").unwrap();

    println!("Tree1:");
    store.print_tree();

    println!("Bytes1:");
    store.print_bytes();

    store.put(b"key1", b"value1").unwrap();

    println!("Tree2:");
    store.print_tree();

    println!("Bytes2:");
    store.print_bytes();
}
