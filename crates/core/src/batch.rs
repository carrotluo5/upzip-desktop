//! Batch decryption: one password and output directory for a list of files, with per-file
//! isolation so a single failure never aborts the remaining files.

use std::path::PathBuf;

use crate::decrypt::{decrypt_file, DecryptOptions};
use crate::error::DecryptError;
use crate::output::{resolve_output_dir, ConflictStrategy};

#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub files: Vec<PathBuf>,
    pub password: String,
    /// Explicit output directory; \`None\` means "first file's parent directory".
    pub output_dir: Option<PathBuf>,
    pub conflict: ConflictStrategy,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOutcome {
    pub input: PathBuf,
    pub outputs: Vec<PathBuf>,
    pub original_name: Option<String>,
    pub bytes_written: u64,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
}

impl FileOutcome {
    pub fn is_ok(&self) -> bool {
        self.error_kind.is_none()
    }

    fn ok(input: PathBuf, report: crate::DecryptReport) -> Self {
        Self {
            input,
            outputs: report.outputs,
            original_name: Some(report.original_name),
            bytes_written: report.bytes_written,
            error_kind: None,
            error_message: None,
        }
    }

    fn err(input: PathBuf, error: &DecryptError) -> Self {
        Self {
            input,
            outputs: Vec::new(),
            original_name: None,
            bytes_written: 0,
            error_kind: Some(error.kind().to_string()),
            error_message: Some(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchProgress {
    pub index: usize,
    pub total: usize,
    pub current_file: PathBuf,
    pub file_processed: u64,
    pub file_total: u64,
    pub overall_processed: u64,
    pub overall_total: u64,
    pub successes: usize,
    pub failures: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub outcomes: Vec<FileOutcome>,
    pub successes: usize,
    pub failures: usize,
    pub output_dir: PathBuf,
}

fn ciphertext_budget(path: &PathBuf) -> u64 {
    std::fs::metadata(path)
        .map(|meta| meta.len().saturating_sub(72).max(1))
        .unwrap_or(1)
}

pub fn run_batch(
    options: &BatchOptions,
    progress: &mut dyn FnMut(BatchProgress),
) -> BatchSummary {
    let total = options.files.len();
    let output_dir = resolve_output_dir(&options.files, options.output_dir.as_deref())
        .unwrap_or_else(|| PathBuf::from("."));

    let file_budgets: Vec<u64> = options.files.iter().map(ciphertext_budget).collect();
    let total_bytes: u64 = file_budgets.iter().sum();

    let mut outcomes: Vec<FileOutcome> = Vec::with_capacity(total);
    let mut successes = 0usize;
    let mut failures = 0usize;
    let mut overall_done: u64 = 0;

    if total > 0 {
        progress(BatchProgress {
            index: 0,
            total,
            current_file: options.files[0].clone(),
            file_processed: 0,
            file_total: file_budgets[0],
            overall_processed: 0,
            overall_total: total_bytes.max(1),
            successes,
            failures,
        });
    }

    for (index, input) in options.files.iter().enumerate() {
        let file_total = file_budgets[index];
        let base = overall_done;
        let outcome = {
            let mut on_progress = |processed: u64, _total: u64| {
                progress(BatchProgress {
                    index,
                    total,
                    current_file: input.clone(),
                    file_processed: processed,
                    file_total,
                    overall_processed: base + processed,
                    overall_total: total_bytes.max(1),
                    successes,
                    failures,
                });
            };
            let decrypt_options = DecryptOptions {
                output_dir: output_dir.clone(),
                conflict: options.conflict,
            };
            match decrypt_file(input, &options.password, &decrypt_options, &mut on_progress) {
                Ok(report) => FileOutcome::ok(input.clone(), report),
                Err(error) => FileOutcome::err(input.clone(), &error),
            }
        };

        if outcome.is_ok() {
            successes += 1;
        } else {
            failures += 1;
        }
        outcomes.push(outcome);
        overall_done += file_total;

        progress(BatchProgress {
            index,
            total,
            current_file: input.clone(),
            file_processed: file_total,
            file_total,
            overall_processed: overall_done.min(total_bytes.max(1)),
            overall_total: total_bytes.max(1),
            successes,
            failures,
        });
    }

    BatchSummary {
        outcomes,
        successes,
        failures,
        output_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{make_enc_file, test_salt};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "unparanoia-enc-test-batch-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn batch_isolates_failures_and_uses_first_file_parent() {
        let dir = temp_dir("isolation");
        let sub = dir.join("inputs");
        fs::create_dir_all(&sub).unwrap();
        let a = sub.join("a.enc");
        let b = sub.join("b.enc");
        let bad = sub.join("bad.enc");
        fs::write(&a, make_enc_file("123", &test_salt(20), 0x11, &[("a.txt", b"alpha")])).unwrap();
        fs::write(&b, make_enc_file("123", &test_salt(21), 0x11, &[("b.txt", b"beta")])).unwrap();
        fs::write(&bad, make_enc_file("999", &test_salt(22), 0x11, &[("bad.txt", b"nope")])).unwrap();

        let options = BatchOptions {
            files: vec![a, b, bad],
            password: "123".to_string(),
            output_dir: None,
            conflict: ConflictStrategy::Rename,
        };
        let mut progress_events: Vec<BatchProgress> = Vec::new();
        let summary = run_batch(&options, &mut |progress| progress_events.push(progress));

        assert_eq!(summary.successes, 2);
        assert_eq!(summary.failures, 1);
        assert_eq!(summary.output_dir, sub);
        assert_eq!(summary.outcomes.len(), 3);
        assert_eq!(summary.outcomes[0].error_kind, None);
        assert_eq!(summary.outcomes[0].original_name.as_deref(), Some("a.txt"));
        assert_eq!(summary.outcomes[1].original_name.as_deref(), Some("b.txt"));
        assert_eq!(summary.outcomes[2].error_kind.as_deref(), Some("wrongPassword"));
        assert!(summary.outcomes[2].outputs.is_empty());

        // Default output directory = directory of the first file.
        assert_eq!(fs::read(sub.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(sub.join("b.txt")).unwrap(), b"beta");
        assert!(!sub.join("bad.txt").exists());

        // Progress must cover every file and never go backwards.
        assert!(progress_events.len() >= 3);
        let mut last = 0u64;
        for event in &progress_events {
            assert_eq!(event.total, 3);
            assert!(event.overall_processed >= last);
            last = event.overall_processed;
        }
        let final_event = progress_events.last().unwrap();
        assert_eq!(final_event.successes, 2);
        assert_eq!(final_event.failures, 1);
        assert_eq!(final_event.overall_processed, final_event.overall_total);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn custom_output_dir_is_used_for_all_files() {
        let dir = temp_dir("custom-out");
        let a = dir.join("a.enc");
        let b = dir.join("b.enc");
        fs::write(&a, make_enc_file("pw", &test_salt(30), 0x11, &[("a.txt", b"a")])).unwrap();
        fs::write(&b, make_enc_file("pw", &test_salt(31), 0x11, &[("b.txt", b"b")])).unwrap();
        let out = dir.join("collected");
        let options = BatchOptions {
            files: vec![a, b],
            password: "pw".to_string(),
            output_dir: Some(out.clone()),
            conflict: ConflictStrategy::Rename,
        };
        let summary = run_batch(&options, &mut |_| {});
        assert_eq!(summary.successes, 2);
        assert_eq!(summary.output_dir, out);
        assert!(out.join("a.txt").is_file());
        assert!(out.join("b.txt").is_file());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_batch_is_a_noop() {
        let options = BatchOptions {
            files: Vec::new(),
            password: "pw".to_string(),
            output_dir: None,
            conflict: ConflictStrategy::Rename,
        };
        let summary = run_batch(&options, &mut |_| {});
        assert_eq!(summary.successes, 0);
        assert_eq!(summary.failures, 0);
        assert_eq!(summary.output_dir, PathBuf::from("."));
    }
}
