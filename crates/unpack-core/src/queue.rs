//! Batch driver: expands inputs, runs the engine with bounded concurrency,
//! forwards events and aggregates statistics.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;

use crate::cancel::{CancelToken, PauseGate};
use crate::engine::Engine;
use crate::types::{Summary, TaskEvent, TaskOutcome, TaskStatus};

pub type Sink = Arc<dyn Fn(TaskEvent) + Send + Sync>;

/// Expand files/folders into a deduplicated, sorted file list. `exclude` is a
/// directory skipped during recursion (the run's output root) so a second run
/// does not walk over what the first one extracted.
pub fn expand_inputs(
    inputs: &[PathBuf],
    recursive: bool,
    exclude: Option<&Path>,
) -> Vec<PathBuf> {
    let excluded = exclude.and_then(|e| fs::canonicalize(e).ok());
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for p in inputs {
        collect(p, recursive, true, excluded.as_deref(), &mut out, &mut seen);
    }
    out.sort();
    out
}

fn collect(
    p: &PathBuf,
    recursive: bool,
    is_root: bool,
    excluded: Option<&Path>,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) {
    let meta = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return,
    };
    if meta.is_dir() {
        if !recursive {
            return;
        }
        if !is_root {
            if let Some(ex) = excluded {
                if fs::canonicalize(p).map(|c| c == ex).unwrap_or(false) {
                    return;
                }
            }
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with(crate::TMP_PREFIX) {
            return;
        }
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                collect(&e.path(), recursive, false, excluded, out, seen);
            }
        }
    } else {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with(crate::TMP_PREFIX) || name == ".DS_Store" {
            return;
        }
        if seen.insert(p.clone()) {
            out.push(p.clone());
        }
    }
}

/// A task outcome that should halt the rest of the batch when
/// `stop_on_error` is enabled.
fn is_fatal(status: Option<TaskStatus>) -> bool {
    matches!(
        status,
        Some(TaskStatus::Failed) | Some(TaskStatus::NeedsPassword)
    )
}

/// Run every file through the engine with at most `cfg.jobs` concurrent tasks.
///
/// * `stop_on_error` — after the first Failed / NeedsPassword outcome no new
///   task is started; the remaining files are reported as Cancelled. Tasks that
///   are already running finish naturally.
/// * A panicking task must never take the batch down: the join error is turned
///   into a Failed outcome (and counts towards `stop_on_error`).
/// * The processed-fingerprint store is flushed once the batch ends.
pub async fn run_batch(
    engine: Arc<Engine>,
    files: Vec<PathBuf>,
    cancel: CancelToken,
    gate: Arc<PauseGate>,
    sink: Sink,
) -> Summary {
    let started = Instant::now();
    let jobs = engine.cfg.jobs.max(1);
    let stop_on_error = engine.cfg.stop_on_error;
    let sem = Arc::new(Semaphore::new(jobs));
    let stopped = Arc::new(AtomicBool::new(false));

    let mut handles: Vec<(u64, PathBuf, tokio::task::JoinHandle<TaskOutcome>)> = Vec::new();
    for (idx, file) in files.into_iter().enumerate() {
        let id = (idx + 1) as u64;
        let size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        sink(TaskEvent::TaskQueued {
            id,
            path: file.clone(),
            size,
        });
        let engine = engine.clone();
        let cancel = cancel.clone();
        let gate = gate.clone();
        let sink = sink.clone();
        let sem = sem.clone();
        let stopped = stopped.clone();
        let task_path = file.clone();
        handles.push((
            id,
            file,
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await;
                // A previous task already failed: report this one as cancelled
                // without touching the archive.
                if stop_on_error && stopped.load(Ordering::SeqCst) {
                    let outcome = TaskOutcome {
                        task_id: id,
                        status: Some(TaskStatus::Cancelled),
                        path: task_path,
                        reason: Some("前序任务失败，已停止派发（stop_on_error）".to_string()),
                        ..Default::default()
                    };
                    sink(TaskEvent::TaskFinished {
                        id,
                        outcome: outcome.clone(),
                    });
                    return outcome;
                }
                let outcome = {
                    let mut emit = |e: TaskEvent| sink(e);
                    engine
                        .process_file(id, &task_path, &cancel, &gate, &mut emit)
                        .await
                };
                sink(TaskEvent::TaskFinished {
                    id,
                    outcome: outcome.clone(),
                });
                if stop_on_error && is_fatal(outcome.status) {
                    stopped.store(true, Ordering::SeqCst);
                }
                outcome
            }),
        ));
    }

    let mut outcomes: Vec<TaskOutcome> = Vec::with_capacity(handles.len());
    for (id, file, handle) in handles {
        match handle.await {
            Ok(outcome) => outcomes.push(outcome),
            Err(join_err) => {
                // The worker panicked (or was aborted): isolate it so the rest
                // of the batch and the summary stay correct.
                let reason = if join_err.is_panic() {
                    "任务内部错误（已隔离 panic），标记失败"
                } else {
                    "任务被中止"
                };
                let status = if join_err.is_panic() {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Cancelled
                };
                let outcome = TaskOutcome {
                    task_id: id,
                    status: Some(status),
                    path: file,
                    reason: Some(reason.to_string()),
                    ..Default::default()
                };
                sink(TaskEvent::TaskFinished {
                    id,
                    outcome: outcome.clone(),
                });
                if stop_on_error && is_fatal(outcome.status) {
                    stopped.store(true, Ordering::SeqCst);
                }
                outcomes.push(outcome);
            }
        }
    }

    // Flush the processed fingerprints once per batch.
    engine.save_processed().await;

    let mut summary = Summary::default();
    for o in &outcomes {
        summary.record(o);
    }
    summary.elapsed_ms = started.elapsed().as_millis() as u64;
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sevenzip::SevenZip;
    use crate::types::RunConfig;

    #[test]
    fn fatal_statuses() {
        assert!(is_fatal(Some(TaskStatus::Failed)));
        assert!(is_fatal(Some(TaskStatus::NeedsPassword)));
        assert!(!is_fatal(Some(TaskStatus::Success)));
        assert!(!is_fatal(Some(TaskStatus::Skipped)));
        assert!(!is_fatal(Some(TaskStatus::Cancelled)));
        assert!(!is_fatal(None));
    }

    #[tokio::test]
    async fn stop_on_error_marks_remaining_cancelled() {
        let dir = std::env::temp_dir()
            .join(format!("paranoia-queue-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        let sz = SevenZip::new(PathBuf::from("/definitely/not/7z-exe"));
        let mut cfg = RunConfig::default();
        cfg.jobs = 1;
        cfg.stop_on_error = true;
        cfg.skip_processed = false;
        cfg.nested = false;
        cfg.output_root = Some(dir.join("out"));
        cfg.processed_path = Some(dir.join("processed.json"));
        let engine = Arc::new(Engine::new(sz, cfg));
        // Nonexistent inputs fail at the detect step (metadata error), which is
        // enough to exercise the stop-on-error path without a 7-Zip binary.
        let files = vec![
            dir.join("a.enc"),
            dir.join("b.enc"),
            dir.join("c.enc"),
        ];
        let sink: Sink = Arc::new(|_: TaskEvent| {});
        let summary = run_batch(
            engine,
            files,
            CancelToken::new(),
            Arc::new(PauseGate::new()),
            sink,
        )
        .await;
        assert_eq!(summary.total, 3);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.cancelled, 2);
        assert_eq!(summary.success, 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
