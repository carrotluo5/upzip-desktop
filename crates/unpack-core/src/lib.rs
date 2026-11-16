//! upzip-unpack-core: detection + smart extraction engine for archives,
//! masked archives, self-extracting executables and Paranoia File Encryption v4
//! (`SSEFE`) containers. UI-agnostic; driven by the CLI or the Tauri app.

pub mod cancel;
pub mod detect;
pub mod engine;
pub mod error;
pub mod layout;
pub mod processed;
pub mod queue;
pub mod settings;
pub mod sevenzip;
pub mod sfx;
pub mod split;
pub mod ssefe;
pub mod types;
pub mod util;

pub use cancel::{CancelToken, PauseGate};
pub use detect::detect;
pub use engine::Engine;
pub use error::{Result, UpError};
pub use layout::{archive_stem, plan_dest, strip_junk_tree};
pub use processed::ProcessedStore;
pub use queue::{expand_inputs, run_batch, Sink};
pub use settings::Settings;
pub use sevenzip::{PasswordMode, PwCheck, SevenZip};
pub use ssefe::{decrypt_to_path, decrypt_to_temp, enc_probe, SsefeError, SsefeProbe};
pub use types::*;

/// Name prefix of staging folders/files. Cleaned up on the next run by
/// `cleanup-staging` (CLI) and matched by the GUI's `stagingPrefix` app info.
pub const TMP_PREFIX: &str = ".upzip-tmp-";
