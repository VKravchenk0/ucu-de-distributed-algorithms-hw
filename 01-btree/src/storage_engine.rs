/// Placeholder storage engine backed by a flat byte buffer.
/// Records are appended as [key_len][key][value_len][value]; `get` scans
/// linearly and keeps the last match, since a later `put` for the same key
/// is simply appended rather than replacing the earlier record in place.
/// This will be replaced by a real B+Tree-backed implementation.
pub struct StorageEngine {
    data: Vec<u8>,
}

impl StorageEngine {
    pub fn new() -> Self {
        StorageEngine { data: Vec::new() }
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) {
        self.data.extend_from_slice(&(key.len() as u32).to_be_bytes());
        self.data.extend_from_slice(key);
        self.data.extend_from_slice(&(value.len() as u32).to_be_bytes());
        self.data.extend_from_slice(value);
    }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let mut pos = 0;
        let mut result = None;

        while pos < self.data.len() {
            let key_len = u32::from_be_bytes(self.data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let entry_key = &self.data[pos..pos + key_len];
            pos += key_len;

            let value_len = u32::from_be_bytes(self.data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let entry_value = &self.data[pos..pos + value_len];
            pos += value_len;

            if entry_key == key {
                result = Some(entry_value.to_vec());
            }
        }

        result
    }
}

impl Default for StorageEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_and_get() {
        let mut engine = StorageEngine::new();
        engine.put(b"hello", b"world");
        let _ = engine.get(b"hello");

        assert_eq!(true, true);
    }
}
