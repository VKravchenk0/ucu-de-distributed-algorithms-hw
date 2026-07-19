use hw_btree::StorageEngine;

fn main() {
    let path = std::env::temp_dir().join("hw_btree_demo.db");
    let store = StorageEngine::open(&path, 4096).expect("failed to open store");

    store.put(b"hello", b"world").unwrap();
    store.put(b"foo", b"bar").unwrap();

    println!(
        "hello -> {:?}",
        store
            .get(b"hello")
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    );
    println!(
        "foo -> {:?}",
        store
            .get(b"foo")
            .map(|v| String::from_utf8_lossy(&v).into_owned())
    );
    println!("missing -> {:?}", store.get(b"missing"));
    println!("(persisted at {})", path.display());
}
