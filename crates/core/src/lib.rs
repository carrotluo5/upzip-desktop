//! Core library for decrypting files produced by Paranoia File Encryption (SSEFE v4, AES-256).
//!
//! The crate is deliberately free of GUI, network and async dependencies so that it can be
//! reused from a GUI shell, a CLI or tests.
//!
//! Besides the original single-file `decrypt_file` pipeline it exposes the streaming building
//! blocks used by the v0.2.0 batch unpack engine:
//! * [`decrypt_to_archive`]: decrypt a container to its inner ZIP without extracting it,
//! * [`verify_password`]: cheap checkcode-only password probe for password candidates.

pub mod batch;
pub mod config;
pub mod crypto;
pub mod decrypt;
pub mod error;
pub mod format;
pub mod kdf;
pub mod output;
pub mod zip_extract;
#[cfg(feature = "testkit")]
pub mod testkit;
mod util;

#[cfg(test)]
mod fixture;

pub use batch::{run_batch, BatchOptions, BatchProgress, BatchSummary, FileOutcome};
pub use decrypt::{
    decrypt_file, decrypt_to_archive, inspect_file, validate_input_path, verify_password,
    DecryptArchiveReport, DecryptOptions, DecryptReport,
};
pub use config::{
    add_password, clear_passwords, clear_remembered_password, config_dir, config_file_path,
    legacy_config_dir, load_passwords, load_remembered_password, remove_password_at,
    save_passwords, save_remembered_password, CONFIG_DIR_NAME, CONFIG_FILE_NAME,
    LEGACY_CONFIG_DIR_NAME,
};
pub use error::{AddFileError, ConfigError, DecryptError};
pub use format::{
    parse_argon2_params, parse_header, Argon2Params, Header, ALGORITHM_AES_256, CHECKCODE_LEN,
    FORMAT_VERSION, HEADER_LEN, MAGIC, MAC_LEN, RECOVERY_MAGIC, SALT_LEN,
};
pub use kdf::{derive_keys, derive_l0_password_hash, Keys};
pub use output::{default_output_dir, resolve_output_dir, ConflictStrategy};
