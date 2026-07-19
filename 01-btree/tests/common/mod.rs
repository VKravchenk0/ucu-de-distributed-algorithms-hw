use std::path::{Path, PathBuf};

/// A scratch file path, removed on drop. Hand-rolled rather than pulled
/// from a crate, matching the project's zero-core-dependency policy.
pub struct TempPath(PathBuf);

impl TempPath {
    pub fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "hw_btree_test_{name}_{}_{nanos}.db",
            std::process::id()
        ));
        TempPath(path)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
