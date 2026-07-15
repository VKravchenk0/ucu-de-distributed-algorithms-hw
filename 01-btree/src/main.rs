use hello_rust::StorageEngine;

fn main() {
    let mut engine = StorageEngine::new();
    engine.put(b"hello", b"world");
    println!("{:?}", engine.get(b"hello"));
}