//! upzip-cli: unified batch decrypt/extract CLI.
//!
//! Legacy (0.1.1) usage is preserved:
//!   upzip-cli a.enc b.enc -p PASSWORD -o out [--json] [--inspect]
//!
//! New subcommands (a bare path list is equivalent to "run"):
//!   run PATH... [flags]                 batch decrypt/extract
//!   detect PATH... [--json]             identify kind / container / password state
//!   inspect FILE... [--json]            print the plaintext SSEFE v4 header
//!   config [--show|--path|--list-passwords|--add-password PW|--remove-password IDX]
//!          [--set-password PW|--clear-password|--clear-passwords]
//!   doctor [--sevenzip PATH]            environment self-check
//!   cleanup-staging [--root DIR]...     remove .upzip-tmp-* leftovers
//!   --licenses                          print the embedded 7-Zip license text
//!
//! Processed-fingerprint skipping is opt-in: pass `--skip-processed` to skip files
//! already recorded in processed.json; without it every run reprocesses everything.
//!
//! Exit codes: 0 all succeeded/skipped, 1 any failed or needs-password,
//! 2 usage/config/environment error, 130 cancelled with Ctrl-C.

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use clap::{Args, Parser};

use upzip_core::config::{
    add_password as add_stored_password, clear_passwords, config_dir, config_file_path,
    load_passwords, remove_password_at, save_passwords,
};
use upzip_core::{
    default_output_dir, inspect_file, parse_argon2_params, BatchSummary, FileOutcome,
};
use upzip_embedded_7z::{ensure_7z as ensure_embedded_7z, LICENSE_TEXT, SEVEN_ZIP_VERSION};
use upzip_unpack_core::{
    archive_stem, expand_inputs, run_batch, util::human_bytes, CancelToken, ConflictPolicy,
    Detection, Engine, FileKind, LayoutPolicy, PasswordState, PauseGate, ProcessedStore, RunConfig,
    Settings, SevenZip, Sink, Summary, TaskEvent, TaskOutcome, TaskStatus, MAX_NESTED_DEPTH,
    TMP_PREFIX,
};

const EXIT_OK: i32 = 0;
const EXIT_FAIL: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_CANCELLED: i32 = 130;

// ---------------------------------------------------------------------------
// entry point / dispatch
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = dispatch(&args).await;
    std::process::exit(code);
}

async fn dispatch(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]).await,
        Some("detect") => cmd_detect(&args[1..]).await,
        Some("inspect") => cmd_inspect(&args[1..]),
        Some("config") => cmd_config(&args[1..]),
        Some("doctor") => cmd_doctor(&args[1..]).await,
        Some("cleanup-staging") => cmd_cleanup(&args[1..]),
        Some("help") => {
            print_help();
            EXIT_OK
        }
        Some("-h") | Some("--help") => {
            print_help();
            EXIT_OK
        }
        Some("-V") | Some("--version") => {
            println!("upzip-cli {}", env!("CARGO_PKG_VERSION"));
            EXIT_OK
        }
        Some("--licenses") => {
            print_licenses();
            EXIT_OK
        }
        _ => cmd_legacy(args).await,
    }
}

fn print_help() {
    println!(
        "upzip-cli {} - 统一批量解密/解压 CLI",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!("用法：");
    println!("  upzip-cli [FILES...] [选项]          旧式用法：直接批量处理（等价 run）");
    println!("  upzip-cli run [PATH...] [选项]       批量解密/解压文件与目录");
    println!("  upzip-cli detect PATH... [--json]    识别类型、容器与加密状态");
    println!("  upzip-cli inspect FILE... [--json]   打印 .enc 明文头信息");
    println!("  upzip-cli config [选项]              查看/管理明文密码库（默认掩码显示）");
    println!("  upzip-cli doctor [--sevenzip PATH]   环境自检（7-Zip、配置目录、detect）");
    println!("  upzip-cli cleanup-staging [--root DIR]...");
    println!("                                              清理 .upzip-tmp-* 残留");
    println!("  upzip-cli --version");
    println!(
        "  upzip-cli --licenses                 打印内嵌 7-Zip 许可证（LGPL + unRAR 限制）"
    );
    println!();
    println!("旧式兼容选项：-p/--password、-o/--output-dir、--conflict、--json、--inspect、");
    println!("              --config-path、--clear-password");
    println!();
    println!("run 常用选项：-j/--jobs、-o/--output-dir、-p/--password（可重复）、");
    println!("  --conflict rename|skip|overwrite、--layout smart|folder|direct、");
    println!("  --no-recursive、--no-nested、--max-depth N、--keep-broken、--dry-run、");
    println!("  --force-documents、--allow-execute-sfx、--stop-on-error、--skip-processed、");
    println!("  --no-strip-junk、--try-empty-password、--json、--sevenzip PATH");
    println!();
    println!("跳过已处理：默认不跳过（每次都会重新处理）；仅当显式 --skip-processed 时");
    println!("            才按 processed.json 指纹跳过已成功处理过的文件。");
    println!();
    println!("退出码：0 成功/跳过；1 有失败或缺少密码；2 用法/配置/环境错误；130 已取消");
    println!(
        "环境变量：UPZIP_PASSWORD（密码；兼容 UNPARANOIA_PASSWORD / PARANOIA_ENC_PASSWORD）、"
    );
    println!("          UPZIP_7Z（兼容 UNPARANOIA_7Z，7-Zip 路径）、UPZIP_CONFIG_DIR（配置目录）");
    println!("各子命令详情：upzip-cli <子命令> --help");
}

fn print_licenses() {
    println!("内嵌 7-Zip {SEVEN_ZIP_VERSION} 许可证（GNU LGPL 2.1 + unRAR 限制）");
    println!("Windows 单 exe 发行版把 7z.exe / 7z.dll 打包进程序本身；macOS .app 随包提供 7zz。");
    println!();
    print!("{LICENSE_TEXT}");
    if !LICENSE_TEXT.ends_with('\n') {
        println!();
    }
}

fn argv_for(args: &[String]) -> Vec<String> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("upzip-cli".to_string());
    argv.extend(args.iter().cloned());
    argv
}

fn parse_argv<T: Parser>(argv: Vec<String>) -> Result<T, i32> {
    match T::try_parse_from(argv) {
        Ok(value) => Ok(value),
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            Err(code)
        }
    }
}

fn parse_sub<T: Parser>(args: &[String]) -> Result<T, i32> {
    parse_argv(argv_for(args))
}

fn parse_root<T: Parser>(args: &[String]) -> Result<T, i32> {
    parse_argv(argv_for(args))
}

// ---------------------------------------------------------------------------
// shared flags
// ---------------------------------------------------------------------------

#[derive(Args, Debug, Clone, Default)]
struct RunFlags {
    /// Output directory (default: directory of the first input, or settings)
    #[arg(short = 'o', long = "output-dir", value_name = "DIR")]
    output_dir: Option<PathBuf>,

    /// Number of concurrent tasks
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    jobs: Option<usize>,

    /// Conflict policy: rename | skip | overwrite (legacy "error" maps to skip)
    #[arg(long = "conflict", value_name = "POLICY")]
    conflict: Option<String>,

    /// Layout policy: smart | folder | direct
    #[arg(long = "layout", value_name = "POLICY")]
    layout: Option<String>,

    /// Do not recurse into dropped folders
    #[arg(long = "no-recursive")]
    no_recursive: bool,

    /// Disable nested archive cascade unpacking
    #[arg(long = "no-nested")]
    no_nested: bool,

    /// Maximum nested cascade depth (1-8)
    #[arg(long = "max-depth", value_name = "N")]
    max_depth: Option<usize>,

    /// Keep the staging directory when extraction fails
    #[arg(long = "keep-broken")]
    keep_broken: bool,

    /// Detect + plan only, write nothing
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Expand Office/JAR documents as archives
    #[arg(long = "force-documents")]
    force_documents: bool,

    /// Windows only: run self-extracting executables as a downgrade path
    #[arg(long = "allow-execute-sfx")]
    allow_execute_sfx: bool,

    /// Stop the batch after the first failure / missing password
    #[arg(long = "stop-on-error")]
    stop_on_error: bool,

    /// Skip files recorded in processed.json (opt-in; default: process everything)
    #[arg(long = "skip-processed")]
    skip_processed: bool,

    /// Deprecated no-op: processed-fingerprint skipping is off by default now
    #[arg(long = "no-skip-processed", hide = true)]
    no_skip_processed: bool,

    /// Keep __MACOSX / .DS_Store / ._* entries
    #[arg(long = "no-strip-junk")]
    no_strip_junk: bool,

    /// Try the empty password as a last resort
    #[arg(long = "try-empty-password")]
    try_empty_password: bool,

    /// Emit JSON on stdout (JSONL events for run, JSON objects elsewhere)
    #[arg(long = "json")]
    json: bool,

    /// Explicit 7-Zip executable (7zz / 7z.exe)
    #[arg(long = "sevenzip", value_name = "PATH")]
    sevenzip: Option<PathBuf>,
}

fn apply_run_flags(cfg: &mut RunConfig, flags: &RunFlags) -> Result<(), String> {
    if let Some(dir) = &flags.output_dir {
        cfg.output_root = Some(dir.clone());
    }
    if let Some(jobs) = flags.jobs {
        cfg.jobs = jobs.clamp(1, 8);
    }
    if let Some(value) = &flags.conflict {
        if value.eq_ignore_ascii_case("error") {
            eprintln!("警告：--conflict error 已弃用（新引擎无该策略），按 skip 处理");
        }
        cfg.conflict = parse_conflict(value)?;
    }
    if let Some(value) = &flags.layout {
        cfg.layout = parse_layout(value)?;
    }
    if let Some(depth) = flags.max_depth {
        cfg.max_depth = depth.clamp(1, MAX_NESTED_DEPTH);
    }
    if flags.no_recursive {
        cfg.recursive = false;
    }
    if flags.no_nested {
        cfg.nested = false;
    }
    // Skipping processed files is opt-in: the engine default is "process everything".
    if flags.skip_processed {
        cfg.skip_processed = true;
    }
    if flags.no_strip_junk {
        cfg.strip_junk = false;
    }
    if flags.keep_broken {
        cfg.keep_broken = true;
    }
    if flags.dry_run {
        cfg.dry_run = true;
    }
    if flags.force_documents {
        cfg.force_documents = true;
    }
    if flags.allow_execute_sfx {
        cfg.allow_execute_sfx = true;
    }
    if flags.stop_on_error {
        cfg.stop_on_error = true;
    }
    if flags.try_empty_password {
        cfg.try_empty_password = true;
    }
    Ok(())
}

fn parse_conflict(value: &str) -> Result<ConflictPolicy, String> {
    match value.to_ascii_lowercase().as_str() {
        "rename" => Ok(ConflictPolicy::Rename),
        "skip" => Ok(ConflictPolicy::Skip),
        "overwrite" => Ok(ConflictPolicy::Overwrite),
        "error" => Ok(ConflictPolicy::Skip),
        other => Err(format!(
            "无效的 --conflict 值 {other:?}（可用：rename、skip、overwrite）"
        )),
    }
}

fn parse_layout(value: &str) -> Result<LayoutPolicy, String> {
    match value.to_ascii_lowercase().as_str() {
        "smart" => Ok(LayoutPolicy::Smart),
        "folder" => Ok(LayoutPolicy::Folder),
        "direct" => Ok(LayoutPolicy::Direct),
        other => Err(format!(
            "无效的 --layout 值 {other:?}（可用：smart、folder、direct）"
        )),
    }
}

// ---------------------------------------------------------------------------
// passwords / environment helpers
// ---------------------------------------------------------------------------

fn add_password(list: &mut Vec<String>, candidate: &str) {
    if candidate.is_empty() {
        return;
    }
    if list.iter().any(|existing| existing == candidate) {
        return;
    }
    list.push(candidate.to_string());
}

/// Environment password. The new name wins; the 0.1.x name is a fallback.
fn env_password() -> Option<String> {
    for name in [
        "UPZIP_PASSWORD",
        "UNPARANOIA_PASSWORD",
        "PARANOIA_ENC_PASSWORD",
    ] {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Password candidates in priority order: repeatable `-p` > environment >
/// the whole saved password library (in stored order). Duplicates removed.
fn resolve_passwords(cli_passwords: &[String]) -> Vec<String> {
    let mut passwords = Vec::new();
    for password in cli_passwords {
        add_password(&mut passwords, password);
    }
    if let Some(password) = env_password() {
        add_password(&mut passwords, &password);
    }
    match load_passwords() {
        Ok(stored) => {
            for password in stored {
                add_password(&mut passwords, &password);
            }
        }
        Err(error) => eprintln!("警告：读取密码库失败：{error}"),
    }
    passwords
}

fn stored_password_count() -> usize {
    load_passwords().map(|list| list.len()).unwrap_or(0)
}

/// Show only a hint of the password (explicit `--list-passwords` prints the
/// plaintext for scripts).
fn mask_password(password: &str) -> String {
    let mut chars = password.chars();
    let prefix: String = chars.by_ref().take(2).collect();
    if chars.next().is_some() {
        format!("{prefix}***")
    } else {
        "***".to_string()
    }
}

fn inputs_look_encrypted(inputs: &[PathBuf]) -> bool {
    !inputs.is_empty()
        && inputs.iter().all(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("enc"))
                .unwrap_or(false)
        })
}

/// Where the resolved 7-Zip executable came from (reported by `doctor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SevenZipSource {
    Explicit,
    Embedded,
    Environment,
    External,
}

impl SevenZipSource {
    fn label(self) -> &'static str {
        match self {
            SevenZipSource::Explicit => "显式 --sevenzip",
            SevenZipSource::Embedded => "内嵌（Windows 单 exe）",
            SevenZipSource::Environment => "环境变量",
            SevenZipSource::External => "外部安装 / 应用目录",
        }
    }
}

struct SevenZipChoice {
    sevenzip: SevenZip,
    source: SevenZipSource,
}

fn build_sevenzip(explicit: Option<PathBuf>) -> Result<SevenZip, i32> {
    resolve_sevenzip(explicit)
        .map(|choice| choice.sevenzip)
        .map_err(|message| {
            eprintln!("错误：{message}");
            eprintln!(
                "提示：安装 7-Zip 后把 7zz/7z.exe 加入 PATH，或设置环境变量 UPZIP_7Z 指向可执行文件。"
            );
            EXIT_USAGE
        })
}

/// Resolve 7-Zip in priority order: explicit `--sevenzip` > `UPZIP_7Z` /
/// `UNPARANOIA_7Z` environment variable (only when it points at an existing file) >
/// the runtime embedded into the Windows single-file build (no-op on macOS/Linux) >
/// the regular `SevenZip::locate` automatic search (app dir, bundled binaries/,
/// PATH, common install dirs).
fn resolve_sevenzip(explicit: Option<PathBuf>) -> Result<SevenZipChoice, String> {
    let explicit_path = explicit.clone();

    // 1. An explicit --sevenzip must win without touching the embedded cache.
    if let Some(path) = explicit {
        return finish_sevenzip_resolution(SevenZip::locate(Some(path)), &explicit_path, None);
    }

    // 2. A valid environment variable must win over the embedded runtime, otherwise
    //    a stale/older embedded copy would silently shadow the user's 7-Zip.
    if env_sevenzip_path().is_some() {
        return finish_sevenzip_resolution(SevenZip::locate(None), &explicit_path, None);
    }

    // 3. Embedded runtime (Windows single exe), then the automatic search.
    let embedded = match ensure_embedded_7z() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("警告：内嵌 7-Zip 初始化失败（{error}），尝试外部 7-Zip…");
            None
        }
    };
    let resolved = if let Some(path) = embedded.clone() {
        match SevenZip::locate(Some(path)) {
            Ok(sevenzip) => Ok(sevenzip),
            Err(error) => {
                eprintln!("警告：内嵌 7-Zip 无法使用（{error}），尝试外部 7-Zip…");
                SevenZip::locate(None)
            }
        }
    } else {
        SevenZip::locate(None)
    };
    finish_sevenzip_resolution(resolved, &explicit_path, embedded.as_deref())
}

fn finish_sevenzip_resolution(
    resolved: upzip_unpack_core::Result<SevenZip>,
    explicit: &Option<PathBuf>,
    embedded: Option<&Path>,
) -> Result<SevenZipChoice, String> {
    resolved
        .map(|sevenzip| {
            let source = classify_sevenzip_source(&sevenzip.exe, explicit.as_deref(), embedded);
            SevenZipChoice { sevenzip, source }
        })
        .map_err(|error| error.to_string())
}

/// First non-empty `UPZIP_7Z` / `UNPARANOIA_7Z` that points at an existing file.
fn env_sevenzip_path() -> Option<PathBuf> {
    for var in ["UPZIP_7Z", "UNPARANOIA_7Z"] {
        if let Some(raw) = std::env::var_os(var).filter(|value| !value.is_empty()) {
            let path = PathBuf::from(raw);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn classify_sevenzip_source(
    exe: &Path,
    explicit: Option<&Path>,
    embedded: Option<&Path>,
) -> SevenZipSource {
    if explicit.is_some_and(|path| path == exe) {
        return SevenZipSource::Explicit;
    }
    if embedded.is_some_and(|path| path == exe) {
        return SevenZipSource::Embedded;
    }
    if env_sevenzip_path().as_deref() == Some(exe) {
        return SevenZipSource::Environment;
    }
    SevenZipSource::External
}

fn install_ctrlc(cancel: CancelToken) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("收到 Ctrl-C，正在取消（再按一次强制退出）…");
            cancel.cancel();
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(EXIT_CANCELLED);
            }
        }
    });
}

fn fmt_ms(ms: u64) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

fn kind_name(kind: &FileKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(|text| text.to_string()))
        .unwrap_or_else(|| format!("{kind:?}"))
}

fn password_state_name(state: &PasswordState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(|text| text.to_string()))
        .unwrap_or_else(|| format!("{state:?}"))
}

fn stage_label(stage: &str) -> &str {
    match stage {
        "detect" => "识别",
        "password" => "尝试密码",
        "list" => "读取目录",
        "plan" => "规划输出",
        "extract" => "解压",
        "move" => "整理结果",
        "nested" => "处理嵌套",
        "done" => "完成",
        other => other,
    }
}

fn original_name(outcome: &TaskOutcome) -> String {
    outcome
        .detection
        .as_ref()
        .and_then(|detection| detection.top_level.first().cloned())
        .unwrap_or_else(|| archive_stem(&outcome.path))
}

/// Best-effort reconstruction of the concrete output paths for the 0.1.1 JSON
/// shape: "outputs" used to be the list of extracted files, not the destination
/// folder. The engine only reports the destination root, so map its detection
/// top-level names back onto that root and fall back to the root itself.
fn legacy_outputs(outcome: &TaskOutcome) -> Vec<PathBuf> {
    let Some(dest) = &outcome.dest else {
        return Vec::new();
    };
    if dest.is_file() {
        return vec![dest.clone()];
    }
    let mut outputs: Vec<PathBuf> = Vec::new();
    if let Some(detection) = &outcome.detection {
        for name in &detection.top_level {
            let candidate = dest.join(name);
            if candidate.exists() {
                outputs.push(candidate);
            }
        }
    }
    if outputs.is_empty() && outcome.files > 0 {
        // Recursive extraction can flatten the archive (e.g. nested.enc ends up
        // as dest/hello.txt instead of dest/nested.zip), so list the concrete
        // top-level entries the engine actually produced. Only trust them when
        // their count matches the outcome, otherwise the destination is shared
        // with other tasks or pre-existing files and the list would mislead.
        if let Ok(entries) = std::fs::read_dir(dest) {
            let mut children: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect();
            children.sort();
            if children.len() == outcome.files {
                outputs.extend(children);
            }
        }
    }
    if outputs.is_empty() {
        outputs.push(dest.clone());
    }
    outputs
}

/// The engine reports a conflict Skip as Success with files == 0 and a reason
/// like "保留 N 个已存在条目".
fn is_conflict_skip(outcome: &TaskOutcome) -> bool {
    outcome
        .reason
        .as_deref()
        .map(|reason| reason.contains("保留") && reason.contains("已存在"))
        .unwrap_or(false)
}

fn legacy_conflict_target(outcome: &TaskOutcome) -> String {
    if let Some(dest) = &outcome.dest {
        if let Some(name) = outcome
            .detection
            .as_ref()
            .and_then(|detection| detection.top_level.first())
        {
            let candidate = dest.join(name);
            if candidate.exists() {
                return candidate.display().to_string();
            }
        }
        return dest.display().to_string();
    }
    outcome.path.display().to_string()
}

// ---------------------------------------------------------------------------
// legacy (0.1.1 compatible) mode
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "upzip-cli",
    version,
    about = "Decrypt Paranoia File Encryption v4 (.enc) files and unpack archives (legacy mode)"
)]
struct LegacyCli {
    /// Input files or folders (batch)
    #[arg(value_name = "FILES")]
    files: Vec<PathBuf>,

    /// Password, repeatable (also UPZIP_PASSWORD / saved password library)
    #[arg(short = 'p', long = "password", value_name = "PASSWORD")]
    passwords: Vec<String>,

    /// Only inspect the plaintext header of the given files, do not decrypt
    #[arg(long = "inspect")]
    inspect: bool,

    /// Print the password store location and exit
    #[arg(long = "config-path")]
    config_path: bool,

    /// Clear the saved password library and exit
    #[arg(long = "clear-password")]
    clear_password: bool,

    #[command(flatten)]
    flags: RunFlags,
}

async fn cmd_legacy(args: &[String]) -> i32 {
    let cli: LegacyCli = match parse_root(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };

    if cli.config_path {
        return match config_file_path() {
            Ok(path) => {
                println!("{}", path.display());
                EXIT_OK
            }
            Err(error) => {
                eprintln!("error: {error}");
                EXIT_USAGE
            }
        };
    }

    if cli.clear_password {
        return match clear_passwords() {
            Ok(()) => {
                println!("remembered password cleared");
                EXIT_OK
            }
            Err(error) => {
                eprintln!("error: {error}");
                EXIT_USAGE
            }
        };
    }

    if cli.inspect {
        return inspect_files(&cli.files, cli.flags.json);
    }

    if cli.files.is_empty() {
        eprintln!("error: no input files given");
        eprintln!("提示：运行 upzip-cli --help 查看用法");
        return EXIT_USAGE;
    }

    let passwords = resolve_passwords(&cli.passwords);
    if passwords.is_empty() && inputs_look_encrypted(&cli.files) {
        eprintln!("error: no password given (use -p/--password or UPZIP_PASSWORD)");
        return EXIT_USAGE;
    }

    let emulate_conflict_error = cli
        .flags
        .conflict
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("error"))
        .unwrap_or(false);

    let output_dir = cli
        .flags
        .output_dir
        .clone()
        .or_else(|| default_output_dir(&cli.files));

    let mut cfg = RunConfig::default();
    cfg.jobs = 1;
    cfg.skip_processed = false;
    cfg.output_root = output_dir.clone();
    cfg.passwords = passwords;
    // 0.1.1 had no processed-fingerprint store; keep legacy runs out of the
    // global processed.json so a later new-style batch or GUI run does not
    // silently skip files the user just decrypted through the legacy interface.
    let legacy_processed =
        std::env::temp_dir().join(format!("upzip-cli-legacy-{}.json", std::process::id()));
    cfg.processed_path = Some(legacy_processed.clone());
    if let Err(message) = apply_run_flags(&mut cfg, &cli.flags) {
        eprintln!("error: {message}");
        return EXIT_USAGE;
    }

    let sevenzip = match build_sevenzip(cli.flags.sevenzip.clone()) {
        Ok(sevenzip) => sevenzip,
        Err(code) => return code,
    };

    // Keep nonexistent inputs in the list so they still produce one per-file
    // failure, exactly like 0.1.1 did. 0.1.1 processed the arguments strictly in
    // command-line order, so do not let expand_inputs() re-sort the user's file
    // list: expand each directory in place and dedupe preserving first occurrence.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for path in &cli.files {
        if path.exists() {
            let expanded = expand_inputs(
                std::slice::from_ref(path),
                cfg.recursive,
                cfg.output_root.as_deref(),
            );
            for file in expanded {
                if seen.insert(file.clone()) {
                    files.push(file);
                }
            }
        } else if seen.insert(path.clone()) {
            files.push(path.clone());
        }
    }
    if files.is_empty() {
        eprintln!("error: no input files given");
        return EXIT_USAGE;
    }

    let show_progress = !cli.flags.json && files.len() == 1 && std::io::stderr().is_terminal();
    let (sink, collected) = legacy_sink(cli.flags.json, show_progress);

    let engine = Arc::new(Engine::new(sevenzip, cfg.clone()));
    let cancel = CancelToken::new();
    install_ctrlc(cancel.clone());
    let gate = Arc::new(PauseGate::new());
    let run_summary = run_batch(engine.clone(), files.clone(), cancel, gate, sink).await;
    if !cfg.dry_run && cfg.skip_processed {
        let _ = engine.save_processed().await;
    }
    let _ = std::fs::remove_file(&legacy_processed);

    let mut outcomes: Vec<TaskOutcome> = collected
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let order = |path: &PathBuf| {
        files
            .iter()
            .position(|file| file == path)
            .unwrap_or(usize::MAX)
    };
    outcomes.sort_by_key(|outcome| order(&outcome.path));

    if cli.flags.json {
        let mapped: Vec<FileOutcome> = outcomes
            .iter()
            .map(|outcome| legacy_file_outcome(outcome, emulate_conflict_error))
            .collect();
        let successes = mapped
            .iter()
            .filter(|outcome| outcome.error_kind.is_none())
            .count();
        let failures = mapped.len().saturating_sub(successes);
        let batch = BatchSummary {
            outcomes: mapped,
            successes,
            failures,
            output_dir: output_dir.clone().unwrap_or_else(|| PathBuf::from(".")),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&batch).unwrap_or_else(|_| "{}".to_string())
        );
        if run_summary.cancelled > 0 {
            return EXIT_CANCELLED;
        }
        if failures > 0 {
            return EXIT_FAIL;
        }
        return EXIT_OK;
    }

    if show_progress {
        eprintln!();
    }
    for outcome in &outcomes {
        print_legacy_outcome(outcome, emulate_conflict_error);
    }

    let failures = outcomes
        .iter()
        .filter(|outcome| effective_legacy_error(outcome, emulate_conflict_error).is_some())
        .count();
    let successes = outcomes.len().saturating_sub(failures);
    println!(
        "successes: {}, failures: {}, output dir: {}",
        successes,
        failures,
        output_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .display()
    );
    if run_summary.cancelled > 0 {
        return EXIT_CANCELLED;
    }
    if failures > 0 {
        EXIT_FAIL
    } else {
        EXIT_OK
    }
}

fn legacy_sink(json: bool, show_progress: bool) -> (Sink, Arc<Mutex<Vec<TaskOutcome>>>) {
    let outcomes = Arc::new(Mutex::new(Vec::new()));
    let collected = outcomes.clone();
    let last_percent = Arc::new(AtomicU8::new(u8::MAX));
    let sink: Sink = Arc::new(move |event: TaskEvent| match event {
        TaskEvent::TaskProgress { percent, .. } if show_progress => {
            let percent = percent.unwrap_or(0);
            if last_percent.load(Ordering::Relaxed) != percent {
                last_percent.store(percent, Ordering::Relaxed);
                eprint!("\r{percent:3}%");
                let _ = std::io::stderr().flush();
            }
        }
        TaskEvent::TaskFinished { outcome, .. } => {
            // Results are printed after the batch in input order; see below.
            if let Ok(mut guard) = collected.lock() {
                guard.push(outcome);
            }
        }
        TaskEvent::Log { line } if !json => eprintln!("{line}"),
        _ => {}
    });
    (sink, outcomes)
}

fn print_legacy_outcome(outcome: &TaskOutcome, emulate_conflict_error: bool) {
    match effective_legacy_error(outcome, emulate_conflict_error) {
        Some((kind, message)) => {
            println!("FAIL {}: {} ({})", outcome.path.display(), message, kind)
        }
        None => {
            let outputs = legacy_outputs(outcome)
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "OK   {} -> {} (original: {}, {} bytes)",
                outcome.path.display(),
                outputs,
                original_name(outcome),
                outcome.bytes
            );
        }
    }
}

fn effective_legacy_error(
    outcome: &TaskOutcome,
    emulate_conflict_error: bool,
) -> Option<(String, String)> {
    match outcome.status {
        Some(TaskStatus::Success) => {
            // The engine has no "error" conflict policy: it skips entries that
            // already exist and still reports Success with files == 0. Emulate
            // the 0.1.1 --conflict error behaviour by failing such a task.
            if emulate_conflict_error && outcome.files == 0 && is_conflict_skip(outcome) {
                Some((
                    "outputExists".to_string(),
                    format!(
                        "output file already exists: {}",
                        legacy_conflict_target(outcome)
                    ),
                ))
            } else {
                None
            }
        }
        Some(TaskStatus::Skipped) => {
            if emulate_conflict_error {
                let target = outcome
                    .dest
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| outcome.path.display().to_string());
                Some((
                    "outputExists".to_string(),
                    format!("output file already exists: {target}"),
                ))
            } else {
                None
            }
        }
        Some(TaskStatus::NeedsPassword) => {
            // 0.1.1 only handled .enc files, where a failed password attempt was
            // reported as the core WrongPassword error. Keep that wording for
            // .enc inputs so scripts matching "(wrongPassword)" keep working.
            if looks_like_enc_input(outcome) {
                Some((
                    "wrongPassword".to_string(),
                    "incorrect password".to_string(),
                ))
            } else {
                Some((
                    "needsPassword".to_string(),
                    outcome
                        .reason
                        .clone()
                        .unwrap_or_else(|| "password required".to_string()),
                ))
            }
        }
        Some(TaskStatus::Failed) => Some(classify_legacy_failure(outcome)),
        Some(TaskStatus::Cancelled) => Some((
            "cancelled".to_string(),
            outcome
                .reason
                .clone()
                .unwrap_or_else(|| "cancelled".to_string()),
        )),
        None => Some((
            "failed".to_string(),
            outcome
                .reason
                .clone()
                .unwrap_or_else(|| "failed".to_string()),
        )),
    }
}

/// 0.1.1 reported the core error kind strings (wrongPassword / integrity / io /
/// zip / ...). The engine now only keeps a human-readable reason, so recover the
/// legacy kind from the reason text for the common failure modes.
fn classify_legacy_failure(outcome: &TaskOutcome) -> (String, String) {
    let reason = outcome
        .reason
        .clone()
        .unwrap_or_else(|| "decryption failed".to_string());
    if reason.contains("完整性校验失败") {
        return (
            "integrity".to_string(),
            "file integrity check failed (BLAKE3 MAC mismatch): the file is corrupted or was modified"
                .to_string(),
        );
    }
    if let Some(rest) = reason.strip_prefix("io error at ") {
        // Engine format: "io error at {path}: {source}".
        if let Some((_path, source)) = rest.rsplit_once(": ") {
            return ("io".to_string(), format!("I/O error: {source}"));
        }
        return ("io".to_string(), reason);
    }
    if reason.contains("无法读取内层 ZIP") {
        return ("zip".to_string(), reason);
    }
    ("failed".to_string(), reason)
}

fn looks_like_enc_input(outcome: &TaskOutcome) -> bool {
    outcome
        .path
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("enc"))
        .unwrap_or(false)
}

fn legacy_file_outcome(outcome: &TaskOutcome, emulate_conflict_error: bool) -> FileOutcome {
    let (error_kind, error_message) = effective_legacy_error(outcome, emulate_conflict_error)
        .map(|(kind, message)| (Some(kind), Some(message)))
        .unwrap_or((None, None));
    let outputs = legacy_outputs(outcome);
    let name = match outcome.status {
        Some(TaskStatus::Success) | Some(TaskStatus::Skipped) => Some(original_name(outcome)),
        _ => None,
    };
    FileOutcome {
        input: outcome.path.clone(),
        outputs,
        original_name: name,
        bytes_written: outcome.bytes,
        error_kind,
        error_message,
    }
}

// ---------------------------------------------------------------------------
// run subcommand
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "run", about = "批量解密/解压文件与目录")]
struct RunCli {
    /// Files or folders to process
    #[arg(value_name = "PATH")]
    inputs: Vec<PathBuf>,

    /// Password, repeatable (also UPZIP_PASSWORD / saved password library)
    #[arg(short = 'p', long = "password", value_name = "PASSWORD")]
    passwords: Vec<String>,

    #[command(flatten)]
    flags: RunFlags,
}

async fn cmd_run(args: &[String]) -> i32 {
    let cli: RunCli = match parse_sub(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    if cli.inputs.is_empty() {
        eprintln!("错误：请提供要处理的文件或目录（用法：upzip-cli run PATH...）");
        return EXIT_USAGE;
    }

    let passwords = resolve_passwords(&cli.passwords);
    let mut cfg = Settings::load().to_run_config(passwords.clone(), None);
    if let Err(message) = apply_run_flags(&mut cfg, &cli.flags) {
        eprintln!("错误：{message}");
        return EXIT_USAGE;
    }
    cfg.passwords = passwords;

    let sevenzip = match build_sevenzip(cli.flags.sevenzip.clone()) {
        Ok(sevenzip) => sevenzip,
        Err(code) => return code,
    };

    let output_root = cfg.output_root.clone();
    let files = expand_inputs(&cli.inputs, cfg.recursive, output_root.as_deref());
    if files.is_empty() {
        eprintln!("错误：没有找到可处理的文件");
        return EXIT_USAGE;
    }

    let sink: Sink = if cli.flags.json {
        json_sink()
    } else {
        run_human_sink()
    };

    let engine = Arc::new(Engine::new(sevenzip, cfg.clone()));
    let cancel = CancelToken::new();
    install_ctrlc(cancel.clone());
    let gate = Arc::new(PauseGate::new());
    let summary: Summary = run_batch(engine.clone(), files, cancel, gate, sink).await;
    // Only persist fingerprints when skipping is enabled; otherwise the store is
    // not consulted and writing it would just leak debug/incidental paths.
    if !cfg.dry_run && cfg.skip_processed {
        let _ = engine.save_processed().await;
    }

    if cli.flags.json {
        let line = serde_json::json!({
            "type": "summary",
            "total": summary.total,
            "success": summary.success,
            "skipped": summary.skipped,
            "needs_password": summary.needs_password,
            "failed": summary.failed,
            "cancelled": summary.cancelled,
            "bytes": summary.bytes,
            "elapsed_ms": summary.elapsed_ms,
        });
        println!("{}", serde_json::to_string(&line).unwrap_or_default());
    } else {
        println!();
        println!(
            "汇总：共 {} 个任务｜成功 {}｜跳过 {}｜需要密码 {}｜失败 {}｜取消 {}",
            summary.total,
            summary.success,
            summary.skipped,
            summary.needs_password,
            summary.failed,
            summary.cancelled
        );
        println!(
            "累计输出 {}，耗时 {}",
            human_bytes(summary.bytes),
            fmt_ms(summary.elapsed_ms)
        );
        match &output_root {
            Some(root) => println!("输出目录：{}", root.display()),
            None => println!("输出位置：每个输入文件旁边"),
        }
        if cfg.dry_run {
            println!("（演练模式：未写入任何文件）");
        }
    }

    if summary.cancelled > 0 {
        EXIT_CANCELLED
    } else if summary.failed > 0 || summary.needs_password > 0 {
        EXIT_FAIL
    } else {
        EXIT_OK
    }
}

fn json_sink() -> Sink {
    Arc::new(|event: TaskEvent| match serde_json::to_string(&event) {
        Ok(line) => {
            println!("{line}");
            let _ = std::io::stdout().flush();
        }
        Err(error) => eprintln!("警告：事件序列化失败：{error}"),
    })
}

fn run_human_sink() -> Sink {
    let last = Arc::new(Mutex::new((u64::MAX, u8::MAX)));
    Arc::new(move |event: TaskEvent| match event {
        TaskEvent::TaskQueued { id, path, size } => {
            println!(
                "[{id}] 加入队列：{}（{}）",
                path.display(),
                human_bytes(size)
            );
        }
        TaskEvent::TaskStage { id, stage, detail } => {
            if let Some(detail) = detail {
                println!("[{id}] {}：{detail}", stage_label(&stage));
            }
        }
        TaskEvent::TaskDetected { id, detection } => {
            println!(
                "[{id}] 识别：{}（容器 {}）",
                kind_name(&detection.kind),
                detection.container
            );
        }
        TaskEvent::TaskProgress { id, percent, .. } => {
            if std::io::stderr().is_terminal() {
                let percent = percent.unwrap_or(0);
                if let Ok(mut guard) = last.lock() {
                    if guard.0 != id || guard.1 != percent {
                        *guard = (id, percent);
                        eprint!("\r[{id}] {percent:3}%");
                        let _ = std::io::stderr().flush();
                    }
                }
            }
        }
        TaskEvent::TaskPasswordAttempt {
            id,
            source,
            ok,
            tried,
        } => {
            println!(
                "[{id}] 尝试密码（来源：{source}，第 {tried} 个）{}",
                if ok { "成功" } else { "失败" }
            );
        }
        TaskEvent::TaskPasswordMissing { id, tried } => {
            println!("[{id}] 缺少密码：已尝试 {tried} 个候选");
        }
        TaskEvent::TaskFinished { id, outcome } => {
            match outcome.status.unwrap_or(TaskStatus::Failed) {
                TaskStatus::Success => println!(
                    "[{id}] 完成：{}（{} 项，{}）",
                    outcome
                        .dest
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "?".to_string()),
                    outcome.files,
                    human_bytes(outcome.bytes)
                ),
                TaskStatus::Skipped => {
                    println!(
                        "[{id}] 跳过：{}",
                        outcome.reason.clone().unwrap_or_default()
                    )
                }
                TaskStatus::NeedsPassword => {
                    println!(
                        "[{id}] 需要密码：{}",
                        outcome.reason.clone().unwrap_or_default()
                    )
                }
                TaskStatus::Failed => {
                    println!(
                        "[{id}] 失败：{}",
                        outcome.reason.clone().unwrap_or_default()
                    )
                }
                TaskStatus::Cancelled => println!("[{id}] 已取消"),
            }
            for warning in &outcome.warnings {
                eprintln!("[{id}] 警告：{warning}");
            }
        }
        TaskEvent::Log { line } => eprintln!("{line}"),
    })
}

// ---------------------------------------------------------------------------
// detect subcommand
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "detect", about = "识别文件类型、容器与加密状态")]
struct DetectCli {
    /// Files to inspect
    #[arg(value_name = "PATH", required = true)]
    paths: Vec<PathBuf>,

    /// Print one JSON Detection object per line
    #[arg(long = "json")]
    json: bool,

    /// Explicit 7-Zip executable
    #[arg(long = "sevenzip", value_name = "PATH")]
    sevenzip: Option<PathBuf>,
}

async fn cmd_detect(args: &[String]) -> i32 {
    let cli: DetectCli = match parse_sub(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let sevenzip = match build_sevenzip(cli.sevenzip.clone()) {
        Ok(sevenzip) => sevenzip,
        Err(code) => return code,
    };
    let cancel = CancelToken::new();
    let mut failures = 0usize;
    for path in &cli.paths {
        match upzip_unpack_core::detect(&sevenzip, path, &cancel).await {
            Ok(detection) => {
                if cli.json {
                    match serde_json::to_string(&detection) {
                        Ok(line) => println!("{line}"),
                        Err(error) => eprintln!("警告：序列化失败：{error}"),
                    }
                } else {
                    print_detection(&detection);
                }
            }
            Err(error) => {
                failures += 1;
                if cli.json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "path": path,
                            "error": error.to_string(),
                        })
                    );
                } else {
                    eprintln!("错误：{}：{error}", path.display());
                }
            }
        }
    }
    if failures == 0 {
        EXIT_OK
    } else {
        EXIT_FAIL
    }
}

fn print_detection(detection: &Detection) {
    println!("{}", detection.path.display());
    println!("  类型：{}", kind_name(&detection.kind));
    match &detection.format {
        Some(format) => println!("  容器：{}（{format}）", detection.container),
        None => println!("  容器：{}", detection.container),
    }
    println!(
        "  大小：{}（{} 字节）",
        human_bytes(detection.size),
        detection.size
    );
    if let Some(offset) = detection.offset {
        println!("  偏移：{offset}");
    }
    println!(
        "  条目：{}（加密 {}）",
        detection.total_entries, detection.encrypted_entries
    );
    if !detection.top_level.is_empty() {
        println!("  顶层：{}", detection.top_level.join(", "));
    }
    println!("  密码：{}", password_state_name(&detection.password_state));
    if !detection.hints.is_empty() {
        println!("  提示：{}", detection.hints.join("; "));
    }
    if !detection.notes.is_empty() {
        println!("  备注：{}", detection.notes.join("; "));
    }
}

// ---------------------------------------------------------------------------
// inspect subcommand (legacy-compatible output)
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "inspect", about = "打印 .enc 文件的明文头信息")]
struct InspectCli {
    /// .enc files to inspect
    #[arg(value_name = "FILE", required = true)]
    files: Vec<PathBuf>,

    /// Print machine-readable JSON instead of human-readable text
    #[arg(long = "json")]
    json: bool,
}

fn cmd_inspect(args: &[String]) -> i32 {
    let cli: InspectCli = match parse_sub(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    inspect_files(&cli.files, cli.json)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn inspect_files(files: &[PathBuf], json: bool) -> i32 {
    if files.is_empty() {
        eprintln!("error: no input files given");
        return EXIT_USAGE;
    }
    let mut failures = 0usize;
    let mut results: Vec<serde_json::Value> = Vec::new();
    for file in files {
        match inspect_file(file) {
            Ok(header) => match parse_argon2_params(header.custom_params) {
                Ok(params) => {
                    let size = std::fs::metadata(file).map(|meta| meta.len()).unwrap_or(0);
                    if json {
                        results.push(serde_json::json!({
                            "file": file,
                            "size": size,
                            "magic": String::from_utf8_lossy(&header.raw[..5]).to_string(),
                            "version": header.version,
                            "algorithm": header.algorithm,
                            "customParams": header.custom_params,
                            "salt": hex(&header.salt),
                            "argon2": {
                                "iterations": params.iterations,
                                "memoryKiB": params.memory_kib,
                                "parallelism": params.parallelism,
                                "outputLen": params.output_len,
                            },
                        }));
                    } else {
                        println!("file:           {}", file.display());
                        println!("size:           {size} bytes");
                        println!(
                            "magic:          {}",
                            String::from_utf8_lossy(&header.raw[..5])
                        );
                        println!("format version: {}", header.version);
                        println!("algorithm:      {} (AES-256)", header.algorithm);
                        println!(
                            "custom params:  0x{:02x} (Argon2id t={}, m={} KiB, p={})",
                            header.custom_params,
                            params.iterations,
                            params.memory_kib,
                            params.parallelism
                        );
                        println!("salt:           {}", hex(&header.salt));
                    }
                }
                Err(error) => {
                    failures += 1;
                    if json {
                        results.push(serde_json::json!({
                            "file": file,
                            "errorKind": error.kind(),
                            "errorMessage": error.to_string(),
                        }));
                    } else {
                        eprintln!("FAIL {}: {} ({})", file.display(), error, error.kind());
                    }
                }
            },
            Err(error) => {
                failures += 1;
                if json {
                    results.push(serde_json::json!({
                        "file": file,
                        "errorKind": error.kind(),
                        "errorMessage": error.to_string(),
                    }));
                } else {
                    eprintln!("FAIL {}: {} ({})", file.display(), error, error.kind());
                }
            }
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "files": results }))
                .unwrap_or_default()
        );
    }
    if failures == 0 {
        EXIT_OK
    } else {
        EXIT_FAIL
    }
}

// ---------------------------------------------------------------------------
// config subcommand
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "config",
    about = "查看/管理明文密码库（默认掩码；--list-passwords 输出明文）"
)]
struct ConfigCli {
    /// Show the config path and the saved password library (masked)
    #[arg(long = "show")]
    show: bool,

    /// Print only the config file path
    #[arg(long = "path")]
    path: bool,

    /// Overwrite the library with a single password (0.1.1 compatibility)
    #[arg(long = "set-password", value_name = "PASSWORD")]
    set_password: Option<String>,

    /// Append one password (no-op when it is already stored)
    #[arg(long = "add-password", value_name = "PASSWORD")]
    add_password: Option<String>,

    /// Remove the password at this 0-based index
    #[arg(long = "remove-password", value_name = "INDEX")]
    remove_password: Option<usize>,

    /// Clear the saved password library (0.1.1 compatibility alias)
    #[arg(long = "clear-password")]
    clear_password: bool,

    /// Clear the saved password library
    #[arg(long = "clear-passwords")]
    clear_passwords: bool,

    /// Print one plaintext password per line (for scripts)
    #[arg(long = "list-passwords")]
    list_passwords: bool,
}

fn cmd_config(args: &[String]) -> i32 {
    let cli: ConfigCli = match parse_sub(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let path = match config_file_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("错误：{error}");
            return EXIT_USAGE;
        }
    };

    let has_action = cli.set_password.is_some()
        || cli.add_password.is_some()
        || cli.remove_password.is_some()
        || cli.clear_password
        || cli.clear_passwords;

    for (flag, value) in [
        ("--set-password", cli.set_password.as_deref()),
        ("--add-password", cli.add_password.as_deref()),
    ] {
        if value == Some("") {
            eprintln!("错误：{flag} 的密码不能为空");
            return EXIT_USAGE;
        }
    }

    // 0.1.1 behaviour: `config --path` alone prints only the path.
    if cli.path && !cli.show && !has_action && !cli.list_passwords {
        println!("{}", path.display());
        return EXIT_OK;
    }

    if let Some(password) = &cli.set_password {
        match save_passwords(std::slice::from_ref(password)) {
            Ok(()) => println!("已保存密码到 {}", path.display()),
            Err(error) => {
                eprintln!("错误：{error}");
                return EXIT_USAGE;
            }
        }
    }
    if let Some(password) = &cli.add_password {
        match add_stored_password(password) {
            Ok(true) => println!("已添加密码（当前 {} 个）", stored_password_count()),
            Ok(false) => println!("密码已存在，未重复添加"),
            Err(error) => {
                eprintln!("错误：{error}");
                return EXIT_USAGE;
            }
        }
    }
    if let Some(index) = cli.remove_password {
        match remove_password_at(index) {
            Ok(true) => println!("已删除第 {index} 个密码"),
            Ok(false) => {
                eprintln!(
                    "错误：密码下标 {index} 超出范围（当前 {} 个）",
                    stored_password_count()
                );
                return EXIT_USAGE;
            }
            Err(error) => {
                eprintln!("错误：{error}");
                return EXIT_USAGE;
            }
        }
    }
    if cli.clear_password || cli.clear_passwords {
        match clear_passwords() {
            Ok(()) => {
                if cli.clear_password {
                    println!("已清除已保存的密码");
                } else {
                    println!("已清空密码库");
                }
            }
            Err(error) => {
                eprintln!("错误：{error}");
                return EXIT_USAGE;
            }
        }
    }

    let show_store = cli.show || (!has_action && !cli.list_passwords && !cli.path);
    if show_store {
        println!("配置路径：{}", path.display());
        match load_passwords() {
            Ok(list) => {
                println!(
                    "密码状态：{}",
                    if list.is_empty() {
                        "未保存"
                    } else {
                        "已保存"
                    }
                );
                println!(
                    "密码库：{} 个（明文保存在本机配置文件；unix 权限 0600）",
                    list.len()
                );
                for (index, password) in list.iter().enumerate() {
                    println!("  [{index}] {}", mask_password(password));
                }
                if list.is_empty() {
                    println!("提示：--add-password PW 添加密码；run -p PW 只用于本次运行");
                } else {
                    println!(
                        "提示：--list-passwords 显示明文；--remove-password INDEX 删除；--clear-passwords 清空"
                    );
                }
            }
            Err(error) => {
                println!("密码状态：读取失败（{error}）");
                return EXIT_USAGE;
            }
        }
    } else if cli.path {
        println!("配置路径：{}", path.display());
    }

    if cli.list_passwords {
        match load_passwords() {
            Ok(list) => {
                for password in &list {
                    println!("{password}");
                }
            }
            Err(error) => {
                eprintln!("错误：{error}");
                return EXIT_USAGE;
            }
        }
    }
    EXIT_OK
}

// ---------------------------------------------------------------------------
// doctor subcommand
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "doctor", about = "环境自检：7-Zip、配置目录、detect 探针")]
struct DoctorCli {
    /// Explicit 7-Zip executable
    #[arg(long = "sevenzip", value_name = "PATH")]
    sevenzip: Option<PathBuf>,
}

async fn cmd_doctor(args: &[String]) -> i32 {
    let cli: DoctorCli = match parse_sub(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let mut ok = true;
    println!("upzip-cli {}", env!("CARGO_PKG_VERSION"));
    println!("平台：{} {}", std::env::consts::OS, std::env::consts::ARCH);

    match config_dir() {
        Ok(dir) => println!("配置目录：{}", dir.display()),
        Err(error) => {
            ok = false;
            println!("配置目录：不可用（{error}）");
        }
    }
    match config_file_path() {
        Ok(path) => println!(
            "配置文件：{}（{}）",
            path.display(),
            if path.exists() { "存在" } else { "不存在" }
        ),
        Err(error) => println!("配置文件：不可用（{error}）"),
    }
    match Settings::path() {
        Some(path) => println!(
            "settings.json：{}（{}）",
            path.display(),
            if path.exists() { "存在" } else { "不存在" }
        ),
        None => println!("settings.json：配置目录不可用"),
    }
    let processed = ProcessedStore::default_path();
    let processed_count = ProcessedStore::load(processed.clone()).count();
    println!(
        "processed.json：{}（{}，{} 条记录）",
        processed.display(),
        if processed.exists() {
            "存在"
        } else {
            "不存在"
        },
        processed_count
    );

    let sevenzip = match resolve_sevenzip(cli.sevenzip.clone()) {
        Ok(choice) => {
            println!(
                "7-Zip：{}（{}，来源：{}）",
                choice.sevenzip.exe.display(),
                choice.sevenzip.version(),
                choice.source.label()
            );
            Some(choice.sevenzip)
        }
        Err(error) => {
            ok = false;
            println!("7-Zip：未找到（{error}）");
            println!(
                "提示：设置环境变量 UPZIP_7Z 指向 7zz/7z.exe，或把 7zz/7z.exe 放到应用同目录。"
            );
            None
        }
    };

    if let Some(sevenzip) = sevenzip {
        let temp =
            std::env::temp_dir().join(format!("upzip-cli-doctor-{}.bin", std::process::id()));
        let mut self_check_ok = false;
        match std::fs::write(&temp, b"upzip-cli doctor self-check\n") {
            Ok(()) => {
                let cancel = CancelToken::new();
                match upzip_unpack_core::detect(&sevenzip, &temp, &cancel).await {
                    Ok(detection) => {
                        println!("detect 自检：通过（kind={}）", kind_name(&detection.kind));
                        self_check_ok = true;
                    }
                    Err(error) => println!("detect 自检：失败（{error}）"),
                }
            }
            Err(error) => println!("detect 自检：无法创建临时文件（{error}）"),
        }
        let _ = std::fs::remove_file(&temp);
        if !self_check_ok {
            ok = false;
        }
    } else {
        println!("detect 自检：跳过（需要 7-Zip）");
    }

    if ok {
        EXIT_OK
    } else {
        EXIT_FAIL
    }
}

// ---------------------------------------------------------------------------
// cleanup-staging subcommand
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "cleanup-staging", about = "清理 .upzip-tmp-* 残留")]
struct CleanupCli {
    /// Directories to scan (default: current directory), repeatable
    #[arg(long = "root", value_name = "DIR")]
    roots: Vec<PathBuf>,
}

fn cmd_cleanup(args: &[String]) -> i32 {
    let cli: CleanupCli = match parse_sub(args) {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let roots: Vec<PathBuf> = if cli.roots.is_empty() {
        match std::env::current_dir() {
            Ok(dir) => vec![dir],
            Err(error) => {
                eprintln!("错误：无法确定当前目录：{error}");
                return EXIT_USAGE;
            }
        }
    } else {
        cli.roots.clone()
    };

    let mut removed = 0usize;
    let mut failed = 0usize;
    let mut valid_roots = 0usize;
    for root in &roots {
        if !root.exists() {
            eprintln!("警告：目录不存在，已跳过：{}", root.display());
            continue;
        }
        valid_roots += 1;
        let mut staging = Vec::new();
        collect_staging(root, 0, &mut staging);
        let mut root_removed = 0usize;
        for path in staging {
            let is_dir = std::fs::symlink_metadata(&path)
                .map(|meta| meta.is_dir())
                .unwrap_or(false);
            let result = if is_dir {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            match result {
                Ok(()) => {
                    removed += 1;
                    root_removed += 1;
                }
                Err(error) => {
                    failed += 1;
                    eprintln!("警告：清理失败 {}：{error}", path.display());
                }
            }
        }
        println!("{}：清理 {root_removed} 个残留", root.display());
    }
    if valid_roots == 0 {
        eprintln!("错误：没有可清理的目录");
        return EXIT_USAGE;
    }
    println!("共清理 {removed} 个 .upzip-tmp-* 残留（失败 {failed} 个）");
    if failed == 0 {
        EXIT_OK
    } else {
        EXIT_FAIL
    }
}

fn collect_staging(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(TMP_PREFIX) {
            out.push(path);
            continue;
        }
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir {
            collect_staging(&path, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Environment mutation is process-wide; keep these tests serialised.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_VARS: [&str; 5] = [
        "UPZIP_PASSWORD",
        "UNPARANOIA_PASSWORD",
        "PARANOIA_ENC_PASSWORD",
        "UPZIP_7Z",
        "UNPARANOIA_7Z",
    ];

    fn clear_env() {
        for name in ENV_VARS {
            std::env::remove_var(name);
        }
    }

    fn touch(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "upzip-env-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn env_password_prefers_new_name_and_falls_back() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        std::env::set_var("PARANOIA_ENC_PASSWORD", "oldest");
        assert_eq!(env_password().as_deref(), Some("oldest"));
        std::env::set_var("UNPARANOIA_PASSWORD", "legacy");
        assert_eq!(env_password().as_deref(), Some("legacy"));
        std::env::set_var("UPZIP_PASSWORD", "current");
        assert_eq!(env_password().as_deref(), Some("current"));
        // Empty values never shadow a lower-priority name.
        std::env::set_var("UPZIP_PASSWORD", "");
        assert_eq!(env_password().as_deref(), Some("legacy"));
        clear_env();
    }

    #[test]
    fn env_sevenzip_path_prefers_new_name_and_falls_back() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_env();
        let legacy = touch("legacy-7z");
        let current = touch("current-7z");
        std::env::set_var("UNPARANOIA_7Z", &legacy);
        assert_eq!(env_sevenzip_path(), Some(legacy.clone()));
        std::env::set_var("UPZIP_7Z", &current);
        assert_eq!(env_sevenzip_path(), Some(current.clone()));
        clear_env();
        let _ = std::fs::remove_file(legacy);
        let _ = std::fs::remove_file(current);
    }
}
