//! User settings for the unpack engine, persisted as `settings.json` next to the
//! Paranoia core `config.json` (same configuration directory).
//!
//! The GUI edits these values; the CLI mostly builds a `RunConfig` from flags but uses
//! `Settings::load` for defaults. Unknown/missing fields fall back to `Default`, and a
//! corrupt file never panics — the engine just starts with defaults.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{ConflictPolicy, LayoutPolicy, RunConfig};

pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// Schema version persisted as `settingsVersion`.
///
/// - `0` = field absent (settings.json written by v0.3.0/v0.3.1)
/// - `2` = current: `skipProcessed` defaults to false (explicit opt-in)
pub const SETTINGS_VERSION: u32 = 2;

/// UI colour theme, persisted with the settings and rendered by the GUI only.
///
/// The engine never reads it; it exists here so the Tauri bridge can round-trip
/// the settings file without a second struct.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Where to extract. `None` = next to each archive.
    pub output_root: Option<PathBuf>,
    /// Concurrent archives (clamped to 1..=8 when converted to a `RunConfig`).
    pub jobs: usize,
    pub conflict: ConflictPolicy,
    pub layout: LayoutPolicy,
    /// Skip archives already extracted successfully (fingerprint store).
    pub skip_processed: bool,
    /// Expand archives that are really Office/JAR containers.
    pub force_documents: bool,
    /// Keep the staging directory after a failure.
    pub keep_broken: bool,
    /// Strip `__MACOSX`, `.DS_Store`, `._*`, `Thumbs.db`.
    pub strip_junk: bool,
    /// Cascade-unpack single nested archives.
    pub nested: bool,
    /// Maximum nested cascade depth (clamped to 1..=8).
    pub max_depth: usize,
    /// Abort the batch after the first failed task.
    pub stop_on_error: bool,
    /// Try the empty password as a last resort.
    pub try_empty_password: bool,
    /// Windows-only: run self-extracting executables as a downgrade path.
    pub allow_execute_sfx: bool,
    /// Recurse into dropped folders when collecting inputs.
    pub recursive: bool,
    /// Free-space multiplier required before extraction.
    pub min_free_ratio: f64,
    /// GUI colour theme (system / light / dark). Not part of `RunConfig`.
    pub theme: ThemeMode,
    /// Persisted settings schema version (see [`SETTINGS_VERSION`]).
    ///
    /// Missing in files written before v0.3.2 → `0` (field-level serde
    /// default), which `load_from` migrates once. Always `2` after `save`.
    #[serde(default)]
    pub settings_version: u32,
}

impl Default for Settings {
    fn default() -> Self {
        // Mirrors `RunConfig::default()` so the GUI and the CLI agree.
        let base = RunConfig::default();
        Self {
            output_root: base.output_root,
            jobs: base.jobs,
            conflict: base.conflict,
            layout: base.layout,
            skip_processed: base.skip_processed,
            force_documents: base.force_documents,
            keep_broken: base.keep_broken,
            strip_junk: base.strip_junk,
            nested: base.nested,
            max_depth: base.max_depth,
            stop_on_error: base.stop_on_error,
            try_empty_password: base.try_empty_password,
            allow_execute_sfx: base.allow_execute_sfx,
            recursive: base.recursive,
            min_free_ratio: base.min_free_ratio,
            theme: ThemeMode::System,
            settings_version: SETTINGS_VERSION,
        }
    }
}

impl Settings {
    /// `<config dir>/settings.json`, or `None` when the platform config dir is unavailable.
    pub fn path() -> Option<PathBuf> {
        unparanoia_core::config::config_dir()
            .ok()
            .map(|dir| dir.join(SETTINGS_FILE_NAME))
    }

    /// Load the persisted settings.
    ///
    /// A missing/corrupt file falls back to `Default`. On the first v0.3.0 run the
    /// new `<config dir>/settings.json` does not exist yet, so the pre-rename
    /// `paranoia-enc-decryptor/settings.json` is read as a migration fallback;
    /// the next `save()` always writes to the new location.
    pub fn load() -> Self {
        let new_path = Self::path();
        let legacy_path =
            unparanoia_core::config::legacy_config_dir().map(|dir| dir.join(SETTINGS_FILE_NAME));
        Self::load_with_paths(new_path.as_deref(), legacy_path.as_deref())
    }

    /// Testable core of `load`: the new file wins whenever it exists; the legacy
    /// file is only consulted when the new one is absent.
    fn load_with_paths(new_path: Option<&Path>, legacy_path: Option<&Path>) -> Self {
        if let Some(path) = new_path {
            if path.is_file() {
                return Self::load_from(path);
            }
        }
        if let Some(path) = legacy_path {
            if path.is_file() {
                return Self::load_from(path);
            }
        }
        Self::default()
    }

    /// Load from an explicit path (used by tests and the CLI).
    ///
    /// Files from before v0.3.2 (no `settingsVersion`) are migrated once: the
    /// old `skipProcessed: true` default is reset to `false` so a stale
    /// fingerprint record can never silently hide the user's archive again.
    pub fn load_from(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(mut settings) => {
                    settings.migrate();
                    settings
                }
                Err(_) => Self::default(),
            },
            Err(_) => Self::default(),
        }
    }

    /// One-time in-memory migration of a deserialized settings file.
    fn migrate(&mut self) {
        if self.settings_version < SETTINGS_VERSION {
            self.skip_processed = false;
            self.settings_version = SETTINGS_VERSION;
        }
    }

    /// Persist atomically (temp file + rename) in the config directory.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or_else(|| "无法定位配置目录，设置未保存".to_string())?;
        self.save_to(&path)
    }

    /// Persist atomically to an explicit path.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| format!("序列化设置失败：{e}"))?;
        crate::util::write_file_atomic(path, &json).map_err(|e| format!("写入设置失败：{e}"))
    }

    /// Convert to the engine `RunConfig`. Clamps user-editable values so a bad
    /// `settings.json` cannot make the engine spin up 1000 workers or recurse forever.
    pub fn to_run_config(
        &self,
        passwords: Vec<String>,
        processed_path: Option<PathBuf>,
    ) -> RunConfig {
        RunConfig {
            output_root: self.output_root.clone(),
            conflict: self.conflict,
            layout: self.layout,
            jobs: self.jobs.clamp(1, 8),
            keep_broken: self.keep_broken,
            try_empty_password: self.try_empty_password,
            skip_processed: self.skip_processed,
            strip_junk: self.strip_junk,
            nested: self.nested,
            max_depth: self.max_depth.clamp(1, crate::types::MAX_NESTED_DEPTH),
            stop_on_error: self.stop_on_error,
            recursive: self.recursive,
            dry_run: false,
            force_documents: self.force_documents,
            allow_execute_sfx: self.allow_execute_sfx,
            passwords,
            min_free_ratio: self.min_free_ratio,
            processed_path,
        }
    }
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
            "unparanoia-unpack-settings-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.jobs, 2);
        assert_eq!(s.max_depth, 4);
        assert!(!s.skip_processed, "skip_processed is opt-in since v0.3.2");
        assert_eq!(s.settings_version, SETTINGS_VERSION);
        assert!(s.strip_junk);
        assert!(s.nested);
        assert!(s.recursive);
        assert!(!s.stop_on_error);
        assert!(!s.try_empty_password);
        assert_eq!(s.conflict, ConflictPolicy::Rename);
        assert_eq!(s.layout, LayoutPolicy::Smart);
        assert!((s.min_free_ratio - 1.1).abs() < f64::EPSILON);
    }

    #[test]
    fn serde_uses_camel_case_and_defaults_missing_fields() {
        let json = r#"{"jobs":4,"maxDepth":6,"skipProcessed":false,"outputRoot":"/tmp/out"}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.jobs, 4);
        assert_eq!(s.max_depth, 6);
        assert!(!s.skip_processed);
        assert_eq!(
            s.output_root.as_deref(),
            Some(std::path::Path::new("/tmp/out"))
        );
        // A file without the version field reports 0 until it is loaded/migrated.
        assert_eq!(s.settings_version, 0);
        // Untouched fields use the default.
        assert!(s.strip_junk);
        assert_eq!(s.conflict, ConflictPolicy::Rename);
        // Serialized keys stay camelCase for the GUI bridge.
        let out = serde_json::to_string(&s).unwrap();
        assert!(out.contains("\"outputRoot\""));
        assert!(out.contains("\"maxDepth\""));
        assert!(out.contains("\"skipProcessed\""));
        assert!(!out.contains("output_root"));
    }

    #[test]
    fn round_trip_through_disk() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.jobs = 5;
        s.output_root = Some(dir.join("out"));
        s.max_depth = 2;
        s.save_to(&path).unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.jobs, 5);
        assert_eq!(loaded.max_depth, 2);
        assert_eq!(loaded.output_root, s.output_root);
        assert_eq!(loaded.settings_version, SETTINGS_VERSION);
        // Corrupt / missing files never panic.
        fs::write(&path, b"{not json").unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.jobs, Settings::default().jobs);
        assert_eq!(Settings::load_from(&dir.join("absent.json")).jobs, 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn to_run_config_clamps_user_values() {
        let mut s = Settings::default();
        s.jobs = 0;
        s.max_depth = 0;
        let cfg = s.to_run_config(vec!["pw".into()], None);
        assert_eq!(cfg.jobs, 1);
        assert_eq!(cfg.max_depth, 1);
        s.jobs = 64;
        s.max_depth = 99;
        let cfg = s.to_run_config(vec!["pw".into()], None);
        assert_eq!(cfg.jobs, 8);
        assert_eq!(cfg.max_depth, crate::types::MAX_NESTED_DEPTH);
        assert_eq!(cfg.effective_max_depth(), crate::types::MAX_NESTED_DEPTH);
        assert_eq!(cfg.passwords, vec!["pw".to_string()]);
        assert!(!cfg.dry_run);
        assert_eq!(cfg.processed_path, None);
    }

    #[test]
    fn legacy_files_without_version_migrate_skip_processed() {
        let dir = temp_dir("migrate");
        let path = dir.join("settings.json");

        // v0.3.1 file: skipProcessed defaulted to true and there is no version.
        fs::write(&path, b"{\"skipProcessed\":true}").unwrap();
        let s = Settings::load_from(&path);
        assert!(
            !s.skip_processed,
            "stale skip flag must not survive the upgrade"
        );
        assert_eq!(s.settings_version, SETTINGS_VERSION);

        // Current file: an explicit opt-in is preserved.
        let current = format!("{{\"settingsVersion\":{SETTINGS_VERSION},\"skipProcessed\":true}}");
        fs::write(&path, current).unwrap();
        let s = Settings::load_from(&path);
        assert!(s.skip_processed);
        assert_eq!(s.settings_version, SETTINGS_VERSION);

        // Saving always stamps the current version (migrated exactly once).
        let mut s = Settings::default();
        s.skip_processed = true;
        s.save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"settingsVersion\": 2"), "settings: {text}");
        assert!(text.contains("\"skipProcessed\": true"), "settings: {text}");
        let reloaded = Settings::load_from(&path);
        assert!(reloaded.skip_processed);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn theme_defaults_and_serde_lowercase() {
        assert_eq!(Settings::default().theme, ThemeMode::System);
        // Missing field -> default.
        let s: Settings = serde_json::from_str(r#"{"jobs":2}"#).unwrap();
        assert_eq!(s.theme, ThemeMode::System);
        // Lowercase wire names, matching the GUI dropdown values.
        for (json, expected) in [
            (r#"{"theme":"system"}"#, ThemeMode::System),
            (r#"{"theme":"light"}"#, ThemeMode::Light),
            (r#"{"theme":"dark"}"#, ThemeMode::Dark),
        ] {
            let s: Settings = serde_json::from_str(json).unwrap();
            assert_eq!(s.theme, expected);
        }
        let dark = Settings {
            theme: ThemeMode::Dark,
            ..Default::default()
        };
        let out = serde_json::to_string(&dark).unwrap();
        assert!(out.contains("\"theme\":\"dark\""));
        // The theme is persisted but is not an engine concern.
        let cfg = dark.to_run_config(Vec::new(), None);
        assert_eq!(cfg.jobs, 2);
    }

    #[test]
    fn legacy_settings_are_read_when_new_file_is_missing() {
        let dir = temp_dir("legacy");
        let new_path = dir.join("new").join("settings.json");
        let legacy_path = dir.join("old").join("settings.json");
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();

        // Only the pre-v0.3.0 file exists -> its values are used.
        fs::write(&legacy_path, br#"{"jobs":7,"theme":"dark"}"#).unwrap();
        let s = Settings::load_with_paths(Some(&new_path), Some(&legacy_path));
        assert_eq!(s.jobs, 7);
        assert_eq!(s.theme, ThemeMode::Dark);

        // Once the new file exists it wins and the legacy file is ignored.
        fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        fs::write(&new_path, br#"{"jobs":3,"theme":"light"}"#).unwrap();
        let s = Settings::load_with_paths(Some(&new_path), Some(&legacy_path));
        assert_eq!(s.jobs, 3);
        assert_eq!(s.theme, ThemeMode::Light);

        // Neither file -> defaults.
        let s =
            Settings::load_with_paths(Some(&dir.join("absent.json")), Some(&dir.join("nope.json")));
        assert_eq!(s.jobs, Settings::default().jobs);

        // A corrupt new file still wins over a valid legacy file (no surprise
        // resurrection of old settings) and falls back to defaults.
        fs::write(&new_path, b"{not json").unwrap();
        let s = Settings::load_with_paths(Some(&new_path), Some(&legacy_path));
        assert_eq!(s.jobs, Settings::default().jobs);
        assert_eq!(s.theme, ThemeMode::System);
        fs::remove_dir_all(&dir).unwrap();
    }
}
