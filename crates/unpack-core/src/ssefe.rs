//! Paranoia File Encryption v4 (`SSEFE`) integration.
//!
//! Two responsibilities:
//! * probe the plaintext header (`enc_probe` / `probe_head`) so detection can classify a
//!   container without shelling out to 7-Zip, and
//! * decrypt a container to a temporary inner ZIP (`decrypt_to_temp`) that the normal
//!   7-Zip list/plan/extract pipeline can consume.
//!
//! This is the only module that talks to `unparanoia_core`; everything above works on
//! the returned plain values.
//!
//! Security note: the decrypted plaintext ZIP is written to a `TMP_PREFIX`-named file next
//! to the output root (or the system temp dir). It is removed by the RAII guard in this
//! module on failure, by the caller's guard after extraction, and by the CLI
//! `cleanup-staging` sweep on the next run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use unparanoia_core::error::DecryptError;
use unparanoia_core::format::{
    parse_argon2_params, parse_header, ALGORITHM_AES_256, FORMAT_VERSION, HEADER_LEN, MAGIC,
    SALT_LEN,
};

use crate::cancel::CancelToken;
use crate::types::TaskEvent;

/// Minimum size of a valid SSEFE v4 container: header + salt + checkcode + MAC.
pub const SSEFE_MIN_LEN: u64 = 104;

const PROBE_PREFIX_LEN: usize = HEADER_LEN + SALT_LEN;
const PROGRESS_POLL_MS: u64 = 120;

/// Everything detection needs to know about a Paranoia v4 header.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SsefeProbe {
    pub path: PathBuf,
    pub size: u64,
    pub version: u8,
    pub algorithm: u8,
    pub custom_params: u8,
    /// Argon2id time-cost nibble from `customParams` (low nibble).
    pub tm: u8,
    /// Argon2id memory nibble from `customParams` (high nibble).
    pub mm: u8,
    pub iterations: u32,
    pub memory_kib: u32,
    pub parallelism: u32,
}

impl SsefeProbe {
    /// Short human readable line used in detection notes.
    pub fn describe(&self) -> String {
        format!(
            "Paranoia File Encryption v4 (SSEFE) AES-256 · Argon2id t={} m={}KiB p={} · {}",
            self.iterations,
            self.memory_kib,
            self.parallelism,
            crate::util::human_bytes(self.size)
        )
    }
}

/// Inspect an already-read file head. Returns `Some` only for a real SSEFE v4 container:
/// magic `SSEFE`, file length >= 104, header version 4 and AES-256.
///
/// `.enc` files whose magic is something else (renamed archives) yield `None` and are
/// handed to the normal archive detection.
pub fn probe_head(head: &[u8], size: u64, path: &Path) -> Option<SsefeProbe> {
    if size < SSEFE_MIN_LEN || head.len() < PROBE_PREFIX_LEN {
        return None;
    }
    if !head.starts_with(&MAGIC) {
        return None;
    }
    let header = parse_header(&head[..PROBE_PREFIX_LEN]).ok()?;
    if header.version != FORMAT_VERSION || header.algorithm != ALGORITHM_AES_256 {
        return None;
    }
    let params = parse_argon2_params(header.custom_params).ok()?;
    Some(SsefeProbe {
        path: path.to_path_buf(),
        size,
        version: header.version,
        algorithm: header.algorithm,
        custom_params: header.custom_params,
        tm: header.custom_params & 0x0f,
        mm: header.custom_params >> 4,
        iterations: params.iterations,
        memory_kib: params.memory_kib,
        parallelism: params.parallelism,
    })
}

/// Probe a file on disk (opens it and reads the 40-byte prefix).
pub fn enc_probe(path: &Path) -> Option<SsefeProbe> {
    use std::io::Read;
    let size = std::fs::metadata(path).ok()?.len();
    if size < SSEFE_MIN_LEN {
        return None;
    }
    let mut head = [0u8; PROBE_PREFIX_LEN];
    let mut file = std::fs::File::open(path).ok()?;
    file.read_exact(&mut head).ok()?;
    probe_head(&head, size, path)
}

/// Errors surfaced while decrypting an SSEFE container. Messages are user-facing Chinese.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SsefeError {
    Cancelled,
    /// Checkcode mismatch: the password is wrong.
    WrongPassword,
    /// BLAKE3 MAC mismatch: the file is corrupted / was modified.
    Integrity,
    /// File too short or length does not match the format.
    Truncated,
    /// Not a Paranoia v4 container.
    NotSsefe,
    Io(String),
    Other(String),
}

impl SsefeError {
    /// Stable machine readable kind (mirrors `unparanoia_core::DecryptError::kind`).
    pub fn kind(&self) -> &'static str {
        match self {
            SsefeError::Cancelled => "cancelled",
            SsefeError::WrongPassword => "wrongPassword",
            SsefeError::Integrity => "integrity",
            SsefeError::Truncated => "truncated",
            SsefeError::NotSsefe => "notSsefe",
            SsefeError::Io(_) => "io",
            SsefeError::Other(_) => "other",
        }
    }

    /// Localized message suitable for `TaskOutcome.reason`.
    pub fn message(&self) -> String {
        match self {
            SsefeError::Cancelled => "已取消".to_string(),
            SsefeError::WrongPassword => "密码错误（口令不正确）".to_string(),
            SsefeError::Integrity => "完整性校验失败：文件已损坏或被修改".to_string(),
            SsefeError::Truncated => "文件不完整（长度与 Paranoia v4 格式不符）".to_string(),
            SsefeError::NotSsefe => "不是 Paranoia File Encryption v4 (SSEFE) 加密文件".to_string(),
            SsefeError::Io(message) => format!("读写失败：{message}"),
            SsefeError::Other(message) => message.clone(),
        }
    }
}

impl std::fmt::Display for SsefeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for SsefeError {}

impl From<DecryptError> for SsefeError {
    fn from(error: DecryptError) -> Self {
        match error {
            DecryptError::Io(error) => SsefeError::Io(error.to_string()),
            DecryptError::TooShort | DecryptError::Truncated => SsefeError::Truncated,
            DecryptError::BadMagic => SsefeError::NotSsefe,
            DecryptError::UnsupportedVersion(version) => SsefeError::Other(format!(
                "不支持的 Paranoia 格式版本：{version}（仅支持 v4）"
            )),
            DecryptError::UnsupportedAlgorithm(algorithm) => SsefeError::Other(format!(
                "不支持的加密算法 {algorithm:#04x}（仅支持 AES-256）"
            )),
            DecryptError::InvalidCustomParams(params) => {
                SsefeError::Other(format!("无效的 KDF 参数自定义字节 {params:#04x}"))
            }
            DecryptError::UnsupportedRecovery => SsefeError::Other(
                "该文件是 Paranoia 恢复容器（PFTEREC1），当前版本不支持".to_string(),
            ),
            DecryptError::WrongPassword => SsefeError::WrongPassword,
            DecryptError::Integrity => SsefeError::Integrity,
            other => SsefeError::Other(other.to_string()),
        }
    }
}

/// Cheap checkcode-only password check: `Ok(true)` = password matches.
/// Errors other than `WrongPassword` (bad/truncated container) are propagated so the
/// engine can distinguish "no candidate matched" from "the file itself is broken".
pub fn verify_password(input: &Path, password: &str) -> Result<bool, SsefeError> {
    match unparanoia_core::verify_password(input, password) {
        Ok(()) => Ok(true),
        Err(DecryptError::WrongPassword) => Ok(false),
        Err(error) => Err(SsefeError::from(error)),
    }
}

/// Length of the Paranoia v4 plaintext checkcode that precedes the inner ZIP.
const PLAINTEXT_CHECKCODE_LEN: usize = 32;

/// Normalize the archive produced by `unparanoia_core::decrypt_to_archive`.
///
/// The core currently writes the *whole* plaintext, whose first 32 bytes are the
/// `[a-z0-9]` checkcode, so the inner ZIP starts at offset 32. 7-Zip refuses a ZIP with a
/// leading payload ("Cannot open the file as [zip] archive"), so strip the prefix here:
///
/// * file already starts with `PK` -> untouched (`Ok(false)`),
/// * offset 32 starts with `PK` and the leading 32 bytes look like a checkcode -> the prefix
///   is stream-copied away (`Ok(true)`),
/// * anything else -> untouched, so genuine corruption still surfaces as "not an archive".
///
/// This is a compatibility shim: if core starts emitting a pure ZIP, it becomes a no-op.
fn normalize_inner_zip(path: &Path) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};

    let mut head = [0u8; PLAINTEXT_CHECKCODE_LEN + 2];
    let mut file = std::fs::File::open(path)?;
    let read = file.read(&mut head)?;
    if read >= 2 && &head[..2] == b"PK" {
        return Ok(false);
    }
    if read < head.len() || &head[PLAINTEXT_CHECKCODE_LEN..] != b"PK" {
        return Ok(false);
    }
    let checkcode = &head[..PLAINTEXT_CHECKCODE_LEN];
    if !checkcode
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Ok(false);
    }

    let stripped = path.with_file_name(format!(
        "{}{}.zip",
        crate::TMP_PREFIX,
        uuid::Uuid::new_v4().simple()
    ));
    let copied = (|| -> std::io::Result<()> {
        file.seek(SeekFrom::Start(PLAINTEXT_CHECKCODE_LEN as u64))?;
        let mut out = std::fs::File::create(&stripped)?;
        std::io::copy(&mut file, &mut out)?;
        out.sync_all()?;
        Ok(())
    })();
    match copied {
        Ok(()) => {
            std::fs::rename(&stripped, path)?;
            Ok(true)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&stripped);
            Err(error)
        }
    }
}

/// Deletes the temp file unless disarmed (success hands ownership to the caller).
struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Hand the path to the caller; the guard no longer deletes it.
    fn disarm(mut self) -> PathBuf {
        debug_assert!(self.path.is_some());
        self.path.take().unwrap_or_default()
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Decrypt `input` with `password` into exactly `out_path` (parent folders are
/// created when missing). Progress is reported as whole percent via `on_percent`.
///
/// The output file is removed on failure or cancellation. On success the caller owns
/// the plaintext file and must delete it once it is no longer needed.
pub async fn decrypt_to_path(
    input: &Path,
    password: &str,
    out_path: &Path,
    cancel: &CancelToken,
    on_percent: &mut (dyn FnMut(u8) + Send),
) -> Result<(), SsefeError> {
    if cancel.is_cancelled() {
        return Err(SsefeError::Cancelled);
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            SsefeError::Io(format!("无法创建临时目录 {}: {error}", parent.display()))
        })?;
    }
    let guard = TempFileGuard::new(out_path.to_path_buf());

    let processed = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let processed_cb = Arc::clone(&processed);
    let total_cb = Arc::clone(&total);
    let input_owned = input.to_path_buf();
    let password_owned = password.to_string();
    let out_owned = out_path.to_path_buf();

    let mut handle = tokio::task::spawn_blocking(move || {
        unparanoia_core::decrypt_to_archive(
            &input_owned,
            &password_owned,
            &out_owned,
            &mut |done, total_bytes| {
                processed_cb.store(done, Ordering::Relaxed);
                if total_bytes > 0 {
                    total_cb.store(total_bytes, Ordering::Relaxed);
                }
            },
        )
        .map(|_report| ())
    });

    let mut last_percent: i16 = 0;
    let finished = loop {
        tokio::select! {
            joined = &mut handle => {
                break joined;
            }
            _ = cancel.wait_cancelled() => {
                // 阻塞解密无法真正中断：等它结束，保证半成品明文不会留在磁盘上。
                let _ = handle.await;
                return Err(SsefeError::Cancelled);
            }
            _ = tokio::time::sleep(Duration::from_millis(PROGRESS_POLL_MS)) => {
                let done = processed.load(Ordering::Relaxed);
                let total_bytes = total.load(Ordering::Relaxed);
                if total_bytes > 0 {
                    let percent = ((done.min(total_bytes) * 100) / total_bytes).min(99) as u8;
                    if i16::from(percent) > last_percent {
                        last_percent = i16::from(percent);
                        on_percent(percent);
                    }
                }
            }
        }
    };

    match finished {
        Ok(Ok(())) => {
            on_percent(100);
            guard.disarm();
            Ok(())
        }
        Ok(Err(error)) => Err(SsefeError::from(error)),
        Err(join_error) => Err(SsefeError::Other(format!("解密线程异常终止：{join_error}"))),
    }
}

/// Decrypt `input` with `password` into a fresh `TMP_PREFIX`-named `.zip` under
/// `staging_parent` (or the system temp dir) and return its path.
///
/// Progress is reported as `TaskEvent::TaskProgress` (percent 0..=100) in the `decrypt`
/// stage. The core decryption has no cancellation entry point, so cancelling waits for the
/// blocking worker to finish and then removes the partial plaintext before returning
/// `SsefeError::Cancelled`.
pub async fn decrypt_to_temp(
    id: u64,
    input: &Path,
    password: &str,
    staging_parent: Option<&Path>,
    cancel: &CancelToken,
    emit: &mut (dyn FnMut(TaskEvent) + Send),
) -> Result<PathBuf, SsefeError> {
    if cancel.is_cancelled() {
        return Err(SsefeError::Cancelled);
    }
    let dir = match staging_parent {
        Some(parent) if parent.is_dir() => parent.to_path_buf(),
        _ => std::env::temp_dir(),
    };
    std::fs::create_dir_all(&dir)
        .map_err(|error| SsefeError::Io(format!("无法创建临时目录 {}: {error}", dir.display())))?;
    let out_zip = dir.join(format!(
        "{}{}.zip",
        crate::TMP_PREFIX,
        uuid::Uuid::new_v4().simple()
    ));

    emit(TaskEvent::TaskProgress {
        id,
        percent: Some(0),
        line: "SSEFE v4 解密 0%".to_string(),
    });
    {
        let mut last_percent: i16 = 0;
        let mut on_percent = |percent: u8| {
            // 100% is reported once by decrypt_to_temp after normalization.
            if percent >= 100 || i16::from(percent) <= last_percent {
                return;
            }
            last_percent = i16::from(percent);
            emit(TaskEvent::TaskProgress {
                id,
                percent: Some(percent),
                line: format!("SSEFE v4 解密 {percent}%"),
            });
        };
        decrypt_to_path(input, password, &out_zip, cancel, &mut on_percent).await?;
    }

    // decrypt_to_path disarmed its guard on success; take ownership again so a
    // normalization failure cannot leave plaintext behind.
    let guard = TempFileGuard::new(out_zip.clone());
    if let Err(error) = normalize_inner_zip(&out_zip) {
        return Err(SsefeError::Io(format!(
            "无法规范化解密后的 ZIP（{}）：{error}",
            out_zip.display()
        )));
    }
    emit(TaskEvent::TaskProgress {
        id,
        percent: Some(100),
        line: "SSEFE v4 解密完成".to_string(),
    });
    Ok(guard.disarm())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "paranoia-unpack-ssefe-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a syntactically valid 104-byte SSEFE v4 header
    /// (`customParams` = 0x00 -> Argon2id t=10, m=10240 KiB, p=1).
    fn fake_ssefe(len: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(len.max(SSEFE_MIN_LEN as usize));
        bytes.extend_from_slice(b"SSEFE\x04\x00\x00"); // magic + version 4 + AES-256 + customParams 0
        bytes.extend_from_slice(&[0x11u8; SALT_LEN]);
        bytes.resize(len.max(SSEFE_MIN_LEN as usize), 0x5a);
        bytes
    }

    #[test]
    fn probe_head_accepts_valid_container() {
        let bytes = fake_ssefe(2048);
        let probe = probe_head(&bytes, bytes.len() as u64, Path::new("secret.enc")).unwrap();
        assert_eq!(probe.version, 4);
        assert_eq!(probe.algorithm, ALGORITHM_AES_256);
        assert_eq!(probe.custom_params, 0);
        assert_eq!(probe.tm, 0);
        assert_eq!(probe.mm, 0);
        assert_eq!(probe.iterations, 10);
        assert_eq!(probe.memory_kib, 10240);
        assert_eq!(probe.parallelism, 4);
        assert_eq!(probe.size, 2048);
        assert!(probe.describe().contains("Paranoia File Encryption v4"));
    }

    #[test]
    fn probe_head_rejects_truncated_renamed_and_wrong_version() {
        // Too short overall (< 104 bytes) even though the header parses.
        let short = fake_ssefe(80);
        assert!(probe_head(&short, 80, Path::new("a.enc")).is_none());
        // Renamed zip: not SSEFE.
        let zip = b"PK\x03\x04rest-of-a-zip-file-".to_vec();
        assert!(probe_head(&zip, zip.len() as u64, Path::new("renamed.enc")).is_none());
        // Header with an unsupported version is not "SSEFE v4".
        let mut wrong_version = fake_ssefe(SSEFE_MIN_LEN as usize);
        wrong_version[5] = 3;
        assert!(probe_head(
            &wrong_version,
            wrong_version.len() as u64,
            Path::new("v3.enc")
        )
        .is_none());
    }

    #[test]
    fn enc_probe_reads_from_disk() {
        let dir = temp_dir("probe");
        let path = dir.join("secret.enc");
        fs::write(&path, fake_ssefe(4096)).unwrap();
        let probe = enc_probe(&path).unwrap();
        assert_eq!(probe.path, path);
        assert_eq!(probe.size, 4096);
        // A missing file is not a probe error path (returns None, never panics).
        assert!(enc_probe(&dir.join("missing.enc")).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn normalize_strips_checkcode_prefix_only_for_real_zip() {
        let dir = temp_dir("normalize");

        // checkcode(32) || zip -> prefix stripped.
        let mut prefixed = b"0123456789abcdefghijklmnopqrstuv".to_vec();
        prefixed.extend_from_slice(b"PK\x03\x04zip-body");
        let prefixed_path = dir.join("prefixed.zip");
        fs::write(&prefixed_path, &prefixed).unwrap();
        assert!(normalize_inner_zip(&prefixed_path).unwrap());
        assert_eq!(
            fs::read(&prefixed_path).unwrap(),
            b"PK\x03\x04zip-body".to_vec()
        );

        // Already a ZIP -> untouched.
        let plain_path = dir.join("plain.zip");
        fs::write(&plain_path, b"PK\x03\x04zip-body").unwrap();
        assert!(!normalize_inner_zip(&plain_path).unwrap());
        assert_eq!(
            fs::read(&plain_path).unwrap(),
            b"PK\x03\x04zip-body".to_vec()
        );

        // Not a checkcode -> untouched (corruption must stay visible).
        let garbage_path = dir.join("garbage.bin");
        fs::write(&garbage_path, b"XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXPK\x03\x04").unwrap();
        assert!(!normalize_inner_zip(&garbage_path).unwrap());
        assert_eq!(
            fs::read(&garbage_path).unwrap(),
            b"XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXPK\x03\x04".to_vec()
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn error_messages_are_distinct_and_localized() {
        let wrong = SsefeError::WrongPassword;
        let integrity = SsefeError::Integrity;
        assert!(wrong.message().contains("密码错误"));
        assert!(integrity.message().contains("完整性校验失败"));
        assert_ne!(wrong.kind(), integrity.kind());
        assert_eq!(SsefeError::Cancelled.message(), "已取消");
        assert!(SsefeError::from(DecryptError::Integrity)
            .message()
            .contains("完整性"));
        assert!(SsefeError::from(DecryptError::WrongPassword)
            .message()
            .contains("密码错误"));
        assert!(SsefeError::from(DecryptError::BadMagic)
            .message()
            .contains("不是 Paranoia"));
    }
}
