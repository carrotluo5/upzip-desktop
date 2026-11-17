//! 把 7-Zip 运行时（`7z.exe` + `7z.dll`）内嵌进 Windows 单文件发行版。
//!
//! 背景：v0.3.0 的 Windows 便携包把 `7z.exe` / `7z.dll` 作为侧车文件放在 exe 旁边，
//! 用户要求“只有一个 exe、不依赖任何外部文件”。本 crate 用 `include_bytes!`
//! 把两个二进制打进调用方的可执行文件，首次运行（或缓存丢失时）解压到
//! `%LOCALAPPDATA%\Upzip\embedded-7z\<版本>-<修订>`，之后复用缓存，
//! 返回可执行文件路径给 [`upzip_unpack_core::SevenZip::locate`] 使用。
//!
//! - 非 Windows 平台：函数直接返回 `Ok(None)`，不占用体积（macOS .app 仍从
//!   `Contents/Resources/binaries/mac/7zz` 读取 7-Zip）。
//! - 缓存目录可用环境变量 `UPZIP_7Z_CACHE_DIR` 覆盖（测试/受限环境）。
//! - 写入采用“唯一临时文件 + rename”的原子方式，多进程同时启动不会互相踩踏。
//!
//! 7-Zip 以 GNU LGPL + unRAR 限制分发，许可证全文随本 crate 内嵌，
//! 通过 [`LICENSE_TEXT`] 暴露；GUI“关于/设置”与 CLI `--licenses` 必须展示。

use std::path::PathBuf;

/// 内嵌 7-Zip 的版本号（与 `binaries/win/7z.exe` 的实际版本一致）。
pub const SEVEN_ZIP_VERSION: &str = "25.01";

/// 缓存布局修订号：内嵌策略/目录结构变化时 +1，强制旧缓存失效。
pub const CACHE_REV: u32 = 1;

/// 便携/内嵌模式使用的 7-Zip 可执行文件名。
pub const EXE_FILE: &str = "7z.exe";
/// 7-Zip 控制台版依赖的 DLL 文件名。
pub const DLL_FILE: &str = "7z.dll";

/// 7-Zip 许可证全文（LGPL 2.1 + unRAR 限制），必须随发行版展示。
pub const LICENSE_TEXT: &str = include_str!("../../../binaries/7z-LICENSE.txt");

/// 内嵌运行时的错误。
#[derive(Debug, thiserror::Error)]
pub enum Embedded7zError {
    /// 读写缓存目录失败。
    #[error("内嵌 7-Zip 解压失败：{0}")]
    Io(#[from] std::io::Error),
    /// 找不到可写的缓存目录。
    #[error("内嵌 7-Zip 初始化失败：找不到可写的缓存目录（已尝试 LOCALAPPDATA 与临时目录）")]
    NoCacheDir,
}

/// 确保内嵌的 7-Zip 已解压到本地缓存，返回 `7z.exe` 的路径。
///
/// Windows 上返回 `Ok(Some(path))`；其他平台返回 `Ok(None)`（不内嵌）。
/// 调用方把这个路径作为 `SevenZip::locate` 的显式路径，从而优先于外部
/// 7-Zip 安装；用户仍可用 `UPZIP_7Z`（兼容旧名 `UNPARANOIA_7Z`）显式覆盖
/// （locate 里环境变量先于内嵌路径时需注意顺序，见调用方实现）。
pub fn ensure_7z() -> Result<Option<PathBuf>, Embedded7zError> {
    #[cfg(windows)]
    {
        imp::ensure_7z()
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

/// Cache folder for the embedded 7-Zip runtime.
///
/// `UPZIP_7Z_CACHE_DIR` wins, the pre-0.4.0 `UNPARANOIA_7Z_CACHE_DIR` still works;
/// both get a version/revision leaf so an upgrade never reuses stale binaries.
#[cfg(any(windows, test))]
fn cache_dir() -> Result<PathBuf, Embedded7zError> {
    let leaf = format!("{SEVEN_ZIP_VERSION}-r{CACHE_REV}");
    if let Some(raw) = std::env::var_os("UPZIP_7Z_CACHE_DIR")
        .or_else(|| std::env::var_os("UNPARANOIA_7Z_CACHE_DIR"))
    {
        if !raw.is_empty() {
            return Ok(PathBuf::from(raw).join(leaf));
        }
    }
    if let Some(raw) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(raw)
            .join("Upzip")
            .join("embedded-7z")
            .join(leaf));
    }
    let temp = std::env::temp_dir();
    if temp.as_os_str().is_empty() {
        return Err(Embedded7zError::NoCacheDir);
    }
    Ok(temp.join("upzip-embedded-7z").join(leaf))
}

#[cfg(windows)]
mod imp {
    use super::{
        cache_dir, Embedded7zError, CACHE_REV, DLL_FILE, EXE_FILE, SEVEN_ZIP_VERSION,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 内嵌的 7-Zip 二进制（编译期打进 exe）。
    static EXE_BYTES: &[u8] = include_bytes!("../../../binaries/win/7z.exe");
    static DLL_BYTES: &[u8] = include_bytes!("../../../binaries/win/7z.dll");
    static LICENSE_BYTES: &[u8] = include_bytes!("../../../binaries/win/7z-LICENSE.txt");
    static LICENSE_FALLBACK: &[u8] = include_bytes!("../../../binaries/7z-LICENSE.txt");

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    pub(super) fn ensure_7z() -> Result<Option<PathBuf>, Embedded7zError> {
        let dir = cache_dir()?;
        let exe = dir.join(EXE_FILE);
        let dll = dir.join(DLL_FILE);
        if has_expected_size(&exe, EXE_BYTES.len()) && has_expected_size(&dll, DLL_BYTES.len()) {
            return Ok(Some(exe));
        }
        fs::create_dir_all(&dir)?;
        write_atomic(&exe, EXE_BYTES)?;
        write_atomic(&dll, DLL_BYTES)?;
        // 许可证文本随缓存放一份，方便用户/审计查看；写入失败不影响运行。
        let license = if LICENSE_BYTES.is_empty() {
            LICENSE_FALLBACK
        } else {
            LICENSE_BYTES
        };
        let _ = write_atomic(&dir.join("7z-LICENSE.txt"), license);
        Ok(Some(exe))
    }

    fn has_expected_size(path: &Path, expected: usize) -> bool {
        fs::metadata(path)
            .map(|m| m.is_file() && m.len() == expected as u64)
            .unwrap_or(false)
    }

    fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Embedded7zError> {
        let tmp = unique_temp(path);
        fs::write(&tmp, bytes)?;
        // Windows 的 rename 不覆盖已存在文件：先尽力删除目标，再 rename。
        // 另一进程同时写入同样的内容时，重复删除/覆盖是良性竞争。
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&tmp);
                Err(Embedded7zError::Io(error))
            }
        }
    }

    fn unique_temp(path: &Path) -> PathBuf {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{}.{}.{}.tmp", std::process::id(), nanos, n));
        path.with_file_name(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn license_text_is_embedded() {
        assert!(LICENSE_TEXT.contains("7-Zip"), "license text missing");
        assert!(LICENSE_TEXT.len() > 500);
    }

    #[test]
    fn version_is_reported() {
        assert_eq!(SEVEN_ZIP_VERSION, "25.01");
        assert!(CACHE_REV >= 1);
    }

    /// The renamed cache variable wins, the pre-0.4.0 name still works.
    #[test]
    fn cache_dir_env_override_prefers_new_name() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let leaf = format!("{SEVEN_ZIP_VERSION}-r{CACHE_REV}");
        let previous_new = std::env::var_os("UPZIP_7Z_CACHE_DIR");
        let previous_legacy = std::env::var_os("UNPARANOIA_7Z_CACHE_DIR");
        std::env::remove_var("UPZIP_7Z_CACHE_DIR");
        std::env::set_var("UNPARANOIA_7Z_CACHE_DIR", "/legacy/cache");
        assert_eq!(
            cache_dir().unwrap(),
            PathBuf::from("/legacy/cache").join(&leaf)
        );
        std::env::set_var("UPZIP_7Z_CACHE_DIR", "/new/cache");
        assert_eq!(cache_dir().unwrap(), PathBuf::from("/new/cache").join(&leaf));
        match previous_new {
            Some(value) => std::env::set_var("UPZIP_7Z_CACHE_DIR", value),
            None => std::env::remove_var("UPZIP_7Z_CACHE_DIR"),
        }
        match previous_legacy {
            Some(value) => std::env::set_var("UNPARANOIA_7Z_CACHE_DIR", value),
            None => std::env::remove_var("UNPARANOIA_7Z_CACHE_DIR"),
        }
    }

    #[test]
    fn non_windows_is_a_noop() {
        #[cfg(not(windows))]
        {
            assert_eq!(ensure_7z().expect("noop"), None);
        }
    }
}
