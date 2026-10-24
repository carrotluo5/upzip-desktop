//! End-to-end pipeline for a single archive:
//! detect -> (SSEFE decrypt) -> password -> re-list -> plan -> extract into staging
//! -> security scrub -> nested cascade -> land -> record.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use regex::Regex;
use uuid::Uuid;

use crate::cancel::{wait_pause_or_cancel, CancelToken, PauseGate};
use crate::detect;
use crate::error::{Result, UpError};
use crate::layout::{self, MoveStats};
use crate::processed::ProcessedStore;
use crate::sevenzip::{PwCheck, SevenZip};
use crate::ssefe::{self, SsefeError};
use crate::types::*;
use crate::util::{fingerprint, free_space, human_bytes, remove_path};

/// Extensions that are ZIP containers but should not be expanded by accident.
pub const DOC_EXTS: &[&str] = &[
    "docx", "docm", "xlsx", "xlsm", "pptx", "pptm", "odt", "ods", "odp", "jar", "apk", "epub",
    "ipa", "vsix", "nupkg", "whl", "xpi",
];

/// A password that was accepted for the current archive / cascade layer.
#[derive(Clone, Debug)]
struct PasswordHit {
    /// `config` (RunConfig.passwords), `hint` (archive comment) or `empty`.
    source: String,
    password: String,
    /// 1-based position in the candidate pool (drives `TaskPasswordAttempt.tried`
    /// and the "使用候选密码 #N 成功" warning).
    tried: usize,
}

/// Build the ordered, de-duplicated password candidate pool for one archive
/// layer: `RunConfig.passwords` (source `config`) -> archive comment `hint`s ->
/// the empty password (source `empty`, only when `try_empty_password`).
///
/// Every cascade layer rebuilds the pool from scratch, so an inner archive whose
/// password differs from the outer one can still be unlocked.
fn password_candidates(cfg: &RunConfig, det: &Detection) -> Vec<PasswordHit> {
    let mut cands: Vec<PasswordHit> = Vec::new();
    for password in &cfg.passwords {
        cands.push(PasswordHit {
            source: "config".to_string(),
            password: password.clone(),
            tried: 0,
        });
    }
    for hint in &det.hints {
        cands.push(PasswordHit {
            source: "hint".to_string(),
            password: hint.clone(),
            tried: 0,
        });
    }
    if cfg.try_empty_password {
        cands.push(PasswordHit {
            source: "empty".to_string(),
            password: String::new(),
            tried: 0,
        });
    }
    let mut seen: HashSet<String> = HashSet::new();
    // Empty strings are never valid explicit passwords; the empty candidate is
    // only injected above when explicitly requested.
    cands.retain(|c| !c.password.is_empty() || c.source == "empty");
    cands.retain(|c| seen.insert(c.password.clone()));
    for (index, cand) in cands.iter_mut().enumerate() {
        cand.tried = index + 1;
    }
    cands
}

/// Candidates strictly after `tried_through`, i.e. the ones not attempted yet.
fn candidates_after(pool: &[PasswordHit], tried_through: usize) -> Vec<PasswordHit> {
    pool.iter()
        .filter(|candidate| candidate.tried > tried_through)
        .cloned()
        .collect()
}

/// A 7-Zip password rejection is retryable with the next candidate; every other
/// extraction error is final.
fn is_retryable_rejection(error: &UpError) -> bool {
    matches!(error, UpError::PasswordRequired)
}

/// Only a wrong SSEFE password is retryable; integrity/format errors are final.
fn is_retryable_ssefe(error: &SsefeError) -> bool {
    matches!(error, SsefeError::WrongPassword)
}

/// Warning appended to a successful outcome when the first candidate did not work.
fn non_first_candidate_warning(hit: &PasswordHit) -> Option<String> {
    (hit.tried > 1).then(|| format!("使用候选密码 #{} 成功", hit.tried))
}

pub struct Engine {
    pub sz: SevenZip,
    pub cfg: RunConfig,
    /// Fingerprints of already processed archives (`processed.json`). Loaded at
    /// construction, mutated in `process_file`, flushed by `run_batch`.
    processed: Mutex<ProcessedStore>,
}

/// Removes a path when dropped, unless `keep()` was called — temp files never leak.
struct TempGuard(Option<PathBuf>);

impl TempGuard {
    fn set(&mut self, p: PathBuf) {
        self.0 = Some(p);
    }
    fn keep(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = remove_path(p);
        }
    }
}

impl Engine {
    pub fn new(sz: SevenZip, cfg: RunConfig) -> Self {
        let path = cfg
            .processed_path
            .clone()
            .unwrap_or_else(ProcessedStore::default_path);
        Self {
            sz,
            processed: Mutex::new(ProcessedStore::load(path)),
            cfg,
        }
    }

    fn processed_lock(&self) -> std::sync::MutexGuard<'_, ProcessedStore> {
        self.processed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Persist the processed-fingerprint store. Called once when a batch ends;
    /// write failures are non-fatal (the next batch simply re-extracts).
    pub async fn save_processed(&self) {
        let store = self.processed_lock();
        let _ = store.save();
    }

    /// Number of fingerprints currently remembered.
    pub fn processed_count(&self) -> usize {
        self.processed_lock().count()
    }

    /// Forget every processed fingerprint (GUI "forget processed"). Returns how
    /// many records were dropped; the on-disk file is removed as well.
    pub fn forget_processed(&self) -> usize {
        self.processed_lock().remove_all()
    }

    pub async fn process_file(
        &self,
        id: u64,
        path: &Path,
        cancel: &CancelToken,
        gate: &PauseGate,
        emit: &mut (dyn FnMut(TaskEvent) + Send),
    ) -> TaskOutcome {
        let started = Instant::now();
        let cfg = &self.cfg;
        let mut staging_guard = TempGuard(None);
        // Plaintext ZIP produced by SSEFE decryption (removed on every exit path).
        let mut plain_guard = TempGuard(None);
        let mut warnings: Vec<String> = Vec::new();
        let mut out = TaskOutcome {
            task_id: id,
            path: path.to_path_buf(),
            ..Default::default()
        };
        macro_rules! bail {
            ($status:expr, $reason:expr) => {{
                if matches!($status, TaskStatus::Failed) && cfg.keep_broken {
                    staging_guard.keep();
                }
                out.warnings = warnings.clone();
                out.status = Some($status);
                out.reason = Some($reason.to_string());
                out.elapsed_ms = started.elapsed().as_millis() as u64;
                return out;
            }};
        }

        if !wait_pause_or_cancel(gate, cancel).await {
            bail!(TaskStatus::Cancelled, "已取消");
        }

        // --- skip files that were already processed successfully -------------
        let fp = fingerprint(path).unwrap_or_default();
        if cfg.skip_processed && !fp.is_empty() {
            let store = self.processed_lock();
            if store.contains(&fp) {
                bail!(TaskStatus::Skipped, "已处理过（processed.json），跳过");
            }
        }

        // --- detect -----------------------------------------------------------
        emit(TaskEvent::TaskStage {
            id,
            stage: "detect".into(),
            detail: None,
        });
        let mut det = match detect::detect(&self.sz, path, cancel).await {
            Ok(d) => d,
            Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
            Err(e) => bail!(TaskStatus::Failed, e.to_string()),
        };
        emit(TaskEvent::TaskDetected {
            id,
            detection: det.clone(),
        });
        out.detection = Some(det.clone());
        let _carved_guard = TempGuard(det.carved_path.clone());

        if !wait_pause_or_cancel(gate, cancel).await {
            bail!(TaskStatus::Cancelled, "已取消");
        }

        let is_ssefe = det.kind == FileKind::SsefeEncrypted;
        let mut read_path: PathBuf = det
            .carved_path
            .clone()
            .unwrap_or_else(|| path.to_path_buf());
        let mut chosen: Option<PasswordHit> = None;
        let mut produced: Option<(PathBuf, Vec<PathBuf>)> = None;

        // Can the opt-in Windows "execute the self-extractor" downgrade apply?
        // Only for PE files whose payload 7-Zip could not read as data.
        let sfx_exec_allowed = !is_ssefe
            && cfg.allow_execute_sfx
            && !cfg.dry_run
            && cfg!(target_os = "windows")
            && det.container == "pe"
            && det.kind == FileKind::NotArchive;

        if !is_ssefe {
            if !sfx_exec_allowed {
                match det.kind {
                    FileKind::NotArchive => bail!(TaskStatus::Skipped, "不是压缩包"),
                    FileKind::Unknown => bail!(TaskStatus::Skipped, "无法识别的格式"),
                    _ => {}
                }
            }
            if !cfg.force_documents {
                if let Some(ext) = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                {
                    if DOC_EXTS.contains(&ext.as_str()) {
                        bail!(
                            TaskStatus::Skipped,
                            format!("文档容器 .{ext}（启用 force_documents 可展开）")
                        );
                    }
                }
                if det.office_like {
                    bail!(TaskStatus::Skipped, "看起来是 Office/JAR 文档容器");
                }
            }

            // --- password (inner 7-Zip archive) ------------------------------
            if det.password_state == PasswordState::NeedsPassword {
                emit(TaskEvent::TaskStage {
                    id,
                    stage: "password".into(),
                    detail: None,
                });
                let (found, tried) = match self
                    .find_7z_password(id, &read_path, &det, cancel, gate, &mut *emit)
                    .await
                {
                    Ok(v) => v,
                    Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                    Err(e) => bail!(TaskStatus::Failed, e.to_string()),
                };
                out.passwords_tried = tried;
                chosen = found;
                if chosen.is_none() {
                    emit(TaskEvent::TaskPasswordMissing { id, tried });
                    bail!(
                        TaskStatus::NeedsPassword,
                        format!("没有匹配的密码（已尝试 {tried} 个候选）")
                    );
                }
            }

            // Re-list with the working password when the header was encrypted,
            // so the layout decision and the size estimate use the real list.
            if let Some(hit) = &chosen {
                if !det.listing_ok_no_pw {
                    if let Ok(l) = self
                        .sz
                        .list(&read_path, Some(hit.password.as_str()), cancel)
                        .await
                    {
                        det.top_level = l.top_level(cfg.strip_junk);
                        det.total_uncompressed = l.total_uncompressed();
                        det.total_entries = l.file_count();
                        det.encrypted_entries = l.encrypted_entries().count();
                        det.quick_entry = l.smallest_encrypted().map(|e| e.raw_path.clone());
                        det.listing_ok_no_pw = true;
                        emit(TaskEvent::TaskDetected {
                            id,
                            detection: det.clone(),
                        });
                    }
                }
            }

            // --- opt-in Windows downgrade: run the self-extractor ------------
            if sfx_exec_allowed {
                produced = match self
                    .try_execute_sfx(
                        id,
                        path,
                        &mut det,
                        cancel,
                        gate,
                        &mut staging_guard,
                        &mut *emit,
                    )
                    .await
                {
                    Ok(p) => p,
                    Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                    Err(e) => bail!(TaskStatus::Failed, e.to_string()),
                };
                if produced.is_none() {
                    bail!(TaskStatus::Failed, "自解压执行未能产出可用文件");
                }
                out.detection = Some(det.clone());
                emit(TaskEvent::TaskDetected {
                    id,
                    detection: det.clone(),
                });
            }
        } else {
            // --- SSEFE (Paranoia File Encryption v4) -------------------------
            emit(TaskEvent::TaskStage {
                id,
                stage: "password".into(),
                detail: Some("Paranoia File Encryption v4 (SSEFE)".into()),
            });
            let (found, tried) = match self
                .find_ssefe_password(id, path, &det, cancel, gate, &mut *emit)
                .await
            {
                Ok(v) => v,
                Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                Err(e) => bail!(TaskStatus::Failed, e.to_string()),
            };
            out.passwords_tried = tried;
            let hit = match found {
                Some(hit) => hit,
                None => {
                    emit(TaskEvent::TaskPasswordMissing { id, tried });
                    let reason = if tried == 0 {
                        "需要 Paranoia v4 明文口令（未配置任何候选密码）".to_string()
                    } else {
                        format!("需要 Paranoia v4 明文口令：已尝试 {tried} 个密码均失败")
                    };
                    bail!(TaskStatus::NeedsPassword, reason);
                }
            };
            out.password_source = Some(hit.source.clone());
            chosen = Some(hit.clone());

            if !cfg.dry_run {
                emit(TaskEvent::TaskStage {
                    id,
                    stage: "decrypt".into(),
                    detail: Some("SSEFE v4 解密".into()),
                });
                // Put the plaintext temp ZIP next to the output root when that
                // directory exists; fall back to the system temp dir.
                let staging_parent: Option<PathBuf> = cfg
                    .output_root
                    .clone()
                    .or_else(|| path.parent().map(Path::to_path_buf));
                let staging_parent = staging_parent.filter(|p| p.is_dir());
                let mut tried = out.passwords_tried;
                match self
                    .decrypt_ssefe_with_retry(
                        id,
                        path,
                        hit,
                        staging_parent.as_deref(),
                        &det,
                        &mut tried,
                        cancel,
                        gate,
                        &mut *emit,
                    )
                    .await
                {
                    Ok((hit, p)) => {
                        out.passwords_tried = hit.tried;
                        chosen = Some(hit);
                        plain_guard.set(p.clone());
                        read_path = p;
                    }
                    Err(SsefeError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                    Err(SsefeError::WrongPassword) => {
                        out.passwords_tried = tried;
                        emit(TaskEvent::TaskPasswordMissing { id, tried });
                        bail!(
                            TaskStatus::NeedsPassword,
                            format!("Paranoia v4 密码被拒绝：已尝试 {tried} 个候选密码均失败")
                        );
                    }
                    Err(e) => bail!(TaskStatus::Failed, format!("解密失败：{}", e.message())),
                }

                // Re-list the plaintext inner ZIP: it drives layout, size and
                // entry counts from now on.
                emit(TaskEvent::TaskStage {
                    id,
                    stage: "detect".into(),
                    detail: Some("解析解密后的内层 ZIP".into()),
                });
                match self.sz.list(&read_path, None, cancel).await {
                    Ok(l) => {
                        det.kind = FileKind::Archive;
                        det.container = "zip".into();
                        det.format = Some(if l.archive_type.is_empty() {
                            "zip".to_string()
                        } else {
                            l.archive_type.clone()
                        });
                        det.total_entries = l.file_count();
                        det.encrypted_entries = l.encrypted_entries().count();
                        det.top_level = l.top_level(cfg.strip_junk);
                        det.total_uncompressed = l.total_uncompressed();
                        det.quick_entry = l.smallest_encrypted().map(|e| e.raw_path.clone());
                        det.listing_ok_no_pw = det.encrypted_entries == 0;
                        det.password_state = if det.encrypted_entries > 0 {
                            PasswordState::NeedsPassword
                        } else {
                            PasswordState::NoPassword
                        };
                        det.notes
                            .push("已用 Paranoia v4 明文口令解密为内层 ZIP".into());
                        out.detection = Some(det.clone());
                        emit(TaskEvent::TaskDetected {
                            id,
                            detection: det.clone(),
                        });
                    }
                    Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                    Err(e) => bail!(
                        TaskStatus::Failed,
                        format!("解密成功，但无法读取内层 ZIP：{e}")
                    ),
                }
            }
        }

        // --- plan -------------------------------------------------------------
        let top_count = match &produced {
            Some((_, items)) => items.len().max(1),
            None => det.top_level.len().max(1),
        };
        let mut plan = layout::plan_dest(cfg.layout, path, cfg.output_root.as_deref(), top_count);
        out.dest = Some(plan.dest_root.clone());
        if cfg.dry_run {
            bail!(
                TaskStatus::Skipped,
                format!("dry-run -> {}", plan.dest_root.display())
            );
        }

        // --- extract into staging (unless the self-extractor already did) -----
        let staging: PathBuf = match produced.take() {
            Some((dir, _)) => dir,
            None => {
                emit(TaskEvent::TaskStage {
                    id,
                    stage: "extract".into(),
                    detail: None,
                });
                if let Err(e) = fs::create_dir_all(&plan.parent) {
                    bail!(
                        TaskStatus::Failed,
                        format!("无法创建 {}：{e}", plan.parent.display())
                    );
                }
                if let Some(free) = free_space(&plan.parent) {
                    let need = (det.total_uncompressed as f64 * cfg.min_free_ratio) as u64;
                    if det.total_uncompressed > 0 && free < need {
                        bail!(
                            TaskStatus::Failed,
                            format!(
                                "磁盘空间不足：需要 {}，可用 {}",
                                human_bytes(need),
                                human_bytes(free)
                            )
                        );
                    }
                }
                let dir = plan
                    .parent
                    .join(format!("{}{}", crate::TMP_PREFIX, Uuid::new_v4().simple()));
                staging_guard.set(dir.clone());

                let pool = password_candidates(cfg, &det);
                let mut tried = out.passwords_tried;
                match self
                    .extract_with_password_retry(
                        id,
                        &read_path,
                        &dir,
                        chosen.clone(),
                        &pool,
                        &mut tried,
                        cancel,
                        gate,
                        &mut *emit,
                    )
                    .await
                {
                    Ok(hit) => {
                        chosen = hit;
                        out.passwords_tried = tried;
                    }
                    Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                    Err(UpError::PasswordRequired) => {
                        out.passwords_tried = tried;
                        let reason = if tried == 0 {
                            "需要密码（未配置任何候选密码）".to_string()
                        } else {
                            format!("解压时密码被拒绝（已尝试 {tried} 个候选密码）")
                        };
                        bail!(TaskStatus::NeedsPassword, reason);
                    }
                    Err(other) => bail!(TaskStatus::Failed, other.to_string()),
                }
                if !wait_pause_or_cancel(gate, cancel).await {
                    bail!(TaskStatus::Cancelled, "已取消");
                }
                dir
            }
        };

        // --- security scrub + junk strip + empty check ------------------------
        scrub_staging(&staging, &mut warnings);
        let mut items: Vec<PathBuf> = fs::read_dir(&staging)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        if cfg.strip_junk {
            // Best-effort cleanup: junk removal failing is not a task failure.
            let _ = layout::strip_junk_tree(&staging);
            items.retain(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                !layout::is_junk_name(&name)
            });
        }
        if items.is_empty() {
            bail!(TaskStatus::Failed, "解压未产出文件");
        }

        // --- nested cascade: keep unpacking while the result is one archive file
        let mut nested_depth = 0usize;
        if cfg.nested {
            let max_depth = cfg.effective_max_depth();
            let mut seen_fps: HashSet<String> = HashSet::new();
            if !det.fingerprint.is_empty() {
                seen_fps.insert(det.fingerprint.clone());
            }
            let mut nested_guards: Vec<TempGuard> = Vec::new();
            loop {
                if !wait_pause_or_cancel(gate, cancel).await {
                    bail!(TaskStatus::Cancelled, "已取消");
                }
                let mut entries: Vec<PathBuf> = fs::read_dir(&staging)
                    .map(|rd| rd.flatten().map(|e| e.path()).collect())
                    .unwrap_or_default();
                if entries.len() != 1 {
                    break;
                }
                let inner = entries.pop().unwrap();
                if !inner.is_file() {
                    break;
                }
                let inner_name = inner
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let det_inner = match detect::detect(&self.sz, &inner, cancel).await {
                    Ok(d) => d,
                    Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                    Err(_) => break,
                };
                let descend = matches!(
                    det_inner.kind,
                    FileKind::Archive
                        | FileKind::MaskedArchive
                        | FileKind::SfxExecutable
                        | FileKind::SsefeEncrypted
                );
                if !descend {
                    break;
                }
                if nested_depth >= max_depth {
                    det.notes.push(format!(
                        "已达最大嵌套层数（{max_depth}），保留 {inner_name}"
                    ));
                    break;
                }
                // Loop protection: a layer that repeats a fingerprint we have
                // already seen cannot make progress.
                if !det_inner.fingerprint.is_empty()
                    && !seen_fps.insert(det_inner.fingerprint.clone())
                {
                    warnings.push(format!("检测到循环嵌套，停止级联：{inner_name}"));
                    break;
                }
                nested_depth += 1;
                nested_guards.push(TempGuard(det_inner.carved_path.clone()));
                emit(TaskEvent::TaskStage {
                    id,
                    stage: "nested".into(),
                    detail: Some(format!("解压内层：{inner_name}")),
                });

                let mut inner_guard = TempGuard(None);
                let mut read_inner: PathBuf = det_inner
                    .carved_path
                    .clone()
                    .unwrap_or_else(|| inner.clone());
                // Candidate accepted for this layer (if any). Every layer rebuilds
                // the full pool, so an inner archive may use a different password.
                let mut inner_hit: Option<PasswordHit> = None;

                if det_inner.kind == FileKind::SsefeEncrypted {
                    let (found, tried) = match self
                        .find_ssefe_password(id, &inner, &det_inner, cancel, gate, &mut *emit)
                        .await
                    {
                        Ok(v) => v,
                        Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                        Err(e) => {
                            warnings.push(format!(
                                "内层 Paranoia 归档「{inner_name}」密码检测失败：{e}，保留外层内容"
                            ));
                            break;
                        }
                    };
                    let Some(hit) = found else {
                        emit(TaskEvent::TaskPasswordMissing { id, tried });
                        warnings.push(format!(
                            "内层 Paranoia 归档「{inner_name}」需要密码（已试 {tried} 个候选），保留外层内容"
                        ));
                        break;
                    };
                    let mut decrypt_tried = hit.tried;
                    match self
                        .decrypt_ssefe_with_retry(
                            id,
                            &inner,
                            hit,
                            Some(&staging),
                            &det_inner,
                            &mut decrypt_tried,
                            cancel,
                            gate,
                            &mut *emit,
                        )
                        .await
                    {
                        Ok((hit, p)) => {
                            inner_guard.set(p.clone());
                            read_inner = p;
                            inner_hit = Some(hit);
                        }
                        Err(SsefeError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                        Err(SsefeError::WrongPassword) => {
                            emit(TaskEvent::TaskPasswordMissing {
                                id,
                                tried: decrypt_tried,
                            });
                            warnings.push(format!(
                                "内层 Paranoia 归档「{inner_name}」密码被拒绝（已尝试 {decrypt_tried} 个候选），保留外层内容"
                            ));
                            break;
                        }
                        Err(e) => {
                            warnings.push(format!(
                                "内层 Paranoia 归档「{inner_name}」解密失败：{}，保留外层内容",
                                e.message()
                            ));
                            break;
                        }
                    }
                } else if det_inner.password_state == PasswordState::NeedsPassword {
                    let (found, tried) = match self
                        .find_7z_password(id, &read_inner, &det_inner, cancel, gate, &mut *emit)
                        .await
                    {
                        Ok(v) => v,
                        Err(UpError::Cancelled) => bail!(TaskStatus::Cancelled, "已取消"),
                        Err(e) => {
                            warnings.push(format!(
                                "内层归档「{inner_name}」密码检测失败：{e}，保留外层内容"
                            ));
                            break;
                        }
                    };
                    let Some(hit) = found else {
                        emit(TaskEvent::TaskPasswordMissing { id, tried });
                        warnings.push(format!(
                            "内层归档「{inner_name}」需要密码（已试 {tried} 个候选），保留外层内容"
                        ));
                        break;
                    };
                    inner_hit = Some(hit);
                }

                let sub = staging.join(format!("{}{}", crate::TMP_PREFIX, Uuid::new_v4().simple()));
                if let Err(e) = fs::create_dir_all(&sub) {
                    warnings.push(format!(
                        "无法创建内层暂存目录 {}：{e}，保留外层内容",
                        sub.display()
                    ));
                    break;
                }
                let pool = password_candidates(cfg, &det_inner);
                let mut inner_tried = inner_hit.as_ref().map(|h| h.tried).unwrap_or(0);
                let extract_res = self
                    .extract_with_password_retry(
                        id,
                        &read_inner,
                        &sub,
                        inner_hit.clone(),
                        &pool,
                        &mut inner_tried,
                        cancel,
                        gate,
                        &mut *emit,
                    )
                    .await;
                match extract_res {
                    Ok(hit) => {
                        if let Some(hit) = hit {
                            if let Some(warning) = non_first_candidate_warning(&hit) {
                                warnings.push(warning);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = remove_path(&sub);
                        match e {
                            UpError::Cancelled => bail!(TaskStatus::Cancelled, "已取消"),
                            UpError::PasswordRequired => {
                                warnings.push(format!(
                                    "内层归档「{inner_name}」密码被拒绝（已尝试 {inner_tried} 个候选），保留外层内容"
                                ));
                                break;
                            }
                            other => {
                                // Corrupt / CRC failure: keep what the outer layer
                                // already produced instead of failing the task.
                                warnings.push(format!(
                                    "内层归档「{inner_name}」解压失败：{other}，保留外层内容"
                                ));
                                break;
                            }
                        }
                    }
                }

                scrub_staging(&sub, &mut warnings);
                let _ = fs::remove_file(&inner);
                let mut moved = 0usize;
                let mut move_failed = false;
                if let Ok(rd) = fs::read_dir(&sub) {
                    for e in rd.flatten() {
                        let dst = staging.join(e.file_name());
                        if let Err(err) = crate::util::move_path(&e.path(), &dst) {
                            warnings.push(format!("级联移动内层条目失败：{err}"));
                            move_failed = true;
                            break;
                        }
                        moved += 1;
                    }
                }
                let _ = fs::remove_dir(&sub);
                if move_failed {
                    break;
                }
                if moved == 0 {
                    warnings.push(format!(
                        "内层归档「{inner_name}」解压未产出文件，保留外层内容"
                    ));
                    break;
                }
                det.notes.push(format!("级联解包（第 {nested_depth} 层）：{inner_name}"));
            }

            if nested_depth > 0 {
                scrub_staging(&staging, &mut warnings);
                let _ = layout::strip_junk_tree(&staging);
                items = fs::read_dir(&staging)
                    .map(|rd| rd.flatten().map(|e| e.path()).collect())
                    .unwrap_or_default();
                if items.is_empty() {
                    bail!(TaskStatus::Failed, "解压未产出文件");
                }
                let (fcount, fbytes) = crate::util::count_tree(&staging);
                det.total_entries = fcount as usize;
                det.total_uncompressed = fbytes;
                det.top_level = items
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                    .collect();
                emit(TaskEvent::TaskDetected {
                    id,
                    detection: det.clone(),
                });
                plan = layout::plan_dest(cfg.layout, path, cfg.output_root.as_deref(), items.len());
                out.dest = Some(plan.dest_root.clone());
            }
        }

        // --- land into place --------------------------------------------------
        emit(TaskEvent::TaskStage {
            id,
            stage: "land".into(),
            detail: None,
        });
        if plan.foldered {
            if let Err(e) = fs::create_dir_all(&plan.dest_root) {
                bail!(
                    TaskStatus::Failed,
                    format!("无法创建 {}：{e}", plan.dest_root.display())
                );
            }
        }
        let mut stats = MoveStats::default();
        let mut produced_items: Vec<String> = Vec::new();
        for item in &items {
            if cancel.is_cancelled() {
                bail!(TaskStatus::Cancelled, "已取消");
            }
            let name = item
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_default();
            let target = plan.dest_root.join(&name);
            match layout::move_with_policy_landing(item, &target, cfg.conflict, true) {
                Ok((st, landed)) => {
                    stats.merge_public(st);
                    if let Ok(rel) = landed.strip_prefix(&plan.dest_root) {
                        produced_items.push(rel.to_string_lossy().to_string());
                    }
                }
                Err(e) => bail!(TaskStatus::Failed, e.to_string()),
            }
        }
        if plan.foldered {
            if let Ok(mut rd) = fs::read_dir(&plan.dest_root) {
                if rd.next().is_none() {
                    let _ = fs::remove_dir(&plan.dest_root);
                }
            }
        }

        // --- record the fingerprint (skipped by dry runs) ---------------------
        if !fp.is_empty() {
            self.processed_lock()
                .insert(&fp, &plan.dest_root, "success");
        }

        if let Some(hit) = &chosen {
            out.password_source = Some(hit.source.clone());
            if let Some(warning) = non_first_candidate_warning(hit) {
                warnings.push(warning);
            }
        }
        out.warnings = warnings;
        out.status = Some(TaskStatus::Success);
        out.files = stats.files as usize;
        out.bytes = stats.bytes;
        if stats.skipped > 0 {
            out.reason = Some(format!("保留 {} 个已存在条目", stats.skipped));
        }
        out.elapsed_ms = started.elapsed().as_millis() as u64;
        out
    }

    /// Opt-in Windows downgrade: run a self-extracting executable whose payload
    /// 7-Zip could not read as data. Returns the staging folder and the
    /// top-level items it produced.
    async fn try_execute_sfx(
        &self,
        id: u64,
        path: &Path,
        det: &mut Detection,
        cancel: &CancelToken,
        gate: &PauseGate,
        staging_guard: &mut TempGuard,
        emit: &mut (dyn FnMut(TaskEvent) + Send),
    ) -> Result<Option<(PathBuf, Vec<PathBuf>)>> {
        let cfg = &self.cfg;
        if !wait_pause_or_cancel(gate, cancel).await {
            return Err(UpError::Cancelled);
        }
        emit(TaskEvent::TaskStage {
            id,
            stage: "execute_sfx".into(),
            detail: Some("Windows 执行式降级（可选，默认关闭，未实机验证）".into()),
        });

        let parent = cfg
            .output_root
            .clone()
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        if let Err(e) = fs::create_dir_all(&parent) {
            return Err(UpError::Other(format!("无法创建 {}：{e}", parent.display())));
        }
        let staging = parent.join(format!("{}{}", crate::TMP_PREFIX, Uuid::new_v4().simple()));
        if let Err(e) = fs::create_dir_all(&staging) {
            return Err(UpError::Other(format!("无法创建 {}：{e}", staging.display())));
        }
        staging_guard.set(staging.clone());

        // Candidate passwords: explicit ones, then archive comment hints.
        let mut pws: Vec<String> = Vec::new();
        pws.extend(cfg.passwords.iter().cloned());
        pws.extend(det.hints.iter().cloned());
        let mut seen: HashSet<String> = HashSet::new();
        pws.retain(|p| !p.is_empty() && seen.insert(p.clone()));

        if let Err(e) = crate::sfx::run_sfx(path, &pws, &staging, cancel).await {
            return match e {
                UpError::Cancelled => Err(UpError::Cancelled),
                other => {
                    det.notes.push(format!("执行式降级失败：{other}"));
                    Err(UpError::Other(format!("自解压执行失败：{other}")))
                }
            };
        }

        let items: Vec<PathBuf> = fs::read_dir(&staging)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        if items.is_empty() {
            det.notes.push("执行式降级未产出文件".into());
            return Err(UpError::Other("自解压执行未产出文件".into()));
        }

        let (files, bytes) = crate::util::count_tree(&staging);
        det.kind = FileKind::SfxExecutable;
        det.password_state = PasswordState::Unknown;
        det.total_entries = files as usize;
        det.total_uncompressed = bytes;
        det.top_level = items
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        det.notes.push(format!(
            "执行式降级产出 {} 个顶层条目（可选路径，尚未在 Windows 实机验证）",
            items.len()
        ));
        Ok(Some((staging, items)))
    }

    /// Try every password candidate (`config` from RunConfig.passwords, archive
    /// comment `hint`s, optionally the `empty` password) against an SSEFE v4
    /// container. verify_password only checks the fast checkcode; the full
    /// decryption (and MAC verification) happens once a candidate is accepted.
    async fn find_ssefe_password(
        &self,
        id: u64,
        path: &Path,
        det: &Detection,
        cancel: &CancelToken,
        gate: &PauseGate,
        emit: &mut (dyn FnMut(TaskEvent) + Send),
    ) -> Result<(Option<PasswordHit>, usize)> {
        let cands = password_candidates(&self.cfg, det);
        let mut tried = 0usize;
        for cand in cands {
            if !wait_pause_or_cancel(gate, cancel).await {
                return Err(UpError::Cancelled);
            }
            tried = cand.tried;
            let input = path.to_path_buf();
            let password = cand.password.clone();
            let check =
                tokio::task::spawn_blocking(move || ssefe::verify_password(&input, &password)).await;
            let ok = match check {
                Ok(Ok(accepted)) => accepted,
                Ok(Err(SsefeError::Cancelled)) => return Err(UpError::Cancelled),
                Ok(Err(e)) => {
                    emit(TaskEvent::Log {
                        line: format!("密码校验失败：{}", e.message()),
                    });
                    false
                }
                Err(e) => {
                    emit(TaskEvent::Log {
                        line: format!("密码校验任务失败：{e}"),
                    });
                    false
                }
            };
            emit(TaskEvent::TaskPasswordAttempt {
                id,
                source: cand.source.clone(),
                ok,
                tried,
            });
            if ok {
                return Ok((Some(cand), tried));
            }
        }
        Ok((None, tried))
    }

    /// Try every password candidate against a 7-Zip archive by testing a single
    /// entry (t -ir!<entry>), which is much cheaper than extracting.
    async fn find_7z_password(
        &self,
        id: u64,
        read_path: &Path,
        det: &Detection,
        cancel: &CancelToken,
        gate: &PauseGate,
        emit: &mut (dyn FnMut(TaskEvent) + Send),
    ) -> Result<(Option<PasswordHit>, usize)> {
        let cands = password_candidates(&self.cfg, det);
        let mut tried = 0usize;
        for cand in cands {
            if !wait_pause_or_cancel(gate, cancel).await {
                return Err(UpError::Cancelled);
            }
            tried = cand.tried;
            match self
                .verify_7z_password(read_path, det, &cand.password, cancel)
                .await
            {
                Ok(true) => {
                    emit(TaskEvent::TaskPasswordAttempt {
                        id,
                        source: cand.source.clone(),
                        ok: true,
                        tried,
                    });
                    return Ok((Some(cand), tried));
                }
                Ok(false) => {
                    emit(TaskEvent::TaskPasswordAttempt {
                        id,
                        source: cand.source.clone(),
                        ok: false,
                        tried,
                    });
                }
                Err(UpError::Cancelled) => return Err(UpError::Cancelled),
                Err(_) => {
                    emit(TaskEvent::TaskPasswordAttempt {
                        id,
                        source: cand.source.clone(),
                        ok: false,
                        tried,
                    });
                }
            }
        }
        Ok((None, tried))
    }

    /// Extract `archive` into `dest` once, forwarding 7-Zip progress as
    /// `TaskProgress` events (de-duplicated by percent).
    async fn extract_once(
        &self,
        id: u64,
        archive: &Path,
        password: Option<&str>,
        dest: &Path,
        cancel: &CancelToken,
        emit: &mut (dyn FnMut(TaskEvent) + Send),
    ) -> Result<()> {
        let mut last_pct: i64 = -1;
        let mut progress = |line: &str| {
            if let Some(pct) = percent_of(line) {
                if pct as i64 != last_pct {
                    last_pct = pct as i64;
                    emit(TaskEvent::TaskProgress {
                        id,
                        percent: Some(pct),
                        line: line.trim().to_string(),
                    });
                }
            }
        };
        self.sz
            .extract(archive, password, dest, cancel, &mut progress)
            .await
            .map(|_| ())
    }

    /// Extract `archive` into `dest`, retrying the remaining password candidates
    /// when 7-Zip rejects the current one (contract §7.3). `first` is the
    /// candidate already accepted by `find_7z_password` (`None` when the listing
    /// needed no password); `tried` is advanced as candidates are attempted.
    /// Returns the accepted candidate, or `None` if no password was needed at all.
    async fn extract_with_password_retry(
        &self,
        id: u64,
        archive: &Path,
        dest: &Path,
        first: Option<PasswordHit>,
        pool: &[PasswordHit],
        tried: &mut usize,
        cancel: &CancelToken,
        gate: &PauseGate,
        emit: &mut (dyn FnMut(TaskEvent) + Send),
    ) -> Result<Option<PasswordHit>> {
        if !wait_pause_or_cancel(gate, cancel).await {
            return Err(UpError::Cancelled);
        }
        let first_password = first.as_ref().map(|hit| hit.password.as_str());
        match self
            .extract_once(id, archive, first_password, dest, cancel, emit)
            .await
        {
            Ok(()) => return Ok(first),
            Err(UpError::Cancelled) => return Err(UpError::Cancelled),
            Err(error) if is_retryable_rejection(&error) => {}
            Err(error) => return Err(error),
        }
        for cand in candidates_after(pool, *tried) {
            if !wait_pause_or_cancel(gate, cancel).await {
                return Err(UpError::Cancelled);
            }
            // A rejected attempt can leave partial files behind; start clean.
            let _ = remove_path(dest);
            if let Err(error) = fs::create_dir_all(dest) {
                return Err(UpError::Io {
                    path: dest.to_path_buf(),
                    source: error,
                });
            }
            match self
                .extract_once(id, archive, Some(&cand.password), dest, cancel, emit)
                .await
            {
                Ok(()) => {
                    emit(TaskEvent::TaskPasswordAttempt {
                        id,
                        source: cand.source.clone(),
                        ok: true,
                        tried: cand.tried,
                    });
                    *tried = cand.tried;
                    return Ok(Some(cand));
                }
                Err(UpError::Cancelled) => return Err(UpError::Cancelled),
                Err(error) => {
                    *tried = cand.tried;
                    emit(TaskEvent::TaskPasswordAttempt {
                        id,
                        source: cand.source.clone(),
                        ok: false,
                        tried: cand.tried,
                    });
                    if !is_retryable_rejection(&error) {
                        return Err(error);
                    }
                }
            }
        }
        emit(TaskEvent::TaskPasswordMissing { id, tried: *tried });
        Err(UpError::PasswordRequired)
    }

    /// Decrypt an SSEFE v4 container with the candidate accepted by
    /// `find_ssefe_password`; if the full decryption still rejects it as a wrong
    /// password, keep trying the remaining candidates (contract §7.3). `tried` is
    /// advanced as candidates are attempted.
    async fn decrypt_ssefe_with_retry(
        &self,
        id: u64,
        input: &Path,
        first: PasswordHit,
        staging_parent: Option<&Path>,
        det: &Detection,
        tried: &mut usize,
        cancel: &CancelToken,
        gate: &PauseGate,
        emit: &mut (dyn FnMut(TaskEvent) + Send),
    ) -> std::result::Result<(PasswordHit, PathBuf), SsefeError> {
        if !wait_pause_or_cancel(gate, cancel).await {
            return Err(SsefeError::Cancelled);
        }
        match ssefe::decrypt_to_temp(id, input, &first.password, staging_parent, cancel, emit).await
        {
            Ok(path) => return Ok((first, path)),
            Err(SsefeError::Cancelled) => return Err(SsefeError::Cancelled),
            Err(error) if is_retryable_ssefe(&error) => {}
            Err(error) => return Err(error),
        }
        let pool = password_candidates(&self.cfg, det);
        for cand in candidates_after(&pool, first.tried) {
            if !wait_pause_or_cancel(gate, cancel).await {
                return Err(SsefeError::Cancelled);
            }
            match ssefe::decrypt_to_temp(
                id,
                input,
                &cand.password,
                staging_parent,
                cancel,
                emit,
            )
            .await
            {
                Ok(path) => {
                    *tried = cand.tried;
                    emit(TaskEvent::TaskPasswordAttempt {
                        id,
                        source: cand.source.clone(),
                        ok: true,
                        tried: cand.tried,
                    });
                    return Ok((cand, path));
                }
                Err(SsefeError::Cancelled) => return Err(SsefeError::Cancelled),
                Err(error) => {
                    *tried = cand.tried;
                    emit(TaskEvent::TaskPasswordAttempt {
                        id,
                        source: cand.source.clone(),
                        ok: false,
                        tried: cand.tried,
                    });
                    if !is_retryable_ssefe(&error) {
                        return Err(error);
                    }
                }
            }
        }
        Err(SsefeError::WrongPassword)
    }

    async fn verify_7z_password(
        &self,
        read_path: &Path,
        det: &Detection,
        pw: &str,
        cancel: &CancelToken,
    ) -> Result<bool> {
        let pattern = det.quick_entry.as_deref().filter(|q| pattern_safe(q));
        match self
            .sz
            .test_entry(read_path, Some(pw), pattern, cancel)
            .await?
        {
            PwCheck::Ok => Ok(true),
            _ => Ok(false),
        }
    }
}

/// Reject dangerous entries a malicious archive may have created: symlinks and
/// names that would escape the extraction root (absolute paths, `..`, Windows
/// drive letters / UNC / alternate data streams). Offending entries are removed
/// and reported as warnings — the rest of the task keeps going.
fn scrub_staging(root: &Path, warnings: &mut Vec<String>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                warnings.push(format!("无法检查条目：{}", p.display()));
                continue;
            };
            if meta.file_type().is_symlink() {
                let _ = remove_path(&p);
                warnings.push(format!("已删除符号链接：{}", relative_label(&p, root)));
                continue;
            }
            if dangerous_component(&name) {
                let _ = remove_path(&p);
                warnings.push(format!("已删除越界路径条目：{}", relative_label(&p, root)));
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            }
        }
    }
}

fn relative_label(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

/// A single path component that must never appear in extractor output.
fn dangerous_component(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return true;
    }
    // Windows drive letters / alternate data streams, UNC prefixes and legacy
    // backslash separators.
    if name.contains(':') || name.contains('\\') {
        return true;
    }
    // Absolute unix paths never appear as a component, but be defensive.
    name.starts_with('/')
}

fn pattern_safe(p: &str) -> bool {
    !p.contains(['*', '?', '[', ']'])
}

fn percent_of(line: &str) -> Option<u8> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\s*(\d{1,3})%").expect("percent regex"));
    let cleaned = line.trim_matches(|c: char| c == '\u{8}' || c == ' ');
    re.captures(cleaned)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u8>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_parsing() {
        assert_eq!(percent_of("  42% 12 - name.txt"), Some(42));
        assert_eq!(percent_of("\u{8}  7% - x"), Some(7));
        assert_eq!(percent_of("Everything is Ok"), None);
    }

    #[test]
    fn pattern_guard() {
        assert!(pattern_safe("a/b/c.txt"));
        assert!(!pattern_safe("a/b?.txt"));
        assert!(!pattern_safe("a/b[1].txt"));
    }

    #[test]
    fn dangerous_components_are_rejected() {
        assert!(dangerous_component(".."));
        assert!(dangerous_component("C:"));
        assert!(dangerous_component("a:b"));
        assert!(dangerous_component("\\\\server\\share"));
        assert!(dangerous_component("/etc/passwd"));
        assert!(!dangerous_component("normal.txt"));
        assert!(!dangerous_component("目录"));
    }

    #[test]
    fn scrub_removes_symlinks_and_keeps_regular_files() {
        let dir = std::env::temp_dir()
            .join(format!("unparanoia-scrub-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("ok.txt"), b"ok").unwrap();
        std::fs::write(dir.join("sub").join("deep.txt"), b"ok").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", dir.join("evil_link")).unwrap();
        let mut warnings = Vec::new();
        scrub_staging(&dir, &mut warnings);
        assert!(dir.join("ok.txt").exists());
        assert!(dir.join("sub").join("deep.txt").exists());
        #[cfg(unix)]
        {
            assert!(!dir.join("evil_link").exists(), "symlink must be removed");
            assert!(!warnings.is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn det_with_hints(hints: &[&str]) -> Detection {
        Detection {
            hints: hints.iter().map(|h| h.to_string()).collect(),
            ..Detection::default()
        }
    }

    fn hit(tried: usize) -> PasswordHit {
        PasswordHit {
            source: "config".to_string(),
            password: "x".to_string(),
            tried,
        }
    }

    #[test]
    fn password_pool_order_and_dedup() {
        let mut cfg = RunConfig::default();
        cfg.passwords = vec!["first".into(), "second".into(), "first".into(), "".into()];
        cfg.try_empty_password = true;
        let det = det_with_hints(&["second", "hint-only", "hint-only"]);
        let pool = password_candidates(&cfg, &det);
        let got: Vec<(String, String, usize)> = pool
            .iter()
            .map(|c| (c.source.clone(), c.password.clone(), c.tried))
            .collect();
        assert_eq!(
            got,
            vec![
                ("config".into(), "first".into(), 1),
                ("config".into(), "second".into(), 2),
                ("hint".into(), "hint-only".into(), 3),
                ("empty".into(), String::new(), 4),
            ]
        );
        // Without try_empty_password the empty candidate disappears.
        cfg.try_empty_password = false;
        let pool = password_candidates(&cfg, &det);
        assert_eq!(pool.len(), 3);
        assert!(pool.iter().all(|c| !c.password.is_empty()));
    }

    #[test]
    fn retry_decisions() {
        assert!(is_retryable_rejection(&UpError::PasswordRequired));
        assert!(!is_retryable_rejection(&UpError::Cancelled));
        assert!(!is_retryable_rejection(&UpError::Other("x".into())));
        assert!(is_retryable_ssefe(&SsefeError::WrongPassword));
        assert!(!is_retryable_ssefe(&SsefeError::Integrity));
        assert!(!is_retryable_ssefe(&SsefeError::Cancelled));
    }

    #[test]
    fn retry_candidates_skip_already_tried() {
        let mut cfg = RunConfig::default();
        cfg.passwords = vec!["a".into(), "b".into(), "c".into()];
        let pool = password_candidates(&cfg, &Detection::default());
        assert_eq!(candidates_after(&pool, 0).len(), 3);
        let rest: Vec<String> = candidates_after(&pool, 1)
            .iter()
            .map(|c| c.password.clone())
            .collect();
        assert_eq!(rest, vec!["b", "c"]);
        assert!(candidates_after(&pool, 3).is_empty());
    }

    #[test]
    fn non_first_candidate_success_warning() {
        assert_eq!(non_first_candidate_warning(&hit(1)), None);
        assert_eq!(
            non_first_candidate_warning(&hit(2)).as_deref(),
            Some("使用候选密码 #2 成功")
        );
        assert_eq!(
            non_first_candidate_warning(&hit(4)).as_deref(),
            Some("使用候选密码 #4 成功")
        );
    }
}
