//! Small filesystem / naming helpers shared across the engine.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{io_ctx, Result};

/// Bytes sampled from head and tail of a file to build its fingerprint.
pub const SAMPLE_CHUNK: u64 = 8 * 1024 * 1024;

/// Read up to `buf.len()` bytes at absolute offset; returns bytes actually read.
pub fn read_at(f: &mut File, off: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    f.seek(SeekFrom::Start(off))?;
    let mut n = 0usize;
    while n < buf.len() {
        let r = f.read(&mut buf[n..])?;
        if r == 0 {
            break;
        }
        n += r;
    }
    Ok(n)
}

/// Size + head/tail sample hashed with SHA-256. Cheap and stable across renames.
pub fn fingerprint(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).map_err(|e| crate::error::UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let size = meta.len();
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());
    hasher.update(
        path.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
            .as_bytes(),
    );
    let mut f = File::open(path).map_err(|e| crate::error::UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let head_len = SAMPLE_CHUNK.min(size) as usize;
    if head_len > 0 {
        let mut buf = vec![0u8; head_len];
        let n = read_at(&mut f, 0, &mut buf)?;
        hasher.update(&buf[..n]);
    }
    if size > SAMPLE_CHUNK.saturating_mul(2) {
        let tail_len = SAMPLE_CHUNK as usize;
        let mut buf = vec![0u8; tail_len];
        let n = read_at(&mut f, size - tail_len as u64, &mut buf)?;
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Make a string safe as a single path component on Windows *and* macOS.
pub fn sanitize_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    // Windows: no trailing dots / spaces.
    while out.ends_with(' ') || out.ends_with('.') {
        out.pop();
    }
    let trimmed = out.trim_start();
    if trimmed.is_empty() {
        return "archive".to_string();
    }
    let mut s = trimmed.to_string();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let upper = s.to_ascii_uppercase();
    if RESERVED.contains(&upper.as_str()) {
        s = format!("_{s}");
    }
    if s.chars().count() > 150 {
        s = s.chars().take(150).collect();
    }
    s
}

/// Free bytes on the volume that holds `path` (None when not determinable).
#[cfg(unix)]
pub fn free_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) == 0 {
            Some(st.f_bavail as u64 * st.f_frsize as u64)
        } else {
            None
        }
    }
}

#[cfg(not(unix))]
pub fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// Recursively remove a file or directory; missing paths are ignored.
pub fn remove_path(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(crate::error::UpError::Io {
                path: path.to_path_buf(),
                source: e,
            })
        }
    };
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(|e| crate::error::UpError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    } else {
        fs::remove_file(path).map_err(|e| crate::error::UpError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}

/// Move a path, falling back to copy + delete across devices.
pub fn move_path(src: &Path, dst: &Path) -> Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_recursive(src, dst)?;
            remove_path(src)
        }
    }
}

/// Copy file or directory tree; returns (files, bytes) copied.
pub fn copy_recursive(src: &Path, dst: &Path) -> Result<(u64, u64)> {
    let meta = fs::symlink_metadata(src).map_err(|e| crate::error::UpError::Io {
        path: src.to_path_buf(),
        source: e,
    })?;
    if meta.is_dir() {
        fs::create_dir_all(dst).map_err(|e| crate::error::UpError::Io {
            path: dst.to_path_buf(),
            source: e,
        })?;
        let mut files = 0u64;
        let mut bytes = 0u64;
        for entry in fs::read_dir(src).map_err(|e| crate::error::UpError::Io {
            path: src.to_path_buf(),
            source: e,
        })? {
            let entry = entry.map_err(|e| crate::error::UpError::Io {
                path: src.to_path_buf(),
                source: e,
            })?;
            let (f, b) = copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
            files += f;
            bytes += b;
        }
        Ok((files, bytes))
    } else {
        let mut in_f = File::open(src).map_err(|e| crate::error::UpError::Io {
            path: src.to_path_buf(),
            source: e,
        })?;
        let mut out_f = File::create(dst).map_err(|e| crate::error::UpError::Io {
            path: dst.to_path_buf(),
            source: e,
        })?;
        let n = std::io::copy(&mut in_f, &mut out_f).map_err(|e| crate::error::UpError::Io {
            path: dst.to_path_buf(),
            source: e,
        })?;
        out_f.flush().ok();
        Ok((1, n))
    }
}

/// Count files + bytes under a path.
pub fn count_tree(path: &Path) -> (u64, u64) {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return (0, 0),
    };
    if meta.is_dir() {
        let mut files = 0u64;
        let mut bytes = 0u64;
        if let Ok(rd) = fs::read_dir(path) {
            for e in rd.flatten() {
                let (f, b) = count_tree(&e.path());
                files += f;
                bytes += b;
            }
        }
        (files, bytes)
    } else {
        (1, meta.len())
    }
}

pub fn write_file(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| crate::error::UpError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let mut f = File::create(path).map_err(|e| crate::error::UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    f.write_all(content).map_err(|e| crate::error::UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Write bytes atomically: same-directory temp file, fsync, rename (0600 on unix).
/// A crash mid-write leaves the previous file intact instead of a truncated one —
/// which matters for `settings.json` and `processed.json`.
pub fn write_file_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| crate::error::UpError::Io {
        path: parent.to_path_buf(),
        source: e,
    })?;
    let stem = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp = parent.join(format!(".{stem}.tmp-{}", uuid::Uuid::new_v4().simple()));

    let write = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    })();
    if let Err(e) = write {
        let _ = fs::remove_file(&tmp);
        return Err(crate::error::UpError::Io {
            path: path.to_path_buf(),
            source: e,
        });
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(crate::error::UpError::Io {
            path: path.to_path_buf(),
            source: e,
        });
    }
    Ok(())
}

pub fn read_to_string(path: &Path) -> Result<String> {
    let mut s = String::new();
    let mut f = File::open(path).map_err(|e| crate::error::UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    f.read_to_string(&mut s)
        .map_err(|e| crate::error::UpError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    Ok(s)
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn data_dir() -> PathBuf {
    for var in ["PARANOIA_DATA_DIR", "UPZIP_DATA_DIR"] {
        if let Ok(p) = std::env::var(var) {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("unparanoia")
}

/// Guard for `std::fs` calls that carry a path for context.
pub fn fs_err<T>(path: &Path, r: std::io::Result<T>) -> Result<T> {
    io_ctx(path, r)
}
