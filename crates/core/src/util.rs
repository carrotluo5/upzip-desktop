//! Small internal helpers: unique temp paths, RAII cleanup, checkcode validation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::DecryptError;
use crate::format::CHECKCODE_LEN;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a unique file name inside `dir` that does not exist yet.
pub fn unique_temp_path(dir: &Path, prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!("{prefix}{pid}-{nanos:x}-{n}.tmp"));
        if !candidate.exists() {
            return candidate;
        }
    }
}

/// Deletes the wrapped file on drop unless `keep()` was called.
pub struct TempGuard {
    path: PathBuf,
    keep: bool,
}

impl TempGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    pub fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The decrypted stream must start with 32 printable lowercase/digit characters; if it does not,
/// the password is wrong (the probability of a false positive is 256^-32).
pub fn is_valid_checkcode(buf: &[u8]) -> bool {
    buf.len() >= CHECKCODE_LEN
        && buf[..CHECKCODE_LEN]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Install a fully written temp file at its final path, replacing an existing file when allowed.
pub fn install_temp_file(temp: &Path, target: &Path) -> Result<(), DecryptError> {
    if target.exists() && target.is_dir() {
        return Err(DecryptError::Crypto(format!(
            "output path is a directory: {}",
            target.display()
        )));
    }
    #[cfg(windows)]
    if target.exists() {
        fs::remove_file(target)?;
    }
    fs::rename(temp, target)?;
    Ok(())
}
