use hw_btree::StorageEngine;

fn main() {
    let store = StorageEngine::in_memory(4096);
    store.put(b"hello", b"world").unwrap();
    println!("{:?}", store.get(b"hello"));
}
