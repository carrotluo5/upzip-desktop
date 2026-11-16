//! Tauri v2 shell for the merged Paranoia decryptor / archive extractor.
//!
//! The heavy lifting lives in `unparanoia-core` (SSEFE v4 decryption) and
//! `unparanoia-unpack-core` (detect / batch extract / layout / processed store).
//! This file only owns:
//!
//! * the single-flight batch state (running / paused / cancel token / summary),
//! * the persisted plain-text settings and the plain-text password store bridge,
//! * the two event streams the React frontend consumes:
//!   - `unparanoia://event` — every `TaskEvent` (queued / stage / detected /
//!     progress / password_attempt / password_missing / finished / log),
//!     serialized exactly as the engine emits it (tag = `type`, snake_case);
//!   - `unparanoia://done` — the final `Summary` of the batch.
//!
//! The password store is kept in plain text by `upzip_core::config`
//! (0600 on unix). Passwords are never logged.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use upzip_core::config::{
    add_password as core_add_password, clear_passwords as core_clear_passwords, config_dir,
    config_file_path, load_passwords as core_load_passwords,
    remove_password_at as core_remove_password_at,
};
use upzip_unpack_core as unpack;
use upzip_unpack_core::{
    expand_inputs, run_batch, CancelToken, Detection, Engine, PauseGate, ProcessedStore, Settings,
    SevenZip, Sink, Summary, TaskEvent, TaskStatus, TMP_PREFIX,
};

pub const EVENT_TASK: &str = "upzip://event";
pub const EVENT_DONE: &str = "upzip://done";

/// Sub-directory of the configuration directory that holds the persistent log.
const LOG_DIR_NAME: &str = "logs";
/// Persistent GUI log file name; rotated to unparanoia.log.1 once it grows
/// past LOG_MAX_BYTES.
const LOG_FILE_NAME: &str = "upzip.log";
/// Log rotation threshold (2 MiB); exactly one previous file is kept.
const LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Serializes log writes/rotation between the UI thread, the batch worker and
/// command handlers.
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Resolved bundled 7-Zip binary + its reported version (resolved once at
/// startup; `None` when no binary could be found at all).
#[derive(Clone, Debug)]
struct SevenZipInfo {
    exe: PathBuf,
    version: String,
}

pub struct AppState {
    /// Wall-clock anchor used only for diagnostics.
    started_at: Instant,
    sevenzip: Mutex<Option<SevenZipInfo>>,
    /// Single-flight guard for the batch worker.
    running: AtomicBool,
    /// Mirrors the engine `PauseGate` so `batch_state` can report it.
    paused: AtomicBool,
    cancel: Mutex<CancelToken>,
    gate: Arc<PauseGate>,
    summary: Mutex<Option<Summary>>,
    settings: Mutex<Settings>,
    /// Files this process was launched with (Finder "Open With" / argv).
    startup_paths: Mutex<Vec<String>>,
}

impl AppState {
    fn new(sevenzip: Option<SevenZipInfo>, settings: Settings, startup_paths: Vec<String>) -> Self {
        Self {
            started_at: Instant::now(),
            sevenzip: Mutex::new(sevenzip),
            running: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancel: Mutex::new(CancelToken::new()),
            gate: Arc::new(PauseGate::new()),
            summary: Mutex::new(None),
            settings: Mutex::new(settings),
            startup_paths: Mutex::new(startup_paths),
        }
    }
}

/// Lock without propagating poisoning: a panic in a previous batch must not
/// turn every subsequent command into a hard error.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Resets `running` unless disarmed once the worker actually owns the flag.
struct RunningGuard<'a> {
    flag: &'a AtomicBool,
    armed: bool,
}

impl<'a> RunningGuard<'a> {
    fn new(flag: &'a AtomicBool) -> Self {
        Self { flag, armed: true }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(false, Ordering::SeqCst);
        }
    }
}

fn locate_sevenzip() -> Result<SevenZipInfo, String> {
    // Priority matches the CLI: explicit --sevenzip (the GUI has none) >
    // UPZIP_7Z / UPZIP_7Z environment override > embedded 7-Zip >
    // automatic search. SevenZip::locate(None) already honours the two env
    // vars, so when one points at a real file we must not pass the embedded
    // path: an explicit path is checked before the env vars and would win.
    let env_override = ["UPZIP_7Z", "UPZIP_7Z"].iter().any(|variable| {
        std::env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value).is_file())
            .unwrap_or(false)
    });
    let embedded = if env_override {
        None
    } else {
        // Windows single-exe builds carry 7z.exe + 7z.dll inside our own
        // binary; ensure_7z() materializes them into %LOCALAPPDATA% and returns
        // the path. On macOS it returns None and SevenZip::locate falls back to
        // the bundled Resources/binaries/mac/7zz or an installed 7-Zip.
        match upzip_embedded_7z::ensure_7z() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("[upzip-gui] 内嵌 7-Zip 初始化失败，回退外部 7-Zip：{error}");
                None
            }
        }
    };
    let sz = SevenZip::locate(embedded).map_err(|error| {
        format!(
            "找不到 7-Zip（{error}）。请安装 7-Zip，或设置环境变量 UPZIP_7Z 指向 7zz/7z.exe，\
             或把二进制放到应用的 binaries/ 目录。"
        )
    })?;
    Ok(SevenZipInfo {
        version: sz.version(),
        exe: sz.exe,
    })
}

fn display_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- logging

/// Directory that holds the persistent GUI log (<config dir>/logs).
fn log_dir() -> PathBuf {
    match config_dir() {
        Ok(dir) => dir.join(LOG_DIR_NAME),
        // A broken config dir must not disable logging entirely: fall back to
        // the same temporary location the processed store uses.
        Err(_) => std::env::temp_dir().join("upzip-logs"),
    }
}

/// Full path of the current log file.
fn log_file_path() -> PathBuf {
    log_dir().join(LOG_FILE_NAME)
}

/// UTC timestamp with milliseconds, e.g. 2026-10-24T11:05:09.123Z.
///
/// Uses the civil-from-days algorithm instead of pulling in a local-time crate;
/// the log is diagnostics output, so unambiguous UTC is enough.
fn log_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil_from_days (days since 1970-01-01 -> Y-M-D).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

/// Rotate unparanoia.log -> unparanoia.log.1 when it exceeds the limit.
fn rotate_log_if_needed(path: &std::path::Path) -> std::io::Result<()> {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(_) => return Ok(()), // missing file: nothing to rotate
    };
    if size <= LOG_MAX_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    let _ = std::fs::remove_file(&rotated);
    std::fs::rename(path, &rotated)
}

/// Append one timestamped line to the persistent log. Creates the directory and
/// rotates the file as needed. Errors bubble up so append_log can report them.
fn try_append_log_line(line: &str) -> std::io::Result<()> {
    let _guard = LOG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = log_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(LOG_FILE_NAME);
    rotate_log_if_needed(&path)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{} {}", log_timestamp(), line)?;
    Ok(())
}

/// Best-effort log write used outside command handlers: never fails the caller.
fn append_log_line(line: &str) {
    if let Err(error) = try_append_log_line(line) {
        eprintln!("[upzip-gui] 写日志失败：{error}");
    }
}

/// Shorten long engine text lines for the log file.
fn shorten(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Human-readable name for a task status (log only).
fn task_status_name(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Success => "success",
        TaskStatus::Skipped => "skipped",
        TaskStatus::NeedsPassword => "needs_password",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

/// One log line per task event: the variant name plus its diagnostic fields.
fn describe_task_event(event: &TaskEvent) -> String {
    match event {
        TaskEvent::TaskQueued { id, path, size } => {
            format!("queued id={id} path={} size={size}", display_path(path))
        }
        TaskEvent::TaskStage { id, stage, detail } => match detail {
            Some(detail) => format!(
                "stage id={id} stage={stage} detail={}",
                shorten(detail, 300)
            ),
            None => format!("stage id={id} stage={stage}"),
        },
        TaskEvent::TaskDetected { id, detection } => format!(
            "detected id={id} container={} format={} entries={} encrypted={} notes={}",
            detection.container,
            detection.format.as_deref().unwrap_or("-"),
            detection.total_entries,
            detection.encrypted_entries,
            detection.notes.len()
        ),
        TaskEvent::TaskProgress { id, percent, line } => {
            let percent = percent
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "progress id={id} percent={percent} line={}",
                shorten(line, 400)
            )
        }
        TaskEvent::TaskPasswordAttempt {
            id,
            source,
            ok,
            tried,
        } => format!("password_attempt id={id} source={source} ok={ok} tried={tried}"),
        TaskEvent::TaskPasswordMissing { id, tried } => {
            format!("password_missing id={id} tried={tried}")
        }
        TaskEvent::TaskFinished { id, outcome } => {
            let status = outcome
                .status
                .as_ref()
                .map(task_status_name)
                .unwrap_or("none");
            format!(
                "finished id={id} status={status} path={} dest={} files={} bytes={} elapsed_ms={} reason={} warnings={}",
                display_path(&outcome.path),
                outcome.dest.as_deref().map(display_path).unwrap_or_default(),
                outcome.files,
                outcome.bytes,
                outcome.elapsed_ms,
                outcome
                    .reason
                    .as_deref()
                    .map(|reason| shorten(reason, 300))
                    .unwrap_or_default(),
                outcome.warnings.len()
            )
        }
        TaskEvent::Log { line } => format!("log {}", shorten(line, 500)),
    }
}

/// Where the resolved 7-Zip binary probably came from (log only).
fn seven_zip_source(info: &SevenZipInfo) -> String {
    for variable in ["UPZIP_7Z", "UPZIP_7Z"] {
        if let Some(value) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            if PathBuf::from(value) == info.exe {
                return format!("环境变量 {variable}");
            }
        }
    }
    let lowered = info.exe.to_string_lossy().to_ascii_lowercase();
    if lowered.contains("embedded-7z") {
        "内嵌（%LOCALAPPDATA%）".to_string()
    } else {
        "自动搜索/外部".to_string()
    }
}

// ---------------------------------------------------------------- DTOs

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    config_dir: String,
    config_path: String,
    settings_path: String,
    seven_zip: Option<String>,
    seven_zip_version: Option<String>,
    staging_prefix: String,
    log_path: String,
    log_dir: String,
}

/// Diagnostic snapshot used by the settings page and the bug-report flow.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnostics {
    version: String,
    os: String,
    arch: String,
    config_dir: String,
    settings_path: String,
    seven_zip: String,
    seven_zip_version: String,
    log_path: String,
    log_dir: String,
    processed_path: String,
    processed_count: usize,
    /// Only the number of remembered passwords, never their values.
    password_count: usize,
    staging_prefix: String,
    settings_json: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchState {
    running: bool,
    paused: bool,
    summary: Option<Summary>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    pub files: Vec<String>,
    pub password: Option<String>,
    pub remember_password: bool,
    pub settings: Settings,
}

// ---------------------------------------------------------------- commands

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    let (seven_zip, seven_zip_version) = {
        let current = lock(&state.sevenzip);
        match current.as_ref() {
            Some(info) => (Some(display_path(&info.exe)), Some(info.version.clone())),
            None => (None, None),
        }
    };
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        config_dir: config_dir()
            .map(|path| display_path(&path))
            .unwrap_or_default(),
        config_path: config_file_path()
            .map(|path| display_path(&path))
            .unwrap_or_default(),
        settings_path: Settings::path()
            .map(|path| display_path(&path))
            .unwrap_or_default(),
        seven_zip,
        seven_zip_version,
        staging_prefix: TMP_PREFIX.to_string(),
        log_path: display_path(&log_file_path()),
        log_dir: display_path(&log_dir()),
    }
}

#[tauri::command]
fn startup_paths(state: State<'_, AppState>) -> Vec<String> {
    lock(&state.startup_paths).clone()
}

/// Expand dropped paths (files + folders) into the queue list. The configured
/// output root is excluded so a second pass never picks up what it extracted.
#[tauri::command]
fn expand_paths(state: State<'_, AppState>, paths: Vec<String>) -> Vec<String> {
    let exclude = lock(&state.settings).output_root.clone();
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    expand_inputs(&inputs, true, exclude.as_deref())
        .into_iter()
        .map(|path| display_path(&path))
        .collect()
}

#[tauri::command]
async fn inspect_file(state: State<'_, AppState>, path: String) -> Result<Detection, String> {
    let exe = {
        let current = lock(&state.sevenzip).clone();
        match current {
            Some(info) => info.exe,
            None => {
                let info = locate_sevenzip()?;
                let exe = info.exe.clone();
                *lock(&state.sevenzip) = Some(info);
                exe
            }
        }
    };
    let sz = SevenZip::new(exe);
    let cancel = CancelToken::new();
    unpack::detect(&sz, std::path::Path::new(&path), &cancel)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn start_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    req: StartRequest,
) -> Result<(), String> {
    if req.files.is_empty() {
        append_log_line("start_batch 失败：文件列表为空");
        return Err("文件列表为空，请先添加要处理的文件".into());
    }
    if state
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        append_log_line("start_batch 失败：已有任务正在运行");
        return Err("已有任务正在运行".into());
    }
    let guard = RunningGuard::new(&state.running);
    append_log_line(&format!(
        "start_batch：收到 {} 个路径（进程启动后 {} ms）",
        req.files.len(),
        state.started_at.elapsed().as_millis()
    ));

    // Persist the settings the frontend sent with this run. Best effort: a
    // read-only config dir must not make the batch impossible to start.
    {
        let mut settings = lock(&state.settings);
        *settings = req.settings.clone();
        if let Err(error) = settings.save() {
            eprintln!("[upzip-gui] 启动前保存设置失败：{error}");
            append_log_line(&format!("启动前保存设置失败：{error}"));
        }
    }

    let sevenzip = {
        let current = lock(&state.sevenzip).clone();
        match current {
            Some(info) => info,
            None => match locate_sevenzip() {
                Ok(info) => {
                    *lock(&state.sevenzip) = Some(info.clone());
                    info
                }
                Err(error) => {
                    append_log_line(&format!("无法定位 7-Zip：{error}"));
                    return Err(error);
                }
            },
        }
    };
    append_log_line(&format!(
        "7-Zip：{}（版本 {}，来源 {}）",
        display_path(&sevenzip.exe),
        sevenzip.version,
        seven_zip_source(&sevenzip)
    ));

    // Password pool: this run's one-off password first, then the stored list
    // (de-duplicated, so the one-off entry never appears twice).
    let one_time_password = req.password.clone().filter(|password| !password.is_empty());
    if req.remember_password {
        if let Some(password) = one_time_password.as_deref() {
            core_add_password(password).map_err(|error| format!("保存密码失败：{error}"))?;
        }
    }
    let mut passwords: Vec<String> = one_time_password.into_iter().collect();
    match core_load_passwords() {
        Ok(stored) => {
            for password in stored {
                if !passwords.contains(&password) {
                    passwords.push(password);
                }
            }
        }
        Err(error) => {
            eprintln!("[upzip-gui] 读取密码库失败：{error}");
            append_log_line(&format!("读取密码库失败：{error}"));
        }
    }
    append_log_line(&format!(
        "密码池：{} 个候选（不记录密码内容）",
        passwords.len()
    ));

    let output_root = req.settings.output_root.clone();
    let inputs: Vec<PathBuf> = req.files.iter().map(PathBuf::from).collect();
    let files = expand_inputs(&inputs, true, output_root.as_deref());
    if files.is_empty() {
        append_log_line("start_batch 失败：没有找到可处理的文件");
        return Err("没有找到可处理的文件".into());
    }
    let total = files.len();

    append_log_line(&format!("开始批量：{total} 个文件"));
    append_log_line(&format!(
        "运行设置：{}",
        serde_json::to_string(&req.settings).unwrap_or_else(|error| format!("序列化失败：{error}"))
    ));
    for path in files.iter().take(10) {
        append_log_line(&format!("  输入：{}", display_path(path)));
    }
    if files.len() > 10 {
        append_log_line(&format!("  ……其余 {} 个文件省略", files.len() - 10));
    }

    let cfg = req.settings.to_run_config(passwords, None);
    let engine = Arc::new(Engine::new(SevenZip::new(sevenzip.exe.clone()), cfg));
    let cancel = CancelToken::new();
    *lock(&state.cancel) = cancel.clone();
    state.gate.resume();
    state.paused.store(false, Ordering::SeqCst);
    *lock(&state.summary) = None;

    let gate = state.gate.clone();
    eprintln!(
        "[upzip-gui] 开始批量任务：{total} 个文件（进程启动后 {} ms）",
        state.started_at.elapsed().as_millis()
    );

    let handle = app.clone();
    let recover = app.clone();
    std::thread::Builder::new()
        .name("upzip-batch".into())
        .spawn(move || {
            // Outer safety net: even if the worker panics outside the run_batch
            // catch (e.g. while persisting state) the UI still receives done and
            // the single-flight flag is released.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_batch_thread(handle, engine, files, cancel, gate, total);
            }));
            if result.is_err() {
                append_log_line("批量任务线程 panic：已发送失败 summary 并释放运行标志");
                let state = recover.state::<AppState>();
                state.running.store(false, Ordering::SeqCst);
                state.paused.store(false, Ordering::SeqCst);
                let mut fallback = Summary::default();
                fallback.total = total;
                fallback.failed = total;
                *lock(&state.summary) = Some(fallback.clone());
                let _ = recover.emit(EVENT_DONE, &fallback);
            }
        })
        .map_err(|error| {
            append_log_line(&format!("无法启动后台线程：{error}"));
            format!("无法启动后台线程：{error}")
        })?;

    guard.disarm();
    Ok(())
}

/// Batch worker body: run the engine, persist the processed store, reset the
/// shared state and always emit `unparanoia://done`.
fn run_batch_thread(
    handle: AppHandle,
    engine: Arc<Engine>,
    files: Vec<PathBuf>,
    cancel: CancelToken,
    gate: Arc<PauseGate>,
    total: usize,
) {
    let sink: Sink = {
        let handle = handle.clone();
        Arc::new(move |event: TaskEvent| {
            append_log_line(&describe_task_event(&event));
            let _ = handle.emit(EVENT_TASK, &event);
        })
    };

    let run = {
        let engine = engine.clone();
        async move { run_batch(engine, files, cancel, gate, sink).await }
    };
    // The engine only reports a cancelled task; a panic must still end the batch.
    let summary = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tauri::async_runtime::block_on(run)
    })) {
        Ok(summary) => summary,
        Err(_) => {
            append_log_line("批量任务异常中止（内部错误），部分文件可能未处理");
            let _ = handle.emit(
                EVENT_TASK,
                &TaskEvent::Log {
                    line: "批量任务异常中止（内部错误），部分文件可能未处理".to_string(),
                },
            );
            let mut fallback = Summary::default();
            fallback.total = total;
            fallback.failed = total;
            fallback
        }
    };

    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tauri::async_runtime::block_on(engine.save_processed());
    }))
    .is_err()
    {
        append_log_line("保存 processed.json 时 panic（处理记录可能不完整）");
    }

    let state = handle.state::<AppState>();
    state.running.store(false, Ordering::SeqCst);
    state.paused.store(false, Ordering::SeqCst);
    *lock(&state.summary) = Some(summary.clone());
    append_log_line(&format!(
        "批次结束：total={} success={} skipped={} needs_password={} failed={} cancelled={} bytes={} elapsed_ms={}",
        summary.total,
        summary.success,
        summary.skipped,
        summary.needs_password,
        summary.failed,
        summary.cancelled,
        summary.bytes,
        summary.elapsed_ms
    ));
    let settings = lock(&state.settings).clone();
    if let Err(error) = settings.save() {
        eprintln!("[upzip-gui] 批量结束后保存设置失败：{error}");
        append_log_line(&format!("批量结束后保存设置失败：{error}"));
    }
    let _ = handle.emit(EVENT_DONE, &summary);
}

#[tauri::command]
fn batch_state(state: State<'_, AppState>) -> BatchState {
    let running = state.running.load(Ordering::SeqCst);
    BatchState {
        running,
        paused: running && state.paused.load(Ordering::SeqCst),
        summary: lock(&state.summary).clone(),
    }
}

#[tauri::command]
fn pause_batch(state: State<'_, AppState>) -> Result<(), String> {
    if !state.running.load(Ordering::SeqCst) {
        return Err("当前没有运行中的任务".into());
    }
    state.gate.pause();
    state.paused.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn resume_batch(state: State<'_, AppState>) -> Result<(), String> {
    state.paused.store(false, Ordering::SeqCst);
    state.gate.resume();
    Ok(())
}

#[tauri::command]
fn cancel_batch(state: State<'_, AppState>) -> Result<(), String> {
    lock(&state.cancel).cancel();
    state.paused.store(false, Ordering::SeqCst);
    // Wake workers parked on the pause gate so they observe the cancellation.
    state.gate.resume();
    Ok(())
}

#[tauri::command]
fn settings_get(state: State<'_, AppState>) -> Settings {
    lock(&state.settings).clone()
}

fn apply_window_theme(app: &AppHandle, theme: &unpack::settings::ThemeMode) {
    if let Some(win) = app.get_webview_window("main") {
        let target = match theme {
            unpack::settings::ThemeMode::Light => Some(tauri::Theme::Light),
            unpack::settings::ThemeMode::Dark => Some(tauri::Theme::Dark),
            unpack::settings::ThemeMode::System => None,
        };
        let _ = win.set_theme(target);
    }
}

#[tauri::command]
fn settings_set(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    settings
        .save()
        .map_err(|error| format!("保存设置失败：{error}"))?;
    apply_window_theme(&app, &settings.theme);
    *lock(&state.settings) = settings;
    Ok(())
}

/// Plain-text password list. Only ever returned to the local webview.
#[tauri::command]
fn list_passwords() -> Vec<String> {
    match core_load_passwords() {
        Ok(passwords) => passwords,
        Err(error) => {
            eprintln!("[upzip-gui] 读取密码库失败：{error}");
            Vec::new()
        }
    }
}

#[tauri::command]
fn add_password(password: String) -> Result<bool, String> {
    core_add_password(&password).map_err(|error| error.to_string())
}

#[tauri::command]
fn remove_password(index: usize) -> Result<bool, String> {
    core_remove_password_at(index).map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_passwords() -> Result<(), String> {
    core_clear_passwords().map_err(|error| error.to_string())
}

/// 7-Zip 许可证全文（LGPL 2.1 + unRAR 限制），设置页“查看许可证”入口使用。
#[tauri::command]
fn seven_zip_license() -> String {
    upzip_embedded_7z::LICENSE_TEXT.to_string()
}

#[tauri::command]
fn get_config_path() -> Result<String, String> {
    config_file_path()
        .map(|path| display_path(&path))
        .map_err(|error| error.to_string())
}
/// Append a frontend log line (already timestamped by the backend) to the
/// persistent log file.
#[tauri::command]
fn append_log(line: String) -> Result<(), String> {
    try_append_log_line(&line).map_err(|error| format!("写日志失败：{error}"))
}

/// Diagnostic snapshot for the settings page: paths, 7-Zip resolution, store
/// counts and the current settings JSON (never passwords).
#[tauri::command]
fn diagnostics(state: State<'_, AppState>) -> Diagnostics {
    let (seven_zip, seven_zip_version) = {
        let current = lock(&state.sevenzip).clone();
        match current {
            Some(info) => (display_path(&info.exe), info.version),
            None => match locate_sevenzip() {
                Ok(info) => {
                    let exe = display_path(&info.exe);
                    let version = info.version.clone();
                    *lock(&state.sevenzip) = Some(info);
                    (exe, version)
                }
                Err(error) => {
                    append_log_line(&format!("诊断：无法定位 7-Zip：{error}"));
                    (String::new(), format!("未找到：{error}"))
                }
            },
        }
    };
    let processed = ProcessedStore::load(ProcessedStore::default_path());
    let processed_path = display_path(processed.path());
    let processed_count = processed.count();
    let password_count = core_load_passwords()
        .map(|passwords| passwords.len())
        .unwrap_or(0);
    let settings_json = match serde_json::to_string_pretty(&*lock(&state.settings)) {
        Ok(json) => json,
        Err(error) => format!("序列化设置失败：{error}"),
    };
    let snapshot = Diagnostics {
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        config_dir: config_dir()
            .map(|path| display_path(&path))
            .unwrap_or_default(),
        settings_path: Settings::path()
            .map(|path| display_path(&path))
            .unwrap_or_default(),
        seven_zip,
        seven_zip_version,
        log_path: display_path(&log_file_path()),
        log_dir: display_path(&log_dir()),
        processed_path,
        processed_count,
        password_count,
        staging_prefix: TMP_PREFIX.to_string(),
        settings_json,
    };
    append_log_line(&format!(
        "诊断信息：version={} os={} arch={} config_dir={} settings={} seven_zip={} seven_zip_version={} log={} processed={}（{} 条）passwords={} 条 staging_prefix={}",
        snapshot.version,
        snapshot.os,
        snapshot.arch,
        snapshot.config_dir,
        snapshot.settings_path,
        if snapshot.seven_zip.is_empty() {
            "-"
        } else {
            snapshot.seven_zip.as_str()
        },
        snapshot.seven_zip_version,
        snapshot.log_path,
        snapshot.processed_path,
        snapshot.processed_count,
        snapshot.password_count,
        snapshot.staging_prefix
    ));
    snapshot
}

#[tauri::command]
async fn pick_input_files(app: AppHandle) -> Vec<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择要处理的文件")
        .add_filter(
            "支持的格式（加密文件 / 压缩包 / 伪装容器）",
            &[
                "enc", "zip", "7z", "rar", "tar", "gz", "tgz", "xz", "bz2", "zst", "iso", "cab",
                "exe", "mp4",
            ],
        )
        .pick_files(move |paths| {
            let _ = tx.send(paths);
        });
    match rx.await {
        Ok(Some(paths)) => paths
            .into_iter()
            .filter_map(|path| path.into_path().ok())
            .map(|path| display_path(&path))
            .collect(),
        _ => Vec::new(),
    }
}

#[tauri::command]
async fn pick_output_dir(app: AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择输出目录")
        .pick_folder(move |folder| {
            let _ = tx.send(folder);
        });
    match rx.await {
        Ok(Some(folder)) => folder.into_path().ok().map(|path| display_path(&path)),
        _ => None,
    }
}

#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("路径为空".into());
    }
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("路径不存在：{path}"));
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        if !target.is_dir() {
            command.arg("-R");
        }
        let status = command
            .arg(&target)
            .status()
            .map_err(|error| format!("无法打开 Finder：{error}"))?;
        if !status.success() {
            return Err(format!("打开 Finder 失败（退出码 {status}）"));
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        // explorer /select returns 1 even on success; only a spawn failure is fatal.
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", target.display()))
            .status()
            .map_err(|error| format!("无法打开资源管理器：{error}"))?;
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if target.is_dir() {
            target
        } else {
            target
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or(target)
        };
        let status = std::process::Command::new("xdg-open")
            .arg(&dir)
            .status()
            .map_err(|error| format!("无法打开文件管理器：{error}"))?;
        if !status.success() {
            return Err(format!("打开文件管理器失败（退出码 {status}）"));
        }
        Ok(())
    }
}

/// Remove leftover staging folders (`.unparanoia-tmp-*`) under the given roots.
#[tauri::command]
fn cleanup_staging(roots: Vec<String>) -> Result<usize, String> {
    // Empty list = "clean whatever the app may have left behind": the system
    // temp dir. The frontend normally passes the configured output root.
    let roots = if roots.is_empty() {
        vec![std::env::temp_dir().to_string_lossy().into_owned()]
    } else {
        roots
    };
    let mut removed = 0usize;
    let mut scanned = 0usize;
    for raw in roots {
        let dir = PathBuf::from(&raw);
        if !dir.is_dir() {
            continue;
        }
        scanned += 1;
        let entries =
            std::fs::read_dir(&dir).map_err(|error| format!("无法读取目录 {raw}：{error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("无法读取目录 {raw}：{error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(TMP_PREFIX) {
                continue;
            }
            let path = entry.path();
            let ok = if path.is_dir() {
                std::fs::remove_dir_all(&path).is_ok()
            } else {
                std::fs::remove_file(&path).is_ok()
            };
            if ok {
                removed += 1;
            }
        }
    }
    if scanned == 0 {
        return Err("指定的目录不存在".into());
    }
    Ok(removed)
}

fn count_processed_records(value: &serde_json::Value) -> usize {
    fn container_len(value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::Array(items) => items.len(),
            serde_json::Value::Object(map) => map.len(),
            _ => 0,
        }
    }
    match value {
        serde_json::Value::Object(map) => {
            // Tolerate both {"<fingerprint>": {...}} and a wrapper such as
            // {"version": 1, "entries": {...}}.
            for key in ["entries", "records", "processed", "items", "files"] {
                if let Some(inner) = map.get(key) {
                    if inner.is_object() || inner.is_array() {
                        return container_len(inner);
                    }
                }
            }
            map.len()
        }
        other => container_len(other),
    }
}

/// Delete `processed.json` (skip-processed record) and report how many records
/// it held. Returns 0 when the file does not exist.
#[tauri::command]
fn forget_processed(state: State<'_, AppState>) -> Result<usize, String> {
    if state.running.load(Ordering::SeqCst) {
        return Err("已有任务正在运行，无法清除处理记录".into());
    }
    let dir = config_dir().map_err(|error| error.to_string())?;
    let path = dir.join("processed.json");
    if !path.exists() {
        return Ok(0);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("读取处理记录失败：{error}"))?;
    let count = serde_json::from_str::<serde_json::Value>(&text)
        .map(|value| count_processed_records(&value))
        .unwrap_or(0);
    std::fs::remove_file(&path).map_err(|error| format!("删除处理记录失败：{error}"))?;
    eprintln!("[upzip-gui] 已清除处理记录 {count} 条");
    Ok(count)
}

// ---------------------------------------------------------------- bootstrap

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let sevenzip = match locate_sevenzip() {
                Ok(info) => Some(info),
                Err(error) => {
                    eprintln!("[upzip-gui] {error}");
                    None
                }
            };
            let settings = Settings::load();
            // Best-effort write-back: persists the v0.3.2 settings migration
            // (settingsVersion) on first launch. A read-only config dir must not
            // prevent the window from opening, so failures only go to stderr.
            match settings.save() {
                Ok(()) => {}
                Err(error) => eprintln!("[upzip-gui] 启动时保存设置失败：{error}"),
            }
            if let Err(error) = std::fs::create_dir_all(log_dir()) {
                eprintln!("[upzip-gui] 无法创建日志目录：{error}");
            }
            // Cold-start "Open With" arguments. Tauri/macOS injects flags such
            // as `-psn_0_...`; anything starting with '-' is not a file.
            let startup_paths: Vec<String> = std::env::args()
                .skip(1)
                .filter(|arg| !arg.starts_with('-'))
                .filter(|arg| std::path::Path::new(arg).exists())
                .collect();
            append_log_line(&format!(
                "应用启动：版本 {}，os {}，arch {}，pid {}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                std::process::id()
            ));
            append_log_line(&format!(
                "配置：config_dir={}，settings={}，日志={}",
                config_dir()
                    .map(|path| display_path(&path))
                    .unwrap_or_default(),
                Settings::path()
                    .map(|path| display_path(&path))
                    .unwrap_or_default(),
                display_path(&log_file_path())
            ));
            match &sevenzip {
                Some(info) => append_log_line(&format!(
                    "7-Zip：{}（版本 {}，来源 {}）",
                    display_path(&info.exe),
                    info.version,
                    seven_zip_source(info)
                )),
                None => append_log_line("7-Zip：启动时未找到（首次批量任务时会重试）"),
            }
            append_log_line(&format!("启动文件参数：{} 个", startup_paths.len()));
            apply_window_theme(app.handle(), &settings.theme);
            app.manage(AppState::new(sevenzip, settings, startup_paths));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            startup_paths,
            expand_paths,
            inspect_file,
            start_batch,
            batch_state,
            pause_batch,
            resume_batch,
            cancel_batch,
            settings_get,
            settings_set,
            list_passwords,
            add_password,
            remove_password,
            clear_passwords,
            seven_zip_license,
            get_config_path,
            pick_input_files,
            pick_output_dir,
            reveal_path,
            cleanup_staging,
            forget_processed,
            append_log,
            diagnostics,
        ]);

    if let Err(error) = builder.run(tauri::generate_context!()) {
        eprintln!("[upzip-gui] 应用启动失败：{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_epoch_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn shorten_only_appends_ellipsis_when_cut() {
        assert_eq!(shorten("abc", 5), "abc");
        assert_eq!(shorten("abcdef", 3), "abc…");
    }
}
