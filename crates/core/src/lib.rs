//! Core library for decrypting files produced by Paranoia File Encryption (SSEFE v4, AES-256).
//!
//! The crate is deliberately free of GUI, network and async dependencies so that it can be
//! reused from a GUI shell, a CLI or tests.

pub mod batch;
pub mod config;
pub mod crypto;
pub mod decrypt;
pub mod error;
pub mod format;
pub mod kdf;
pub mod output;
pub mod zip_extract;
mod util;

#[cfg(test)]
mod fixture;

pub use batch::{run_batch, BatchOptions, BatchProgress, BatchSummary, FileOutcome};
pub use decrypt::{decrypt_file, inspect_file, validate_input_path, DecryptOptions, DecryptReport};
pub use config::{
    clear_remembered_password, config_file_path, load_remembered_password,
    save_remembered_password,
};
pub use error::{AddFileError, ConfigError, DecryptError};
pub use format::{
    parse_argon2_params, parse_header, Argon2Params, Header, ALGORITHM_AES_256, CHECKCODE_LEN,
    FORMAT_VERSION, HEADER_LEN, MAGIC, MAC_LEN, RECOVERY_MAGIC, SALT_LEN,
};
pub use kdf::{derive_keys, derive_l0_password_hash, Keys};
pub use output::{default_output_dir, resolve_output_dir, ConflictStrategy};
