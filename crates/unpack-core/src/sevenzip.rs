//! Thin async wrapper around the bundled 7-Zip command line tool.
//!
//! Verified behaviour (7-Zip 23.01 / 25.01, macOS):
//! * `l -slt` is the authoritative format detector: it reports the real
//!   container (`Type = zip`), the payload `Offset` inside masked/containered
//!   files and the SFX `Embedded Stub Size`.
//! * Exit codes: 0 ok, 1 warnings, 2 error (includes "Wrong password"),
//!   255 needs a password / broken pipe.
//! * `t -ir!<entry>` verifies a password against a single entry in ~10-25 ms.
//! * Passwords go over stdin by default so they never show up in the process
//!   list; on Windows the default is `-p<pw>` on the command line instead,
//!   because 7z.exe may read the password through the console API and ignore a
//!   redirected stdin (NOT yet verified on real Windows hardware). Either
//!   behaviour can be forced with `UNPARANOIA_7Z_PW_MODE=stdin|argv` (legacy
//!   `UPZIP_7Z_PW_MODE` is still accepted).

use std::collections::BTreeMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::cancel::CancelToken;
use crate::error::{Result, UpError};

pub const EXIT_OK: i32 = 0;
pub const EXIT_WARNING: i32 = 1;
pub const EXIT_ERROR: i32 = 2;
pub const EXIT_NEEDS_PASSWORD: i32 = 255;

const MAX_CAPTURE: usize = 8 * 1024 * 1024;

/// Windows: never open a console window for a child process. Without this a GUI
/// build flashes a black console window every time 7z.exe is spawned.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Windows spawn errors worth retrying: the executable is momentarily locked by
/// antivirus or the system (5 = access denied, 32 = sharing violation,
/// 225 = scanner still holding a fresh file).
const RETRYABLE_SPAWN_ERRORS: [i32; 3] = [5, 32, 225];
/// How many times a retryable spawn error is retried (150 ms apart).
const SPAWN_RETRIES: u32 = 3;
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(150);

/// How much of the 7-Zip stderr tail is quoted in an error message.
const ERROR_TAIL_LIMIT: usize = 1000;

#[derive(Debug, Clone, Default)]
pub struct ListEntry {
    /// Normalised path (forward slashes).
    pub path: String,
    /// Path exactly as 7-Zip printed it (native separators).
    pub raw_path: String,
    pub is_dir: bool,
    pub size: u64,
    pub packed: u64,
    pub encrypted: bool,
    pub method: String,
}

impl ListEntry {
    pub fn top(&self) -> &str {
        self.path.split('/').next().unwrap_or("")
    }
}

#[derive(Debug, Clone, Default)]
pub struct ListResult {
    pub archive_type: String,
    pub offset: u64,
    pub stub_size: u64,
    pub physical_size: u64,
    /// Number of volumes reported for a split container (0 = not split).
    pub volumes: u32,
    /// Sum of all volume sizes reported for a split container.
    pub total_physical_size: u64,
    pub entries: Vec<ListEntry>,
}

pub const JUNK_NAMES: &[&str] = &[
    "__MACOSX",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
];

impl ListResult {
    pub fn is_junk_path(path: &str) -> bool {
        path.split('/').any(|c| {
            c.is_empty()
                || JUNK_NAMES.iter().any(|j| j.eq_ignore_ascii_case(c))
                || c.starts_with("._")
        })
    }

    /// Distinct top-level names. `strip_junk` drops `__MACOSX`, `._*`, ...
    pub fn top_level(&self, strip_junk: bool) -> Vec<String> {
        let mut set: Vec<String> = Vec::new();
        for e in &self.entries {
            let p = e.path.trim_start_matches("./");
            if p.is_empty() {
                continue;
            }
            if strip_junk && Self::is_junk_path(p) {
                continue;
            }
            let t = p.split('/').next().unwrap_or("").to_string();
            if t.is_empty() {
                continue;
            }
            if !set.contains(&t) {
                set.push(t);
            }
        }
        set
    }

    pub fn encrypted_entries(&self) -> impl Iterator<Item = &ListEntry> {
        self.entries.iter().filter(|e| e.encrypted && !e.is_dir)
    }

    pub fn smallest_encrypted(&self) -> Option<&ListEntry> {
        self.encrypted_entries()
            .filter(|e| e.size > 0)
            .min_by_key(|e| e.packed.min(e.size))
    }

    pub fn total_uncompressed(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.size)
            .sum()
    }

    pub fn file_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_dir).count()
    }

    /// Does this look like an OOXML/JAR container (Office document, .jar, .apk)? Those
    /// are ZIP files but users do not want them expanded.
    pub fn looks_like_office(&self) -> bool {
        self.entries.iter().any(|e| {
            let p = e.path.trim_start_matches("./");
            p == "[Content_Types].xml"
                || p == "META-INF/MANIFEST.MF"
                || p == "mimetype"
                || p.starts_with("docProps/")
        })
    }
}

#[derive(Debug, Clone)]
pub enum PwCheck {
    Ok,
    Wrong,
    NeedsPassword,
    Other(String),
}

#[derive(Debug, Clone, Default)]
pub struct ExtractStats {
    pub warnings: usize,
}

/// How the archive password is handed to 7-Zip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordMode {
    /// Feed the password over stdin so it never shows up in the process list.
    Stdin,
    /// Pass `-p<pw>` on the command line. Needed on Windows, where 7z.exe may
    /// read the password through the console API and ignore a redirected stdin.
    Argv,
}

impl PasswordMode {
    /// stdin everywhere except Windows, where the argv form is the safer bet.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            Self::Argv
        } else {
            Self::Stdin
        }
    }

    /// `UNPARANOIA_7Z_PW_MODE=stdin|argv` wins (legacy `UPZIP_7Z_PW_MODE` is
    /// still accepted), otherwise the platform default.
    pub fn from_env_or_platform() -> Self {
        let mode = std::env::var("UNPARANOIA_7Z_PW_MODE")
            .or_else(|_| std::env::var("UPZIP_7Z_PW_MODE"))
            .ok()
            .map(|s| s.trim().to_ascii_lowercase());
        match mode.as_deref() {
            Some("argv") => Self::Argv,
            Some("stdin") => Self::Stdin,
            _ => Self::platform_default(),
        }
    }
}

pub struct SevenZip {
    pub exe: PathBuf,
    pub password_mode: PasswordMode,
}

impl SevenZip {
    pub fn new(exe: PathBuf) -> Self {
        Self {
            exe,
            password_mode: PasswordMode::from_env_or_platform(),
        }
    }

    pub fn with_password_mode(exe: PathBuf, password_mode: PasswordMode) -> Self {
        Self { exe, password_mode }
    }

    /// Resolve the 7-Zip binary. Search order:
    /// 1. explicit path;
    /// 2. `UNPARANOIA_7Z` (legacy `UPZIP_7Z` still accepted);
    /// 3. next to the running executable, then its `binaries/` folder and the
    ///    platform subdir (`binaries/mac`, `binaries/win`);
    /// 4. macOS `.app`: `../Resources/binaries/{mac,}`, Windows portable:
    ///    `../binaries/win`, `../Resources/binaries/win`;
    /// 5. the repo's `binaries/{mac,win}/` folder (dev builds: up to 6 parent
    ///    directories of the executable);
    /// 6. `PATH`, 7. common install dirs.
    pub fn locate(explicit: Option<PathBuf>) -> Result<Self> {
        let mut searched: Vec<PathBuf> = Vec::new();

        let (names, subdir): (&[&str], &str) = if cfg!(windows) {
            (&["7z.exe", "7za.exe"], "win")
        } else {
            (&["7zz", "7z", "7za"], "mac")
        };

        if let Some(p) = explicit {
            if p.is_file() {
                return Ok(Self::new(p));
            }
            searched.push(p);
        }
        for var in ["UNPARANOIA_7Z", "UPZIP_7Z"] {
            if let Some(raw) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                let p = PathBuf::from(raw);
                if p.is_file() {
                    return Ok(Self::new(p));
                }
                searched.push(p);
            }
        }

        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut exe_dir: Option<PathBuf> = None;
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                exe_dir = Some(d.to_path_buf());
                // Directly next to the app binary, then a bundled binaries/ folder.
                dirs.push(d.to_path_buf());
                dirs.push(d.join("binaries"));
                dirs.push(d.join("binaries").join(subdir));
                // macOS .app: Contents/MacOS/app -> Contents/Resources/binaries/...
                dirs.push(d.join("../Resources/binaries"));
                dirs.push(d.join("../Resources/binaries").join(subdir));
                dirs.push(d.join("../Resources"));
                // Windows portable layout: binaries next to the exe's parent.
                dirs.push(d.join("../binaries"));
                dirs.push(d.join("../binaries").join(subdir));
            }
        }
        // Dev builds: climb towards the repository root and use its binaries/.
        if let Some(d) = &exe_dir {
            let mut cur: Option<&Path> = Some(d.as_path());
            for _ in 0..6 {
                let Some(p) = cur else { break };
                dirs.push(p.join("binaries").join(subdir));
                dirs.push(p.join("binaries"));
                cur = p.parent();
            }
        }
        dirs.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../binaries")
                .join(subdir),
        );
        dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../binaries"));
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            dirs.push(PathBuf::from(d));
        }

        for d in dirs {
            for n in names {
                let cand = d.join(n);
                if cand.is_file() {
                    return Ok(Self::new(cand));
                }
                searched.push(cand);
            }
        }
        Err(UpError::SevenZipMissing { searched })
    }

    pub fn version(&self) -> String {
        #[allow(unused_mut)]
        let mut cmd = std::process::Command::new(&self.exe);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        match cmd.output() {
            Ok(o) => {
                let text = String::from_utf8_lossy(&o.stdout);
                text.lines()
                    .find(|l| l.contains("7-Zip"))
                    .unwrap_or("7-Zip (unknown)")
                    .trim()
                    .to_string()
            }
            Err(e) => format!("error running {:?}: {e}", self.exe),
        }
    }

    /// Run 7-Zip, reading stdout line-by-line (`\r` and `\n` both split lines) so
    /// `-bsp1` progress can be forwarded live. Returns (exit code, stdout, stderr).
    pub async fn run(
        &self,
        args: &[String],
        password: Option<&str>,
        timeout: Option<Duration>,
        cancel: &CancelToken,
        on_line: &mut (dyn FnMut(&str) + Send),
    ) -> Result<(i32, String, String)> {
        // Argv mode: inject `-p<pw>` right after the command word. Empty
        // passwords are skipped — a bare `-p` would make 7-Zip prompt, and
        // stdin is closed in that case, so the run would fail confusingly.
        let argv_owned: Vec<String> = match (self.password_mode, password) {
            (PasswordMode::Argv, Some(pw)) if !pw.is_empty() => inject_password(args, pw),
            _ => Vec::new(),
        };
        let effective_args: &[String] = if argv_owned.is_empty() {
            args
        } else {
            &argv_owned
        };
        let stdin_password: Option<&str> = match (self.password_mode, password) {
            (PasswordMode::Stdin, Some(pw)) => Some(pw),
            _ => None,
        };

        // Spawn with a fresh command on every attempt: Windows antivirus or an
        // installer may briefly hold the bundled 7z.exe (5 / 32 / 225).
        let mut attempt: u32 = 0;
        let mut child = loop {
            let mut cmd = base_command(&self.exe);
            cmd.args(effective_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(if stdin_password.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                });
            match cmd.spawn() {
                Ok(child) => break child,
                Err(e) if is_retryable_spawn_error(e.raw_os_error()) && attempt < SPAWN_RETRIES => {
                    attempt += 1;
                    tokio::time::sleep(SPAWN_RETRY_DELAY).await;
                }
                Err(e) => return Err(spawn_failure(&self.exe, &e)),
            }
        };

        if let Some(pw) = stdin_password {
            if let Some(mut si) = child.stdin.take() {
                let _ = si.write_all(pw.as_bytes()).await;
                let _ = si.write_all(b"\n").await;
                drop(si);
            }
        }

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let err_task = tokio::spawn(async move {
            let mut buf = String::new();
            let mut reader = BufReader::new(stderr);
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if buf.len() < MAX_CAPTURE {
                            buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                        }
                    }
                }
            }
            buf
        });

        let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
        let mut deadline_fut = Box::pin(async move {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        });

        let mut out_buf = String::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut reader = BufReader::new(stdout);
        let mut chunk = [0u8; 8192];
        let mut killed: Option<UpError> = None;

        loop {
            tokio::select! {
                _ = cancel.wait_cancelled() => {
                    let _ = child.kill().await;
                    killed = Some(UpError::Cancelled);
                    break;
                }
                _ = &mut deadline_fut => {
                    let _ = child.kill().await;
                    killed = Some(UpError::Other("7-Zip timed out".into()));
                    break;
                }
                read = reader.read(&mut chunk) => {
                    match read {
                        Ok(0) => break,
                        Ok(n) => {
                            for &b in &chunk[..n] {
                                if b == b'\r' || b == b'\n' {
                                    if !pending.is_empty() {
                                        let line = String::from_utf8_lossy(&pending).to_string();
                                        pending.clear();
                                        if out_buf.len() < MAX_CAPTURE {
                                            out_buf.push_str(&line);
                                            out_buf.push('\n');
                                        }
                                        on_line(&line);
                                    }
                                } else {
                                    pending.push(b);
                                }
                            }
                        }
                        Err(e) => {
                            killed = Some(UpError::Other(format!("read error: {e}")));
                            break;
                        }
                    }
                }
            }
        }
        if !pending.is_empty() {
            let line = String::from_utf8_lossy(&pending).to_string();
            if out_buf.len() < MAX_CAPTURE {
                out_buf.push_str(&line);
                out_buf.push('\n');
            }
            on_line(&line);
        }

        if let Some(e) = killed {
            let _ = child.wait().await;
            return Err(e);
        }

        let status = child.wait().await.map_err(|e| UpError::SevenZipFailed {
            code: -1,
            message: format!(
                "无法等待 7-Zip 进程结束（{}）：{e}。可设置 UNPARANOIA_7Z 指向已安装的 7-Zip（7zz / 7z.exe）。",
                self.exe.display()
            ),
        })?;
        let err_text = err_task.await.unwrap_or_default();
        Ok((status.code().unwrap_or(-1), out_buf, err_text))
    }

    /// `l -slt` — authoritative detection + listing.
    pub async fn list(
        &self,
        archive: &Path,
        password: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<ListResult> {
        let args = vec![
            "l".to_string(),
            "-slt".to_string(),
            "-sccUTF-8".to_string(),
            archive.to_string_lossy().into_owned(),
        ];
        let (code, out, err) = self
            .run(
                &args,
                password,
                Some(Duration::from_secs(600)),
                cancel,
                &mut |_| {},
            )
            .await?;
        let text = format!("{out}\n{err}");
        match code {
            EXIT_OK | EXIT_WARNING => Ok(parse_slt(&out)),
            EXIT_NEEDS_PASSWORD => Err(UpError::PasswordRequired),
            EXIT_ERROR => {
                if text.contains("Wrong password") {
                    Err(UpError::PasswordRequired)
                } else if looks_like_not_archive(&text) {
                    Err(UpError::NotAnArchive(archive.to_path_buf()))
                } else {
                    Err(sevenzip_failure(&self.exe, code, &text))
                }
            }
            c => Err(sevenzip_failure(&self.exe, c, &text)),
        }
    }

    /// Best-effort listing of a split-volume container.
    ///
    /// 7-Zip can exit with an error while still printing the split header
    /// ("Type = Split", "Volumes = N") and a partial entry list — exactly what
    /// happens when a later volume is missing. Returns `Some` only when the
    /// output proves the file is the first volume of a split family.
    pub async fn list_split(
        &self,
        archive: &Path,
        cancel: &CancelToken,
    ) -> Result<Option<ListResult>> {
        let args = vec![
            "l".to_string(),
            "-slt".to_string(),
            "-sccUTF-8".to_string(),
            archive.to_string_lossy().into_owned(),
        ];
        let (code, out, err) = self
            .run(
                &args,
                None,
                Some(Duration::from_secs(600)),
                cancel,
                &mut |_| {},
            )
            .await?;
        if code == EXIT_OK || code == EXIT_WARNING {
            return Ok(None);
        }
        let text = format!("{out}\n{err}");
        if !text.contains("Type = Split") {
            return Ok(None);
        }
        let mut parsed = parse_slt(&out);
        if let Some(volumes) = header_u32(&text, "Volumes") {
            parsed.volumes = volumes;
        }
        Ok(Some(parsed))
    }

    async fn run_test(
        &self,
        archive: &Path,
        password: Option<&str>,
        pattern: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<(i32, String)> {
        let mut args = vec!["t".to_string(), "-sccUTF-8".to_string()];
        if let Some(p) = pattern {
            args.push(format!("-ir!{p}"));
        }
        args.push(archive.to_string_lossy().into_owned());
        let (code, out, err) = self
            .run(
                &args,
                password,
                Some(Duration::from_secs(1800)),
                cancel,
                &mut |_| {},
            )
            .await?;
        Ok((code, format!("{out}\n{err}")))
    }

    /// Quick password check. If `pattern` is given only that entry is tested
    /// (~10 ms); otherwise the whole archive is tested. A pattern that matches
    /// nothing is retried as a full test so a valid password is never rejected.
    pub async fn test_entry(
        &self,
        archive: &Path,
        password: Option<&str>,
        pattern: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<PwCheck> {
        let (mut code, mut text) = self.run_test(archive, password, pattern, cancel).await?;
        if pattern.is_some() && text.contains("No files to process") {
            let (c2, t2) = self.run_test(archive, password, None, cancel).await?;
            code = c2;
            text = t2;
        }
        Ok(match code {
            EXIT_OK | EXIT_WARNING => PwCheck::Ok,
            EXIT_NEEDS_PASSWORD => PwCheck::NeedsPassword,
            EXIT_ERROR => {
                if text.contains("Wrong password") {
                    PwCheck::Wrong
                } else if looks_like_not_archive(&text) {
                    PwCheck::Other("not an archive".into())
                } else {
                    PwCheck::Other(tail_text(&text))
                }
            }
            c => PwCheck::Other(format!("exit {c}")),
        })
    }

    /// Extract into `dest`. `-aos` keeps the staging dir idempotent; the real
    /// conflict policy is applied when the staging content is moved into place.
    pub async fn extract(
        &self,
        archive: &Path,
        password: Option<&str>,
        dest: &Path,
        cancel: &CancelToken,
        on_line: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ExtractStats> {
        std::fs::create_dir_all(dest).map_err(|e| UpError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;
        let args = vec![
            "x".to_string(),
            "-y".to_string(),
            "-sccUTF-8".to_string(),
            "-bsp1".to_string(),
            "-aos".to_string(),
            "-xr!__MACOSX".to_string(),
            "-xr!.DS_Store".to_string(),
            "-xr!._*".to_string(),
            "-xr!Thumbs.db".to_string(),
            format!("-o{}", dest.to_string_lossy()),
            archive.to_string_lossy().into_owned(),
        ];
        let (code, out, err) = self.run(&args, password, None, cancel, on_line).await?;
        let text = format!("{out}\n{err}");
        match code {
            EXIT_OK => Ok(ExtractStats { warnings: 0 }),
            EXIT_WARNING => Ok(ExtractStats {
                warnings: text.matches("WARNING").count().max(1),
            }),
            EXIT_NEEDS_PASSWORD => Err(UpError::PasswordRequired),
            EXIT_ERROR => {
                if text.contains("Wrong password") {
                    Err(UpError::PasswordRequired)
                } else if looks_like_not_archive(&text) {
                    Err(UpError::NotAnArchive(archive.to_path_buf()))
                } else {
                    Err(sevenzip_failure(&self.exe, code, &text))
                }
            }
            c => Err(sevenzip_failure(&self.exe, c, &text)),
        }
    }
}

/// Insert `-p<pw>` right after the 7-Zip command word (e.g. after `t` or `x`),
/// which is a position every 7-Zip build accepts.
fn inject_password(args: &[String], password: &str) -> Vec<String> {
    let mut out = args.to_vec();
    let at = if out.is_empty() { 0 } else { 1 };
    out.insert(at, format!("-p{password}"));
    out
}

pub fn looks_like_not_archive(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        "Can not open the file as archive",
        "Cannot open the file as archive",
        "is not supported archive",
        "Is not archive",
        "Unexpected end of archive",
        "Can not open the file as [",
        "Cannot open the file as [",
    ];
    MARKERS.iter().any(|m| text.contains(m))
}

fn tail_text(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| l.contains("ERROR") || l.contains("Wrong password") || l.contains("WARNING"))
        .collect();
    let tail: Vec<&str> = if lines.is_empty() {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        lines
            .into_iter()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    tail.join(" | ")
}

/// Build the async 7-Zip command, hiding the extra console window on Windows
/// (tokio's own `creation_flags`, available on Windows builds only).
fn base_command(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

fn is_retryable_spawn_error(code: Option<i32>) -> bool {
    matches!(code, Some(c) if RETRYABLE_SPAWN_ERRORS.contains(&c))
}

/// Format a `SevenZipFailed` whose message is actually actionable in the GUI:
/// executable path, exit code, stderr tail (≤ 1000 chars) and a remedy hint.
fn sevenzip_failure(exe: &Path, code: i32, text: &str) -> UpError {
    let detail = tail_chars(&tail_text(text), ERROR_TAIL_LIMIT);
    let mut message = format!("7-Zip 执行失败（{}，退出码 {code}）", exe.display());
    if !detail.is_empty() {
        message.push_str("：");
        message.push_str(&detail);
    }
    message.push_str("。可设置 UNPARANOIA_7Z 指向已安装的 7-Zip（7zz / 7z.exe）。");
    UpError::SevenZipFailed { code, message }
}

fn spawn_failure(exe: &Path, err: &std::io::Error) -> UpError {
    UpError::SevenZipFailed {
        code: -1,
        message: format!(
            "无法启动 7-Zip（{}）：{err}。可设置 UNPARANOIA_7Z 指向已安装的 7-Zip（7zz / 7z.exe）。",
            exe.display()
        ),
    }
}

/// Keep at most `limit` characters from the END of `text` (the tail is what
/// explains a 7-Zip failure). A truncated result is prefixed with an ellipsis
/// and is still exactly `limit` characters long.
fn tail_chars(text: &str, limit: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= limit {
        return text.to_string();
    }
    let keep = limit.saturating_sub(1);
    let tail: String = text.chars().skip(count - keep).collect();
    format!("…{tail}")
}

fn collect_kv(block: &str, out: &mut BTreeMap<String, String>) {
    for line in block.lines() {
        if let Some((k, v)) = line.split_once(" = ") {
            let k = k.trim();
            if !k.is_empty() && !k.starts_with('-') {
                out.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
}

/// Read one numeric header field (e.g. "Volumes") from raw `-slt` output.
fn header_u32(text: &str, key: &str) -> Option<u32> {
    let needle = format!("{key} = ");
    text.lines()
        .find_map(|line| line.trim().strip_prefix(needle.as_str()))
        .and_then(|value| value.trim().parse().ok())
}

/// Parse `l -slt` output into the archive header + entries.
pub fn parse_slt(text: &str) -> ListResult {
    let text = text.replace("\r\n", "\n");
    let mut result = ListResult::default();

    let (head_part, entry_part) = match text.find("----------") {
        Some(idx) => (&text[..idx], &text[idx + "----------".len()..]),
        None => (text.as_str(), ""),
    };

    let mut header = BTreeMap::new();
    collect_kv(head_part, &mut header);
    result.archive_type = header.get("Type").cloned().unwrap_or_default();
    result.offset = header
        .get("Offset")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    result.stub_size = header
        .get("Embedded Stub Size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    result.physical_size = header
        .get("Physical Size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    result.volumes = header
        .get("Volumes")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    result.total_physical_size = header
        .get("Total Physical Size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    for kv in split_entry_blocks(entry_part) {
        let Some(raw_path) = kv.get("Path") else {
            continue;
        };
        let path = raw_path.replace('\\', "/");
        result.entries.push(ListEntry {
            path,
            raw_path: raw_path.clone(),
            is_dir: kv.get("Folder").map(|v| v == "+").unwrap_or(false),
            size: kv.get("Size").and_then(|v| v.parse().ok()).unwrap_or(0),
            packed: kv
                .get("Packed Size")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            encrypted: kv.get("Encrypted").map(|v| v == "+").unwrap_or(false),
            method: kv.get("Method").cloned().unwrap_or_default(),
        });
    }
    result
}

/// Group the `-slt` entry section into per-entry key/value maps.
///
/// We cannot rely on blank lines between entries: 7-Zip's `-slt` output is read
/// through a line reader that drops empty lines, so entries are separated by a
/// bare `Path = ` line instead.
fn split_entry_blocks(entry_part: &str) -> Vec<BTreeMap<String, String>> {
    let mut blocks: Vec<BTreeMap<String, String>> = Vec::new();
    let mut cur: Option<BTreeMap<String, String>> = None;
    for line in entry_part.lines() {
        if line.starts_with("Path =") {
            if let Some(b) = cur.take() {
                blocks.push(b);
            }
            cur = Some(BTreeMap::new());
        }
        if let Some(b) = cur.as_mut() {
            if let Some((k, v)) = line.split_once(" = ") {
                let k = k.trim();
                if !k.is_empty() && !k.starts_with('-') {
                    b.insert(k.to_string(), v.trim().to_string());
                }
            }
        }
    }
    if let Some(b) = cur.take() {
        blocks.push(b);
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\n7-Zip (z) 25.01 (arm64) : Copyright\n\nScanning the drive for archives:\n1 file, 102616600 bytes (98 MiB)\n\nListing archive: /tmp/a.mp4\n\n--\nPath = /tmp/a.mp4\nType = zip\nPhysical Size = 96324661\nOffset = 6291939\n\n----------\nPath = folder/preview.png\nFolder = -\nSize = 1098098\nPacked Size = 1098064\nEncrypted = +\nMethod = AES-256 Deflate\n\nPath = folder/sub\nFolder = +\nSize = 0\n";

    #[test]
    fn parses_header_and_entries() {
        let r = parse_slt(SAMPLE);
        assert_eq!(r.archive_type, "zip");
        assert_eq!(r.offset, 6291939);
        assert_eq!(r.entries.len(), 2);
        assert!(r.entries[0].encrypted);
        assert!(r.entries[1].is_dir);
        assert_eq!(r.top_level(true), vec!["folder".to_string()]);
        assert_eq!(r.smallest_encrypted().unwrap().path, "folder/preview.png");
        assert_eq!(r.total_uncompressed(), 1098098);
    }

    /// Real `-slt` output goes through a line reader that drops blank lines, so
    /// entries must be split on `Path = ` boundaries, not on empty lines.
    #[test]
    fn parses_entries_without_blank_lines() {
        const NO_BLANKS: &str = "Listing archive: /tmp/a.zip\n\n--\nPath = /tmp/a.zip\nType = zip\nOffset = 4242\n\n----------\nPath = pkg/a.txt\nFolder = -\nSize = 1\nEncrypted = +\nMethod = AES-256 Deflate\nPath = pkg/sub\nFolder = +\nSize = 0\nPath = top2.txt\nFolder = -\nSize = 7\n";
        let r = parse_slt(NO_BLANKS);
        assert_eq!(r.offset, 4242);
        assert_eq!(r.entries.len(), 3, "entries = {:?}", r.entries);
        assert_eq!(r.file_count(), 2);
        assert_eq!(
            r.top_level(true),
            vec!["pkg".to_string(), "top2.txt".to_string()]
        );
        assert_eq!(r.total_uncompressed(), 8);
        assert_eq!(r.smallest_encrypted().unwrap().path, "pkg/a.txt");
    }

    #[test]
    fn argv_mode_injects_password_after_command() {
        let args = vec![
            "t".to_string(),
            "-sccUTF-8".to_string(),
            "a.zip".to_string(),
        ];
        let injected = inject_password(&args, "pw1");
        assert_eq!(injected[0], "t");
        assert_eq!(injected[1], "-ppw1");
        assert_eq!(injected.len(), 4);
        assert_eq!(injected[3], "a.zip");
    }

    #[test]
    fn platform_default_matches_target() {
        #[cfg(windows)]
        assert_eq!(PasswordMode::platform_default(), PasswordMode::Argv);
        #[cfg(not(windows))]
        assert_eq!(PasswordMode::platform_default(), PasswordMode::Stdin);
    }

    #[test]
    fn retryable_spawn_errors_are_windows_lock_codes() {
        assert!(is_retryable_spawn_error(Some(5)));
        assert!(is_retryable_spawn_error(Some(32)));
        assert!(is_retryable_spawn_error(Some(225)));
        assert!(!is_retryable_spawn_error(Some(2)));
        assert!(!is_retryable_spawn_error(None));
    }

    #[test]
    fn failure_messages_are_actionable_and_bounded() {
        let exe = PathBuf::from("/opt/paranoia/binaries/mac/7zz");
        let err = sevenzip_failure(&exe, 2, "ERROR: x\nERROR: y\n");
        assert!(matches!(err, UpError::SevenZipFailed { code: 2, .. }));
        let msg = err.to_string();
        assert!(
            msg.contains("/opt/paranoia/binaries/mac/7zz"),
            "message: {msg}"
        );
        assert!(msg.contains("退出码 2"), "message: {msg}");
        assert!(msg.contains("UNPARANOIA_7Z"), "message: {msg}");

        // A huge stderr tail is truncated from the front, keeping the end.
        let long = format!("PREFIX {} SUFFIX", "x".repeat(5000));
        let msg = sevenzip_failure(&exe, 2, &long).to_string();
        assert!(msg.contains("SUFFIX"), "tail must be kept: {msg}");
        assert!(!msg.contains("PREFIX"), "head must be dropped: {msg}");
        assert!(
            msg.chars().count() < 1200,
            "message too long: {} chars",
            msg.chars().count()
        );

        // Spawn failures still name the executable and the remedy.
        let io_err = std::io::Error::from_raw_os_error(2);
        let msg = spawn_failure(&exe, &io_err).to_string();
        assert!(
            msg.contains("/opt/paranoia/binaries/mac/7zz"),
            "message: {msg}"
        );
        assert!(msg.contains("UNPARANOIA_7Z"), "message: {msg}");
    }

    #[test]
    fn junk_is_ignored_for_top_level() {
        let mut r = ListResult::default();
        r.entries.push(ListEntry {
            path: "__MACOSX/._x".into(),
            raw_path: String::new(),
            is_dir: false,
            size: 1,
            packed: 1,
            encrypted: false,
            method: String::new(),
        });
        r.entries.push(ListEntry {
            path: "root/a.txt".into(),
            raw_path: String::new(),
            is_dir: false,
            size: 2,
            packed: 2,
            encrypted: false,
            method: String::new(),
        });
        assert_eq!(r.top_level(false).len(), 2);
        assert_eq!(r.top_level(true), vec!["root".to_string()]);
    }
}
