//! Public error types. Every variant has a stable `kind()` string so that frontends can
//! localize messages without matching on English text.

use std::path::PathBuf;
use thiserror::Error;

/// Errors returned while decrypting or inspecting an encrypted file.
#[derive(Debug, Error)]
pub enum DecryptError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file is too short to be a Paranoia encrypted file")]
    TooShort,
    #[error("file is truncated or its length does not match the format")]
    Truncated,
    #[error("not a Paranoia encrypted file (bad magic)")]
    BadMagic,
    #[error("unsupported format version: {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported algorithm {0:#04x}; only AES-256 (0x00) is supported")]
    UnsupportedAlgorithm(u8),
    #[error("unsupported KDF custom-params byte {0:#04x}")]
    InvalidCustomParams(u8),
    #[error("file contains a Paranoia recovery container (PFTEREC1), which is not supported in phase 1")]
    UnsupportedRecovery,
    #[error("incorrect password")]
    WrongPassword,
    #[error("file integrity check failed (BLAKE3 MAC mismatch): the file is corrupted or was modified")]
    Integrity,
    #[error("zip archive error: {0}")]
    Zip(String),
    #[error("output file already exists: {0}")]
    OutputExists(PathBuf),
    #[error("invalid output path from archive: {0}")]
    InvalidOutputName(String),
    #[error("crypto error: {0}")]
    Crypto(String),
}

impl DecryptError {
    /// Stable machine readable kind, used by the GUI/CLI for localized user messages.
    pub fn kind(&self) -> &'static str {
        match self {
            DecryptError::Io(_) => "io",
            DecryptError::TooShort => "tooShort",
            DecryptError::Truncated => "truncated",
            DecryptError::BadMagic => "badMagic",
            DecryptError::UnsupportedVersion(_) => "unsupportedVersion",
            DecryptError::UnsupportedAlgorithm(_) => "unsupportedAlgorithm",
            DecryptError::InvalidCustomParams(_) => "invalidCustomParams",
            DecryptError::UnsupportedRecovery => "unsupportedRecovery",
            DecryptError::WrongPassword => "wrongPassword",
            DecryptError::Integrity => "integrity",
            DecryptError::Zip(_) => "zip",
            DecryptError::OutputExists(_) => "outputExists",
            DecryptError::InvalidOutputName(_) => "invalidOutputName",
            DecryptError::Crypto(_) => "crypto",
        }
    }
}

/// Errors returned when validating a path before adding it to the decrypt list.
#[derive(Debug, Error)]
pub enum AddFileError {
    #[error("only .enc files are supported")]
    NotEncExtension,
    #[error("cannot read file: {0}")]
    CannotRead(#[source] std::io::Error),
    #[error("not a regular file")]
    NotRegularFile,
    #[error("file is too short")]
    TooShort,
    #[error("not a Paranoia encrypted file (bad magic)")]
    BadMagic,
    #[error("unsupported format version: {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported algorithm {0:#04x}; only AES-256 (0x00) is supported")]
    UnsupportedAlgorithm(u8),
    #[error("unsupported KDF custom-params byte {0:#04x}")]
    InvalidCustomParams(u8),
}

impl AddFileError {
    pub fn kind(&self) -> &'static str {
        match self {
            AddFileError::NotEncExtension => "notEncExtension",
            AddFileError::CannotRead(_) => "cannotRead",
            AddFileError::NotRegularFile => "notRegularFile",
            AddFileError::TooShort => "tooShort",
            AddFileError::BadMagic => "badMagic",
            AddFileError::UnsupportedVersion(_) => "unsupportedVersion",
            AddFileError::UnsupportedAlgorithm(_) => "unsupportedAlgorithm",
            AddFileError::InvalidCustomParams(_) => "invalidCustomParams",
        }
    }
}

impl From<DecryptError> for AddFileError {
    fn from(err: DecryptError) -> Self {
        match err {
            DecryptError::TooShort | DecryptError::Truncated => AddFileError::TooShort,
            DecryptError::BadMagic => AddFileError::BadMagic,
            DecryptError::UnsupportedVersion(v) => AddFileError::UnsupportedVersion(v),
            DecryptError::UnsupportedAlgorithm(a) => AddFileError::UnsupportedAlgorithm(a),
            DecryptError::InvalidCustomParams(p) => AddFileError::InvalidCustomParams(p),
            DecryptError::Io(e) => AddFileError::CannotRead(e),
            other => AddFileError::CannotRead(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                other.to_string(),
            )),
        }
    }
}

/// Errors returned by the plaintext password store.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot determine the user configuration directory")]
    NoConfigDir,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid config JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl ConfigError {
    pub fn kind(&self) -> &'static str {
        match self {
            ConfigError::NoConfigDir => "noConfigDir",
            ConfigError::Io(_) => "io",
            ConfigError::Json(_) => "json",
        }
    }
}
