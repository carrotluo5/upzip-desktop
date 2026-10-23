//! 应用运行时状态：启动计时、解密任务占用标志、自动验证配置与诊断日志。
//!
//! 这里不保存任何密码；密码只通过核心库的配置文件 API 读写。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 自动验证（端到端自测）配置，仅在通过命令行参数或环境变量启动时存在。
#[derive(Clone, Debug)]
pub struct AutotestConfig {
    /// 前端要执行的验证步骤（JSON 字符串，由前端解析）。
    pub spec_json: String,
    /// 前端把验证报告写到的路径（必须位于系统临时目录内）。
    pub report_path: PathBuf,
}

impl AutotestConfig {
    /// 与应用日志同目录的启动诊断日志。
    pub fn log_path(&self) -> PathBuf {
        self.report_path.with_extension("log")
    }
}

pub struct AppState {
    started_at: Instant,
    /// 是否有解密任务正在运行（同一时间只允许一个批次）。
    pub running: Arc<AtomicBool>,
    pub autotest: Option<AutotestConfig>,
    ui_ready: Mutex<bool>,
    log_path: Option<PathBuf>,
    log_lock: Mutex<()>,
}

impl AppState {
    pub fn new(autotest: Option<AutotestConfig>) -> Self {
        let log_path = autotest.as_ref().map(|config| config.log_path());
        Self {
            started_at: Instant::now(),
            running: Arc::new(AtomicBool::new(false)),
            autotest,
            ui_ready: Mutex::new(false),
            log_path,
            log_lock: Mutex::new(()),
        }
    }

    /// 自进程启动以来经过的毫秒数（用于启动耗时统计）。
    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }

    pub fn mark_ui_ready(&self) {
        let mut ready = self.ui_ready.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !*ready {
            *ready = true;
            self.log("前端初始化完成（ui_ready）");
        }
    }

    /// 记录一条诊断日志：始终写 stderr；自动验证模式下同时追加到报告旁边的 .log 文件。
    pub fn log(&self, message: &str) {
        eprintln!("[paranoia-gui +{}ms] {message}", self.elapsed_ms());
        let Some(path) = self.log_path.as_ref() else { return };
        let _guard = self.log_lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else { return };
        let _ = writeln!(file, "[+{}ms] {message}", self.elapsed_ms());
    }
}
