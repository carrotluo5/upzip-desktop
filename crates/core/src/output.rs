//! Output directory defaults, conflict strategies and target-path resolution.

use std::path::{Path, PathBuf};

use crate::error::DecryptError;

/// What to do when the target output file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictStrategy {
    /// Replace the existing file.
    Overwrite,
    /// Append " (1)", " (2)", ... before the extension.
    Rename,
    /// Fail the file with an \`outputExists\` error and write nothing.
    Error,
}

impl Default for ConflictStrategy {
    fn default() -> Self {
        ConflictStrategy::Rename
    }
}

/// Default output directory for a batch: the parent directory of the first file.
pub fn default_output_dir(files: &[PathBuf]) -> Option<PathBuf> {
    files.first().map(|file| match file.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    })
}

/// Effective output directory: explicit custom directory, otherwise the default.
pub fn resolve_output_dir(files: &[PathBuf], custom: Option<&Path>) -> Option<PathBuf> {
    custom
        .map(Path::to_path_buf)
        .or_else(|| default_output_dir(files))
}

/// Resolve a desired output path according to the batch-wide conflict strategy.
///
/// For \`Rename\` a non-existing candidate is returned; for \`Error\` an existing target yields
/// \`DecryptError::OutputExists\`; for \`Overwrite\` the target is returned as-is.
pub fn resolve_conflict_path(
    target: &Path,
    strategy: ConflictStrategy,
) -> Result<PathBuf, DecryptError> {
    match strategy {
        ConflictStrategy::Overwrite => Ok(target.to_path_buf()),
        ConflictStrategy::Error => {
            if target.exists() {
                Err(DecryptError::OutputExists(target.to_path_buf()))
            } else {
                Ok(target.to_path_buf())
            }
        }
        ConflictStrategy::Rename => Ok(rename_candidate(target)),
    }
}

fn rename_candidate(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = target.extension().map(|s| s.to_string_lossy().to_string());
    for n in 1..10_000u32 {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = target.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match ext {
        Some(ext) => target.with_file_name(format!("{stem}.{ts}.{ext}")),
        None => target.with_file_name(format!("{stem}.{ts}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "upzip-enc-test-out-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_output_dir_is_first_file_parent() {
        let dir = temp_dir("default");
        let a = dir.join("a.enc");
        let b = dir.join("sub").join("b.enc");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(&a, b"x").unwrap();
        fs::write(&b, b"x").unwrap();
        assert_eq!(default_output_dir(&[a.clone(), b.clone()]), Some(dir.clone()));
        assert_eq!(default_output_dir(&[]), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn custom_output_dir_wins() {
        let dir = temp_dir("custom");
        let a = dir.join("a.enc");
        fs::write(&a, b"x").unwrap();
        let custom = dir.join("output");
        assert_eq!(
            resolve_output_dir(&[a.clone()], Some(&custom)),
            Some(custom.clone())
        );
        assert_eq!(
            resolve_output_dir(&[a.clone()], None),
            Some(dir.clone())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_conflict_appends_counter() {
        let dir = temp_dir("conflict");
        let target = dir.join("3138.7z.002");
        fs::write(&target, b"existing").unwrap();
        let resolved = resolve_conflict_path(&target, ConflictStrategy::Rename).unwrap();
        assert_eq!(resolved, dir.join("3138.7z (1).002"));
        fs::write(&resolved, b"second").unwrap();
        let resolved2 = resolve_conflict_path(&target, ConflictStrategy::Rename).unwrap();
        assert_eq!(resolved2, dir.join("3138.7z (2).002"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn error_strategy_rejects_existing_target() {
        let dir = temp_dir("error");
        let target = dir.join("out.bin");
        fs::write(&target, b"existing").unwrap();
        let err = resolve_conflict_path(&target, ConflictStrategy::Error).unwrap_err();
        assert_eq!(err.kind(), "outputExists");
        assert!(resolve_conflict_path(&dir.join("fresh.bin"), ConflictStrategy::Error).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overwrite_strategy_returns_target() {
        let dir = temp_dir("overwrite");
        let target = dir.join("out.bin");
        fs::write(&target, b"existing").unwrap();
        assert_eq!(
            resolve_conflict_path(&target, ConflictStrategy::Overwrite).unwrap(),
            target
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
