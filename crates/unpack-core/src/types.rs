//! Shared data types for the engine (serde-friendly for the Tauri bridge).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    /// Never touch existing files; give the extracted item a free name (`name_1`).
    Rename,
    /// Keep what is already there, drop the freshly extracted item.
    Skip,
    /// Replace existing files/folders with the extracted ones.
    Overwrite,
}

impl Default for ConflictPolicy {
    fn default() -> Self {
        ConflictPolicy::Rename
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutPolicy {
    /// 1 top-level item -> extract into the folder itself; 2+ -> create a folder named after the archive.
    Smart,
    /// Always create a folder named after the archive.
    Folder,
    /// Always extract into the folder itself.
    Direct,
}

impl Default for LayoutPolicy {
    fn default() -> Self {
        LayoutPolicy::Smart
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// Plain archive (.zip/.7z/.rar/...)
    Archive,
    /// Archive hidden inside another container (.mp4 + appended zip, etc.)
    MaskedArchive,
    /// PE executable with an appended archive payload (self-extracting)
    SfxExecutable,
    /// Paranoia File Encryption v4 (SSEFE) container — decrypt first, then treat
    /// the inner ZIP as the source archive.
    SsefeEncrypted,
    NotArchive,
    Unknown,
}

impl Default for FileKind {
    fn default() -> Self {
        FileKind::Unknown
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasswordState {
    NoPassword,
    NeedsPassword,
    Unknown,
}

impl Default for PasswordState {
    fn default() -> Self {
        PasswordState::Unknown
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Detection {
    pub path: PathBuf,
    pub size: u64,
    pub kind: FileKind,
    /// "none" | "pe" | "mp4" | "other" | "ssefe"
    pub container: String,
    pub format: Option<String>,
    /// Byte offset at which the embedded archive payload starts (0 = whole file is the archive).
    pub offset: Option<u64>,
    /// For SFX: size of the executable stub (7-Zip's `Embedded Stub Size`).
    pub stub_size: Option<u64>,
    pub total_entries: usize,
    pub encrypted_entries: usize,
    pub top_level: Vec<String>,
    pub total_uncompressed: u64,
    pub password_state: PasswordState,
    /// Password candidates scraped from archive comments / stub strings.
    pub hints: Vec<String>,
    /// Smallest encrypted entry — used for the ~10 ms password check.
    pub quick_entry: Option<String>,
    /// True when `7zz l` worked without a password (entry-encrypted archives do).
    pub listing_ok_no_pw: bool,
    /// Entry layout looks like an OOXML/JAR document container.
    pub office_like: bool,
    /// Temp file holding a carved-out payload when 7-Zip could not read the container.
    pub carved_path: Option<PathBuf>,
    pub fingerprint: String,
    /// If set, the payload was carved into a temp file starting at this offset.
    pub carved_from: Option<u64>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Success,
    Skipped,
    NeedsPassword,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub task_id: u64,
    pub status: Option<TaskStatus>,
    pub path: PathBuf,
    pub detection: Option<Detection>,
    pub dest: Option<PathBuf>,
    pub files: usize,
    pub bytes: u64,
    pub reason: Option<String>,
    pub password_source: Option<String>,
    pub passwords_tried: usize,
    /// Non-fatal problems (skipped unsafe entries, nested archives kept as-is, ...).
    pub warnings: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskEvent {
    TaskQueued {
        id: u64,
        path: PathBuf,
        size: u64,
    },
    TaskStage {
        id: u64,
        stage: String,
        detail: Option<String>,
    },
    TaskDetected {
        id: u64,
        detection: Detection,
    },
    TaskProgress {
        id: u64,
        percent: Option<u8>,
        line: String,
    },
    TaskPasswordAttempt {
        id: u64,
        source: String,
        ok: bool,
        tried: usize,
    },
    TaskPasswordMissing {
        id: u64,
        tried: usize,
    },
    TaskFinished {
        id: u64,
        outcome: TaskOutcome,
    },
    Log {
        line: String,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub total: usize,
    pub success: usize,
    pub skipped: usize,
    pub needs_password: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub bytes: u64,
    pub elapsed_ms: u64,
}

impl Summary {
    pub fn record(&mut self, o: &TaskOutcome) {
        self.total += 1;
        self.bytes += o.bytes;
        match o.status {
            Some(TaskStatus::Success) => self.success += 1,
            Some(TaskStatus::Skipped) => self.skipped += 1,
            Some(TaskStatus::NeedsPassword) => self.needs_password += 1,
            Some(TaskStatus::Failed) => self.failed += 1,
            Some(TaskStatus::Cancelled) => self.cancelled += 1,
            None => self.failed += 1,
        }
    }
}

/// Upper bound for nested cascade depth (protects against archive bombs).
pub const MAX_NESTED_DEPTH: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunConfig {
    /// Where to extract. `None` = next to each archive.
    pub output_root: Option<PathBuf>,
    pub conflict: ConflictPolicy,
    pub layout: LayoutPolicy,
    /// Concurrent archives.
    pub jobs: usize,
    /// Keep the staging directory when extraction failed (for inspection).
    pub keep_broken: bool,
    /// Try the empty password as a last resort (off by default: 7-Zip treats an empty
    /// `-p` as "prompt me", which we cannot feed interactively).
    pub try_empty_password: bool,
    /// Skip archives whose fingerprint was already extracted successfully.
    pub skip_processed: bool,
    /// Strip `__MACOSX`, `.DS_Store`, `._*`, `Thumbs.db` from the result.
    pub strip_junk: bool,
    /// Cascade-unpack: when the result is a single archive file (e.g. an mp4
    /// that unpacked to one .rar), keep unpacking until final files/folders.
    pub nested: bool,
    /// Maximum nested cascade depth (clamped to `1..=8` when used).
    pub max_depth: usize,
    /// Abort the batch after the first failed / password-required task.
    pub stop_on_error: bool,
    /// Recurse into dropped folders.
    pub recursive: bool,
    /// Detect + plan only, never write anything.
    pub dry_run: bool,
    /// Expand archives that are really Office/JAR containers (.docx/.jar/...).
    pub force_documents: bool,
    /// Windows-only downgrade path: run a self-extracting .exe when its payload
    /// cannot be read as data. Off by default; never used on macOS.
    pub allow_execute_sfx: bool,
    /// Extra passwords to try first (from the CLI `-p`).
    pub passwords: Vec<String>,
    /// Required free space multiplier (e.g. 1.1 = 10% headroom).
    pub min_free_ratio: f64,
    /// Override for the processed-fingerprint store (`processed.json`).
    pub processed_path: Option<PathBuf>,
}

impl RunConfig {
    /// `max_depth` clamped to the supported range. Config files are user-editable,
    /// so never trust the raw value.
    pub fn effective_max_depth(&self) -> usize {
        self.max_depth.clamp(1, MAX_NESTED_DEPTH)
    }
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            output_root: None,
            conflict: ConflictPolicy::default(),
            layout: LayoutPolicy::default(),
            jobs: 2,
            keep_broken: false,
            try_empty_password: false,
            skip_processed: true,
            strip_junk: true,
            nested: true,
            max_depth: 4,
            stop_on_error: false,
            recursive: true,
            dry_run: false,
            force_documents: false,
            allow_execute_sfx: false,
            passwords: Vec::new(),
            min_free_ratio: 1.1,
            processed_path: None,
        }
    }
}

/// Progress line coming out of 7-Zip.
#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub percent: Option<u8>,
    pub line: String,
}
