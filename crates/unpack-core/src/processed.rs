//! Persistent "already processed" store: fingerprint -> output path.
//!
//! Plain-text JSON (no secrets) named `processed.json` in the same configuration
//! directory the Paranoia core uses. Read/write failures are non-fatal — a corrupt or
//! unreadable store behaves like an empty one so extraction never blocks on bookkeeping.
//!
//! The GUI's "forget processed" action calls `forget` / `remove_all`, which deletes the
//! file and clears memory, returning the number of records that were dropped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const PROCESSED_FILE_NAME: &str = "processed.json";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessedRecord {
    /// Where the archive landed.
    pub path: PathBuf,
    /// Completion time (Unix epoch milliseconds).
    #[serde(rename = "finishedAtMs")]
    pub finished_at_ms: u64,
    /// `success` (only successful extractions are recorded for now).
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProcessedFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    records: BTreeMap<String, ProcessedRecord>,
}

impl Default for ProcessedFile {
    fn default() -> Self {
        Self {
            version: 1,
            records: BTreeMap::new(),
        }
    }
}

pub struct ProcessedStore {
    path: PathBuf,
    records: BTreeMap<String, ProcessedRecord>,
}

impl ProcessedStore {
    /// `<config dir>/processed.json`, with a temp-dir fallback when the platform
    /// configuration directory is unavailable.
    pub fn default_path() -> PathBuf {
        match unparanoia_core::config::config_dir() {
            Ok(dir) => dir.join(PROCESSED_FILE_NAME),
            Err(_) => std::env::temp_dir().join(format!("unparanoia-{PROCESSED_FILE_NAME}")),
        }
    }

    /// Load the store at `path` (the file is created by `save`).
    pub fn load(path: PathBuf) -> Self {
        let records = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<ProcessedFile>(&bytes) {
                Ok(file) => file.records,
                Err(_) => BTreeMap::new(),
            },
            Err(_) => BTreeMap::new(),
        };
        Self { path, records }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    pub fn contains(&self, fingerprint: &str) -> bool {
        self.records.contains_key(fingerprint)
    }

    pub fn get(&self, fingerprint: &str) -> Option<&ProcessedRecord> {
        self.records.get(fingerprint)
    }

    /// Record a finished extraction. The caller is responsible for `save`.
    pub fn insert(&mut self, fingerprint: &str, path: &Path, status: &str) {
        self.records.insert(
            fingerprint.to_string(),
            ProcessedRecord {
                path: path.to_path_buf(),
                finished_at_ms: now_ms(),
                status: status.to_string(),
            },
        );
    }

    /// Atomically persist the store. Errors are returned so callers can log them, but
    /// callers must not fail the extraction batch because of bookkeeping.
    pub fn save(&self) -> Result<(), String> {
        let payload = ProcessedFile {
            version: 1,
            records: self.records.clone(),
        };
        let json =
            serde_json::to_vec_pretty(&payload).map_err(|e| format!("序列化处理记录失败：{e}"))?;
        crate::util::write_file_atomic(&self.path, &json)
            .map_err(|e| format!("写入处理记录失败：{e}"))
    }

    /// Delete the backing file and clear memory; returns the number of dropped records.
    /// Used by the GUI "forget processed" action.
    pub fn forget(&mut self) -> usize {
        let dropped = self.records.len();
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "warn: cannot delete processed store {}: {error}",
                    self.path.display()
                );
            }
        }
        self.records.clear();
        dropped
    }

    /// Alias of `forget` for callers that phrase it as "remove all".
    pub fn remove_all(&mut self) -> usize {
        self.forget()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "paranoia-unpack-processed-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_and_forget() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("processed.json");
        let mut store = ProcessedStore::load(path.clone());
        assert_eq!(store.count(), 0);
        assert!(!store.contains("abc"));
        store.insert("abc", &dir.join("out"), "success");
        store.insert("def", &dir.join("out2"), "success");
        assert_eq!(store.count(), 2);
        assert!(store.contains("abc"));
        let rec = store.get("abc").unwrap();
        assert_eq!(rec.status, "success");
        assert_eq!(rec.path, dir.join("out"));
        assert!(rec.finished_at_ms > 0);
        store.save().unwrap();

        let reloaded = ProcessedStore::load(path.clone());
        assert_eq!(reloaded.count(), 2);
        assert!(reloaded.contains("def"));
        // Plain JSON on disk (no encryption) with camelCase timestamp.
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("finishedAtMs"));
        assert!(raw.contains("\"abc\""));

        let mut reloaded = reloaded;
        assert_eq!(reloaded.forget(), 2);
        assert_eq!(reloaded.count(), 0);
        assert!(!path.exists());
        // Forget is idempotent.
        assert_eq!(reloaded.remove_all(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_or_missing_file_reads_as_empty() {
        let dir = temp_dir("corrupt");
        let path = dir.join("processed.json");
        assert_eq!(ProcessedStore::load(path.clone()).count(), 0);
        fs::write(&path, b"{not json").unwrap();
        let store = ProcessedStore::load(path.clone());
        assert_eq!(store.count(), 0);
        assert_eq!(store.path(), path.as_path());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_is_atomic_and_creates_parent_dirs() {
        let dir = temp_dir("atomic");
        let path = dir.join("nested").join("processed.json");
        let mut store = ProcessedStore::load(path.clone());
        store.insert("fp", &dir, "success");
        store.save().unwrap();
        assert!(path.exists());
        // No leftover temp file next to the store.
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
