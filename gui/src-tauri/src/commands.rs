//! Tauri 命令：文件校验、原生文件/目录选择、密码记忆、批量解密与自动验证报告。

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use paranoia_enc_core::{
    clear_remembered_password as core_clear_remembered_password, config_file_path,
    default_output_dir, load_remembered_password,
    run_batch,
    save_remembered_password as core_save_remembered_password, validate_input_path, BatchOptions,
    BatchProgress, BatchSummary, ConflictStrategy, FileOutcome,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::state::AppState;

const EVENT_BATCH_PROGRESS: &str = "batch-progress";
const EVENT_FILE_FINISHED: &str = "file-finished";
const EVENT_BATCH_FINISHED: &str = "batch-finished";
/// 进度事件节流间隔：约 20 次/秒。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/* ------------------------------------------------------------------ */
/* 数据契约                                                            */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutotestInfo {
    pub spec_json: String,
    pub report_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialState {
    pub remembered_password: Option<String>,
    pub config_path: Option<String>,
    pub app_version: String,
    /// 从进程启动到本命令执行的毫秒数（启动耗时参考值）。
    pub startup_ms: u64,
    pub autotest: Option<AutotestInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathValidation {
    pub path: String,
    pub accepted: bool,
    pub canonical_path: Option<String>,
    pub size: u64,
    pub file_name: String,
    pub reason: Option<String>,
    pub reason_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartBatchRequest {
    pub files: Vec<String>,
    pub password: String,
    pub output_dir: Option<String>,
    pub conflict: String,
    pub remember_password: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFinishedPayload {
    pub ok: bool,
    pub input: String,
    pub outputs: Vec<String>,
    pub original_name: Option<String>,
    pub bytes_written: u64,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
}

impl FileFinishedPayload {
    fn from_outcome(outcome: FileOutcome) -> Self {
        Self {
            ok: outcome.error_kind.is_none(),
            input: display_path(&outcome.input),
            outputs: outcome.outputs.iter().map(|path| display_path(path)).collect(),
            original_name: outcome.original_name,
            bytes_written: outcome.bytes_written,
            error_kind: outcome.error_kind,
            error_message: outcome.error_message,
        }
    }

    fn provisional(input: &Path, ok: bool) -> Self {
        Self {
            ok,
            input: display_path(input),
            outputs: Vec::new(),
            original_name: None,
            bytes_written: 0,
            error_kind: None,
            error_message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFinishedPayload {
    pub outcomes: Vec<FileFinishedPayload>,
    pub successes: usize,
    pub failures: usize,
    pub output_dir: String,
}

/* ------------------------------------------------------------------ */
/* 事件发送（带节流）                                                  */
/* ------------------------------------------------------------------ */

struct BatchEmitter {
    app: AppHandle,
    last_emit: Instant,
    emitted_once: bool,
    last_finished_index: Option<usize>,
    successes_seen: usize,
    failures_seen: usize,
}

impl BatchEmitter {
    fn new(app: AppHandle) -> Self {
        Self {
            app,
            last_emit: Instant::now(),
            emitted_once: false,
            last_finished_index: None,
            successes_seen: 0,
            failures_seen: 0,
        }
    }

    fn emit<T: Serialize + Clone>(&self, event: &str, payload: &T) {
        if let Err(error) = self.app.emit(event, payload.clone()) {
            self.app
                .state::<AppState>()
                .log(&format!("发送事件 {event} 失败：{error}"));
        }
    }

    fn on_progress(&mut self, progress: BatchProgress) {
        // 每个文件结束时立刻发一次 file-finished（此时只知道成功/失败，输出细节由 batch-finished 汇总补齐）。
        //
        // 注意：core 在"最后一个分块已处理完、但结果尚未统计"时也会发一次 file_process == file_total
        // 的进度事件。此时 successes/failures 还是旧值，若据此判定 ok 会把成功的文件瞬时报成失败
        // （用户看到的"闪一下失败"）。因此只有成功/失败计数真的变化时才发 file-finished 事件。
        let file_done = progress.file_total > 0 && progress.file_processed >= progress.file_total;
        let counters_changed =
            progress.successes > self.successes_seen || progress.failures > self.failures_seen;
        if file_done && counters_changed && self.last_finished_index != Some(progress.index) {
            self.last_finished_index = Some(progress.index);
            let ok = progress.successes > self.successes_seen;
            self.successes_seen = progress.successes;
            self.failures_seen = progress.failures;
            self.emit(
                EVENT_FILE_FINISHED,
                &FileFinishedPayload::provisional(&progress.current_file, ok),
            );
        }

        // 节流到约 20 次/秒；首帧一定发送。
        let now = Instant::now();
        if !self.emitted_once || now.saturating_duration_since(self.last_emit) >= PROGRESS_INTERVAL {
            self.emitted_once = true;
            self.last_emit = now;
            self.emit(EVENT_BATCH_PROGRESS, &progress);
        }
    }
}

/* ------------------------------------------------------------------ */
/* 中文文案                                                            */
/* ------------------------------------------------------------------ */

fn add_error_text(kind: &str) -> &'static str {
    match kind {
        "notEncExtension" => "不是 .enc 文件",
        "cannotRead" => "无法读取文件",
        "notRegularFile" => "不是普通文件",
        "tooShort" => "文件过短",
        "badMagic" => "不是 Paranoia 加密文件",
        "unsupportedVersion" => "不支持的格式版本",
        "unsupportedAlgorithm" => "不是 AES-256 加密文件",
        "invalidCustomParams" => "不支持的 KDF 参数",
        "notFound" => "文件不存在",
        _ => "文件不可用",
    }
}

fn config_error_message(error: &paranoia_enc_core::ConfigError) -> String {
    match error {
        paranoia_enc_core::ConfigError::NoConfigDir => "无法确定用户配置目录".to_string(),
        other => format!("读写配置文件失败：{other}"),
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn parse_conflict(value: &str) -> Result<ConflictStrategy, String> {
    match value {
        "overwrite" => Ok(ConflictStrategy::Overwrite),
        "rename" | "" => Ok(ConflictStrategy::Rename),
        "error" => Ok(ConflictStrategy::Error),
        other => Err(format!("未知的冲突策略：{other}")),
    }
}

/* ------------------------------------------------------------------ */
/* 命令                                                                */
/* ------------------------------------------------------------------ */

#[tauri::command]
pub fn get_initial_state(state: State<'_, AppState>) -> Result<InitialState, String> {
    let remembered_password = match load_remembered_password() {
        Ok(password) => password,
        Err(error) => {
            state.log(&format!("读取记忆密码失败：{error}"));
            None
        }
    };
    let config_path = config_file_path().ok().map(|path| display_path(&path));
    let autotest = state.autotest.as_ref().map(|config| AutotestInfo {
        spec_json: config.spec_json.clone(),
        report_path: display_path(&config.report_path),
    });
    let startup_ms = state.elapsed_ms();
    state.log(&format!("get_initial_state（启动后 {startup_ms} ms，记忆密码 {}）",
        if remembered_password.is_some() { "存在" } else { "不存在" }));
    Ok(InitialState {
        remembered_password,
        config_path,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        startup_ms,
        autotest,
    })
}

/// 前端完成初始化后调用；用于启动耗时诊断。
#[tauri::command]
pub fn ui_ready(state: State<'_, AppState>) {
    state.mark_ui_ready();
}

#[tauri::command]
pub fn validate_paths(paths: Vec<String>) -> Vec<PathValidation> {
    paths.into_iter().map(|path| validate_one(&path)).collect()
}

fn validate_one(raw: &str) -> PathValidation {
    let mut validation = PathValidation {
        path: raw.to_string(),
        accepted: false,
        canonical_path: None,
        size: 0,
        file_name: String::new(),
        reason: None,
        reason_message: None,
    };
    if raw.trim().is_empty() {
        validation.reason = Some("emptyPath".to_string());
        validation.reason_message = Some("路径为空".to_string());
        return validation;
    }

    let path = PathBuf::from(raw);
    validation.file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Ok(metadata) = std::fs::metadata(&path) else {
        validation.reason = Some("notFound".to_string());
        validation.reason_message = Some("文件不存在或无法访问".to_string());
        return validation;
    };
    validation.size = metadata.len();
    if !metadata.is_file() {
        validation.reason = Some("notRegularFile".to_string());
        validation.reason_message = Some(add_error_text("notRegularFile").to_string());
        return validation;
    }

    match validate_input_path(&path) {
        Ok(_header) => {
            validation.accepted = true;
            validation.canonical_path = Some(display_path(
                &std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()),
            ));
        }
        Err(error) => {
            let kind = error.kind();
            validation.reason = Some(kind.to_string());
            validation.reason_message = Some(add_error_text(kind).to_string());
        }
    }
    validation
}

#[tauri::command]
pub async fn pick_input_files(app: AppHandle) -> Result<Vec<String>, String> {
    let picked = app
        .dialog()
        .file()
        .set_title("选择要解密的 .enc 文件")
        .add_filter("Paranoia 加密文件", &["enc"])
        .blocking_pick_files();
    let mut paths = Vec::new();
    for file_path in picked.unwrap_or_default() {
        match file_path.into_path() {
            Ok(path) => paths.push(display_path(&path)),
            Err(error) => eprintln!("[paranoia-gui] 忽略无法解析的文件路径：{error}"),
        }
    }
    Ok(paths)
}

#[tauri::command]
pub async fn pick_output_dir(
    app: AppHandle,
    current: Option<String>,
) -> Result<Option<String>, String> {
    let mut builder = app.dialog().file().set_title("选择输出目录");
    if let Some(directory) = current.as_deref().filter(|value| !value.is_empty()) {
        builder = builder.set_directory(directory);
    }
    match builder.blocking_pick_folder() {
        Some(folder) => match folder.into_path() {
            Ok(path) => Ok(Some(display_path(&path))),
            Err(error) => Err(format!("无效的目录路径：{error}")),
        },
        None => Ok(None),
    }
}

#[tauri::command]
pub fn get_default_output_dir(paths: Vec<String>) -> Result<Option<String>, String> {
    let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    Ok(default_output_dir(&files).map(|path| display_path(&path)))
}

#[tauri::command]
pub fn set_remembered_password(password: String) -> Result<(), String> {
    if password.is_empty() {
        return Err("密码为空，未保存".to_string());
    }
    core_save_remembered_password(&password)
        .map_err(|error| format!("保存记忆密码失败：{}", config_error_message(&error)))
}

#[tauri::command]
pub fn clear_remembered_password() -> Result<(), String> {
    core_clear_remembered_password()
        .map_err(|error| format!("清除记忆密码失败：{}", config_error_message(&error)))
}

#[tauri::command]
pub fn start_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartBatchRequest,
) -> Result<(), String> {
    if request.files.is_empty() {
        return Err("文件列表为空，请先添加 .enc 文件".to_string());
    }
    if request.password.is_empty() {
        return Err("请输入解密密码".to_string());
    }
    let conflict = parse_conflict(&request.conflict)?;

    let mut files = Vec::with_capacity(request.files.len());
    for raw in &request.files {
        let path = PathBuf::from(raw);
        if let Err(error) = validate_input_path(&path) {
            return Err(format!(
                "文件不可用：{}（{}）",
                raw,
                add_error_text(error.kind())
            ));
        }
        files.push(path);
    }

    if state
        .running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("已有解密任务正在运行".to_string());
    }

    if request.remember_password {
        match core_save_remembered_password(&request.password) {
            Ok(()) => state.log("已保存记忆密码"),
            Err(error) => state.log(&format!("保存记忆密码失败：{error}")),
        }
    }

    let output_dir = request
        .output_dir
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(directory) = output_dir.as_ref() {
        if let Err(error) = std::fs::create_dir_all(directory) {
            state.running.store(false, Ordering::SeqCst);
            return Err(format!("无法创建输出目录：{error}"));
        }
    }

    let total = files.len();
    let options = BatchOptions {
        files,
        password: request.password,
        output_dir,
        conflict,
    };
    state.log(&format!("开始批量解密：{total} 个文件，冲突策略 {conflict:?}"));

    let running_flag = state.running.clone();
    let app_handle = app.clone();
    let spawn_result = std::thread::Builder::new()
        .name("paranoia-decrypt-batch".to_string())
        .spawn(move || {
            let _guard = RunningGuard(running_flag);
            let app_state = app_handle.state::<AppState>();
            let mut emitter = BatchEmitter::new(app_handle.clone());
            let summary = run_batch(&options, &mut |progress| emitter.on_progress(progress));
            let BatchSummary {
                outcomes,
                successes,
                failures,
                output_dir,
            } = summary;
            let payload = BatchFinishedPayload {
                outcomes: outcomes
                    .into_iter()
                    .map(FileFinishedPayload::from_outcome)
                    .collect(),
                successes,
                failures,
                output_dir: display_path(&output_dir),
            };
            app_state.log(&format!(
                "批量解密完成：成功 {successes}，失败 {failures}，输出目录 {}",
                payload.output_dir
            ));
            emitter.emit(EVENT_BATCH_FINISHED, &payload);
        });

    if let Err(error) = spawn_result {
        state.running.store(false, Ordering::SeqCst);
        return Err(format!("无法启动解密线程：{error}"));
    }
    Ok(())
}

/// 线程退出（包括意外 panic）时释放运行标志，避免界面永远停在“解密中”。
struct RunningGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("路径为空".to_string());
    }
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("路径不存在：{path}"));
    }
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");

    let status = command.arg(&target).status();
    match status {
        Ok(exit) if exit.success() => Ok(()),
        Ok(exit) => Err(format!("打开目录失败（退出码 {exit}）")),
        Err(error) => Err(format!("无法打开目录：{error}")),
    }
}

/// 自动验证模式专用：写入报告后退出应用。
#[tauri::command]
pub fn autotest_finish(app: AppHandle, report_json: String) -> Result<(), String> {
    let config = {
        let state = app.state::<AppState>();
        state
            .autotest
            .clone()
            .ok_or_else(|| "当前不是自动验证模式".to_string())?
    };
    let report_path = config.report_path;
    if !report_path.is_absolute() {
        return Err("自动验证报告路径必须是绝对路径".to_string());
    }
    let value: serde_json::Value =
        serde_json::from_str(&report_json).map_err(|error| format!("报告 JSON 解析失败：{error}"))?;
    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| format!("报告序列化失败：{error}"))?;
    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建报告目录：{error}"))?;
    }
    std::fs::write(&report_path, bytes).map_err(|error| format!("写入报告失败：{error}"))?;
    app.state::<AppState>()
        .log(&format!("自动验证报告已写入 {}", display_path(&report_path)));
    schedule_autotest_exit();
    Ok(())
}

/// 自动验证模式专用：即使报告写入失败也要结束进程，避免无人值守地挂在后台。
#[tauri::command]
pub fn autotest_exit(app: AppHandle) -> Result<(), String> {
    if app.state::<AppState>().autotest.is_none() {
        return Err("当前不是自动验证模式".to_string());
    }
    schedule_autotest_exit();
    Ok(())
}

fn schedule_autotest_exit() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(700));
        std::process::exit(0);
    });
}
