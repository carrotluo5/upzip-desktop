//! Extraction of the decrypted inner ZIP archive into the output directory.
//!
//! Every entry is written to a temporary file in the destination directory first, its CRC32 is
//! verified by the zip crate and only then the file is renamed into place. A failure therefore
//! never leaves a half-written output file behind.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::error::DecryptError;
use crate::output::{resolve_conflict_path, ConflictStrategy};
use crate::util;

pub(crate) struct ExtractReport {
    pub outputs: Vec<PathBuf>,
    pub original_name: String,
    pub bytes_written: u64,
}

pub(crate) fn extract_zip(
    zip_path: &Path,
    out_dir: &Path,
    conflict: ConflictStrategy,
) -> Result<ExtractReport, DecryptError> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| DecryptError::Zip(e.to_string()))?;
    if archive.len() == 0 {
        return Err(DecryptError::Zip("archive is empty".to_string()));
    }

    let mut outputs: Vec<PathBuf> = Vec::new();
    let mut bytes_written: u64 = 0;
    let mut first_rel: Option<PathBuf> = None;

    for index in 0..archive.len() {
        // The borrow of the archive must end before the next iteration, so keep this body short.
        let mut entry = archive
            .by_index(index)
            .map_err(|e| DecryptError::Zip(e.to_string()))?;
        let raw_name = entry
            .name()
            .map_err(|e| DecryptError::Zip(e.to_string()))?
            .to_string();
        if raw_name.is_empty() {
            continue;
        }
        let rel = match entry.enclosed_name() {
            Some(path) => path,
            None => return Err(DecryptError::InvalidOutputName(raw_name)),
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        if entry.is_dir() {
            fs::create_dir_all(out_dir.join(&rel))?;
            continue;
        }
        // Only restore regular files; symlinks / devices in the archive are rejected.
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type != 0 && file_type != 0o100000 {
                return Err(DecryptError::Zip(format!(
                    "unsupported entry type in archive: {raw_name}"
                )));
            }
        }

        let target = out_dir.join(&rel);
        let final_path = resolve_conflict_path(&target, conflict)?;
        let parent = final_path
            .parent()
            .ok_or_else(|| DecryptError::InvalidOutputName(raw_name.clone()))?
            .to_path_buf();
        fs::create_dir_all(&parent)?;

        let expected_size = entry.size();
        let temp = util::unique_temp_path(&parent, ".upzip-enc-extract-");
        let guard = util::TempGuard::new(temp.clone());
        {
            let mut writer = BufWriter::new(File::create(&temp)?);
            // io::copy reads to EOF, which is where the zip crate verifies the CRC32.
            io::copy(&mut entry, &mut writer)
                .map_err(|e| DecryptError::Zip(format!("cannot extract {raw_name}: {e}")))?;
            writer.flush()?;
        }
        let written = fs::metadata(&temp)?.len();
        if written != expected_size {
            return Err(DecryptError::Zip(format!(
                "size mismatch for {raw_name}: expected {expected_size}, got {written}"
            )));
        }
        util::install_temp_file(&temp, &final_path)?;
        guard.keep();
        if first_rel.is_none() {
            first_rel = Some(rel.clone());
        }
        outputs.push(final_path);
        bytes_written += written;
    }

    if outputs.is_empty() {
        return Err(DecryptError::Zip(
            "archive does not contain any regular file".to_string(),
        ));
    }

    let original_name = recovered_original_name(first_rel.as_deref(), outputs.len());
    Ok(ExtractReport {
        outputs,
        original_name,
        bytes_written,
    })
}

/// Recover a human readable original name from the archive entry path: for a single-file
/// archive that is the file name, for a directory archive the top-level directory name.
/// Deliberately independent of conflict-renamed output paths.
fn recovered_original_name(first_rel: Option<&Path>, output_count: usize) -> String {
    let Some(rel) = first_rel else {
        return String::new();
    };
    if output_count == 1 {
        if let Some(name) = rel.file_name() {
            return name.to_string_lossy().to_string();
        }
    }
    if let Some(first) = rel.components().next() {
        let name = first.as_os_str().to_string_lossy().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    String::new()
}

/// Extract the single regular-file entry of `zip_path` directly to `dest_path`
/// without spawning an external tool. Used by the batch engine to unwrap
/// SSEFE-encrypted split volumes in-process.
pub fn extract_single_entry_to_path(zip_path: &Path, dest_path: &Path) -> Result<u64, DecryptError> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| DecryptError::Zip(e.to_string()))?;
    if archive.len() != 1 {
        return Err(DecryptError::Zip(format!(
            "expected 1 entry in archive, found {}",
            archive.len()
        )));
    }
    let mut entry = archive
        .by_index(0)
        .map_err(|e| DecryptError::Zip(e.to_string()))?;
    if entry.is_dir() {
        return Err(DecryptError::Zip(
            "expected a regular file entry, found directory".to_string(),
        ));
    }
    if let Some(parent) = dest_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let expected_size = entry.size();
    let parent = dest_path.parent().unwrap_or_else(|| Path::new("."));
    let temp = util::unique_temp_path(parent, ".upzip-enc-unwrap-");
    let guard = util::TempGuard::new(temp.clone());
    {
        let mut writer = BufWriter::with_capacity(1 << 20, File::create(&temp)?);
        io::copy(&mut entry, &mut writer)
            .map_err(|e| DecryptError::Zip(format!("cannot unwrap entry: {e}")))?;
        writer.flush()?;
    }
    let written = fs::metadata(&temp)?.len();
    if written != expected_size {
        return Err(DecryptError::Zip(format!(
            "size mismatch: expected {expected_size}, got {written}"
        )));
    }
    util::install_temp_file(&temp, dest_path)?;
    guard.keep();
    Ok(written)
}
