//! Error type shared by the whole engine.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum UpError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "找不到 7-Zip 可执行文件（已查找 {searched:?}）；请设置环境变量 UNPARANOIA_7Z 指向 7zz/7z.exe，\
         或把 7zz/7z.exe 放到应用同目录或 binaries/ 下"
    )]
    SevenZipMissing { searched: Vec<PathBuf> },
    #[error("7-Zip exited with code {code}: {message}")]
    SevenZipFailed { code: i32, message: String },
    #[error("password required (no password matched)")]
    PasswordRequired,
    #[error("not an archive: {0}")]
    NotAnArchive(PathBuf),
    /// Paranoia File Encryption (SSEFE) container problem, message already user-facing.
    #[error("SSEFE: {0}")]
    Ssefe(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, UpError>;

impl From<std::io::Error> for UpError {
    fn from(e: std::io::Error) -> Self {
        UpError::Other(format!("io error: {e}"))
    }
}

pub fn io_ctx<T>(path: &std::path::Path, r: std::io::Result<T>) -> Result<T> {
    r.map_err(|source| UpError::Io {
        path: path.to_path_buf(),
        source,
    })
}
