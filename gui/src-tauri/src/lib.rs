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
//! The password store is kept in plain text by `unparanoia_core::config`
//! (0600 on unix). Passwords are never logged.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use unparanoia_core::config::{
    add_password as core_add_password, clear_passwords as core_clear_passwords, config_dir,
    config_file_path, load_passwords as core_load_passwords,
    remove_password_at as core_remove_password_at,
};
use unparanoia_unpack_core as unpack;
use unparanoia_unpack_core::{
    expand_inputs, run_batch, CancelToken, Detection, Engine, PauseGate, Settings, SevenZip, Sink,
    Summary, TaskEvent, TMP_PREFIX,
};

pub const EVENT_TASK: &str = "unparanoia://event";
pub const EVENT_DONE: &str = "unparanoia://done";

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
    let sz = SevenZip::locate(None).map_err(|error| {
        format!(
            "找不到 7-Zip（{error}）。请安装 7-Zip，或设置环境变量 UNPARANOIA_7Z 指向 7zz/7z.exe，\
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
        return Err("文件列表为空，请先添加要处理的文件".into());
    }
    if state
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("已有任务正在运行".into());
    }
    let guard = RunningGuard::new(&state.running);

    // Persist the settings the frontend sent with this run. Best effort: a
    // read-only config dir must not make the batch impossible to start.
    {
        let mut settings = lock(&state.settings);
        *settings = req.settings.clone();
        if let Err(error) = settings.save() {
            eprintln!("[unparanoia-gui] 启动前保存设置失败：{error}");
        }
    }

    let sevenzip = {
        let current = lock(&state.sevenzip).clone();
        match current {
            Some(info) => info,
            None => {
                let info = locate_sevenzip()?;
                *lock(&state.sevenzip) = Some(info.clone());
                info
            }
        }
    };

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
        Err(error) => eprintln!("[unparanoia-gui] 读取密码库失败：{error}"),
    }

    let output_root = req.settings.output_root.clone();
    let inputs: Vec<PathBuf> = req.files.iter().map(PathBuf::from).collect();
    let files = expand_inputs(&inputs, true, output_root.as_deref());
    if files.is_empty() {
        return Err("没有找到可处理的文件".into());
    }
    let total = files.len();

    let cfg = req.settings.to_run_config(passwords, None);
    let engine = Arc::new(Engine::new(SevenZip::new(sevenzip.exe.clone()), cfg));
    let cancel = CancelToken::new();
    *lock(&state.cancel) = cancel.clone();
    state.gate.resume();
    state.paused.store(false, Ordering::SeqCst);
    *lock(&state.summary) = None;

    let gate = state.gate.clone();
    eprintln!(
        "[unparanoia-gui] 开始批量任务：{total} 个文件（进程启动后 {} ms）",
        state.started_at.elapsed().as_millis()
    );

    let handle = app.clone();
    let recover = app.clone();
    std::thread::Builder::new()
        .name("unparanoia-batch".into())
        .spawn(move || {
            // Outer safety net: even if the worker panics outside the run_batch
            // catch (e.g. while persisting state) the UI still receives done and
            // the single-flight flag is released.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_batch_thread(handle, engine, files, cancel, gate, total);
            }));
            if result.is_err() {
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
        .map_err(|error| format!("无法启动后台线程：{error}"))?;

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

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tauri::async_runtime::block_on(engine.save_processed());
    }));

    let state = handle.state::<AppState>();
    state.running.store(false, Ordering::SeqCst);
    state.paused.store(false, Ordering::SeqCst);
    *lock(&state.summary) = Some(summary.clone());
    let settings = lock(&state.settings).clone();
    if let Err(error) = settings.save() {
        eprintln!("[unparanoia-gui] 批量结束后保存设置失败：{error}");
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

#[tauri::command]
fn settings_set(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    settings
        .save()
        .map_err(|error| format!("保存设置失败：{error}"))?;
    *lock(&state.settings) = settings;
    Ok(())
}

/// Plain-text password list. Only ever returned to the local webview.
#[tauri::command]
fn list_passwords() -> Vec<String> {
    match core_load_passwords() {
        Ok(passwords) => passwords,
        Err(error) => {
            eprintln!("[unparanoia-gui] 读取密码库失败：{error}");
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

#[tauri::command]
fn get_config_path() -> Result<String, String> {
    config_file_path()
        .map(|path| display_path(&path))
        .map_err(|error| error.to_string())
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
    eprintln!("[unparanoia-gui] 已清除处理记录 {count} 条");
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
                    eprintln!("[unparanoia-gui] {error}");
                    None
                }
            };
            let settings = Settings::load();
            // Cold-start "Open With" arguments. Tauri/macOS injects flags such
            // as `-psn_0_...`; anything starting with '-' is not a file.
            let startup_paths: Vec<String> = std::env::args()
                .skip(1)
                .filter(|arg| !arg.starts_with('-'))
                .filter(|arg| std::path::Path::new(arg).exists())
                .collect();
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
            get_config_path,
            pick_input_files,
            pick_output_dir,
            reveal_path,
            cleanup_staging,
            forget_processed,
        ]);

    if let Err(error) = builder.run(tauri::generate_context!()) {
        eprintln!("[unparanoia-gui] 应用启动失败：{error}");
        std::process::exit(1);
    }
}
