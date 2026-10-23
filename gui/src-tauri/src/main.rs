//! Paranoia 解密工具 GUI 入口。
//!
//! 前端是纯静态 HTML/CSS/JS（withGlobalTauri），所有文件与密码操作都通过本文件的
//! Tauri 命令完成；解密逻辑全部复用 paranoia-enc-core。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use std::path::PathBuf;

use state::{AppState, AutotestConfig};
use tauri::Manager;

/// 自动验证参数：--autotest-spec <spec.json> --autotest-report <report.json>
/// 也支持环境变量 PARANOIA_ENC_AUTOTEST_SPEC / PARANOIA_ENC_AUTOTEST_REPORT。
fn parse_startup_options() -> Option<AutotestConfig> {
    let mut spec_path: Option<String> = std::env::var("PARANOIA_ENC_AUTOTEST_SPEC").ok();
    let mut report_path: Option<String> = std::env::var("PARANOIA_ENC_AUTOTEST_REPORT").ok();

    let arguments: Vec<String> = std::env::args().collect();
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--autotest-spec" => {
                if let Some(value) = arguments.get(index + 1) {
                    spec_path = Some(value.clone());
                    index += 1;
                }
            }
            "--autotest-report" => {
                if let Some(value) = arguments.get(index + 1) {
                    report_path = Some(value.clone());
                    index += 1;
                }
            }
            _ => {}
        }
        index += 1;
    }

    match (spec_path, report_path) {
        (Some(spec_path), Some(report_path)) => {
            let spec_json = match std::fs::read_to_string(&spec_path) {
                Ok(contents) => contents,
                Err(error) => {
                    eprintln!("[paranoia-gui] 读取自动验证 spec 失败（{spec_path}）：{error}");
                    String::new()
                }
            };
            Some(AutotestConfig {
                spec_json,
                report_path: PathBuf::from(report_path),
            })
        }
        (None, None) => None,
        _ => {
            eprintln!("[paranoia-gui] 自动验证需要同时提供 --autotest-spec 与 --autotest-report");
            None
        }
    }
}

fn main() {
    let autotest = parse_startup_options();
    let app_state = AppState::new(autotest);
    app_state.log("进程启动");

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_initial_state,
            commands::ui_ready,
            commands::validate_paths,
            commands::pick_input_files,
            commands::pick_output_dir,
            commands::get_default_output_dir,
            commands::set_remembered_password,
            commands::clear_remembered_password,
            commands::start_batch,
            commands::reveal_path,
            commands::autotest_finish,
            commands::autotest_exit,
        ])
        .setup(|app| {
            app.state::<AppState>().log("窗口已创建（setup）");
            Ok(())
        });

    if let Err(error) = builder.run(tauri::generate_context!()) {
        eprintln!("[paranoia-gui] 应用启动失败：{error}");
        std::process::exit(1);
    }
}
