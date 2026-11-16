//! Password library ("remember passwords") storage.
//!
//! Since v0.3.0 the app keeps multiple passwords, all in plain text in the current user's
//! configuration directory, so the batch engine can try them in order (including on nested
//! archives whose password differs from the outer layer). Nothing ever leaves the machine, the
//! file is excluded from version control, and both GUI and CLI expose a clear action. This is
//! an intentional product decision: the local threat model values convenience over at-rest
//! protection (documented in README).
//!
//! Backwards compatibility: the v0.2.0 layout stored a single remembered_password field in
//! <legacy-dir>/config.json; on first load that file is imported into the new passwords array.
//! The legacy file itself is left untouched.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// File name inside the configuration directory.
pub const CONFIG_FILE_NAME: &str = "config.json";
/// Current configuration directory name (product name since v0.3.0).
pub const CONFIG_DIR_NAME: &str = "upzip";
/// Pre-v0.3.0 configuration directory name, read only for migration.
pub const LEGACY_CONFIG_DIR_NAME: &str = "unparanoia";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    passwords: Vec<String>,
    /// v0.2.0 single password. Read for migration only; never written.
    ///
    /// Both spellings are accepted: the Rust API field and an older camelCase JSON file.
    #[serde(
        default,
        alias = "rememberedPassword",
        skip_serializing_if = "Option::is_none"
    )]
    remembered_password: Option<String>,
}

fn default_config_dir(name: &str) -> Result<PathBuf, ConfigError> {
    #[cfg(target_os = "macos")]
    let dir = {
        let home = std::env::var_os("HOME").ok_or(ConfigError::NoConfigDir)?;
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(name)
    };
    #[cfg(target_os = "windows")]
    let dir = {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("LOCALAPPDATA"))
            .ok_or(ConfigError::NoConfigDir)?;
        PathBuf::from(base).join(name)
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let dir = {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join(name));
            }
        }
        let home = std::env::var_os("HOME").ok_or(ConfigError::NoConfigDir)?;
        PathBuf::from(home).join(".config").join(name)
    };
    Ok(dir)
}

/// Directory used for the password library.
///
/// UNPARANOIA_CONFIG_DIR overrides everything; the pre-v0.3.0 PARANOIA_ENC_CONFIG_DIR is
/// honoured as a fallback for old scripts.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    for key in ["UPZIP_CONFIG_DIR", "UNPARANOIA_CONFIG_DIR", "PARANOIA_ENC_CONFIG_DIR"] {
        if let Some(override_dir) = std::env::var_os(key) {
            if !override_dir.is_empty() {
                return Ok(PathBuf::from(override_dir));
            }
        }
    }
    default_config_dir(CONFIG_DIR_NAME)
}

/// Full path of the plaintext password file.
pub fn config_file_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}

/// Pre-v0.3.0 configuration directory, when it exists on this machine.
///
/// Used to import an old single password (and old settings) once. PARANOIA_ENC_CONFIG_DIR is
/// honoured here as well so tests and portable installs can point at a legacy tree.
pub fn legacy_config_dir() -> Option<PathBuf> {
    if let Some(override_dir) = std::env::var_os("PARANOIA_ENC_CONFIG_DIR") {
        if !override_dir.is_empty() {
            return Some(PathBuf::from(override_dir));
        }
    }
    default_config_dir(LEGACY_CONFIG_DIR_NAME).ok()
}

fn sanitize(passwords: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    passwords.retain(|password| !password.is_empty() && seen.insert(password.clone()));
}

fn read_config(path: &Path) -> Result<Option<ConfigFile>, ConfigError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ConfigError::Io(error)),
    }
}

fn passwords_from(config: ConfigFile) -> Vec<String> {
    let mut passwords = config.passwords;
    if let Some(single) = config.remembered_password {
        passwords.push(single);
    }
    sanitize(&mut passwords);
    passwords
}

fn load_passwords_from(path: &Path, legacy: Option<&Path>) -> Result<Vec<String>, ConfigError> {
    if !path.exists() {
        if let Some(legacy_dir) = legacy {
            let legacy_path = legacy_dir.join(CONFIG_FILE_NAME);
            if let Ok(Some(config)) = read_config(&legacy_path) {
                let passwords = passwords_from(config);
                if !passwords.is_empty() {
                    // Best effort: a read-only legacy tree must never block startup.
                    let _ = save_to(path, &passwords);
                }
            }
        }
    }
    let Some(config) = read_config(path)? else {
        return Ok(Vec::new());
    };
    Ok(passwords_from(config))
}

/// Load the plaintext password library (empty when nothing was saved yet).
///
/// On first run against a pre-v0.3.0 installation, the old single remembered password is
/// imported and persisted to the new location.
pub fn load_passwords() -> Result<Vec<String>, ConfigError> {
    let path = config_file_path()?;
    let legacy = legacy_config_dir();
    load_passwords_from(&path, legacy.as_deref())
}

fn save_to(path: &Path, passwords: &[String]) -> Result<(), ConfigError> {
    let parent = path.parent().ok_or(ConfigError::NoConfigDir)?;
    fs::create_dir_all(parent)?;
    let mut passwords = passwords.to_vec();
    sanitize(&mut passwords);
    let payload = ConfigFile {
        passwords,
        remembered_password: None,
    };
    let json = serde_json::to_vec_pretty(&payload)?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp, path)?;
    Ok(())
}

/// Replace the whole password library (empty entries and duplicates are dropped).
pub fn save_passwords(passwords: &[String]) -> Result<(), ConfigError> {
    save_to(&config_file_path()?, passwords)
}

fn add_password_to(
    path: &Path,
    legacy: Option<&Path>,
    password: &str,
) -> Result<bool, ConfigError> {
    if password.is_empty() {
        return Ok(false);
    }
    let mut passwords = load_passwords_from(path, legacy)?;
    if passwords.iter().any(|existing| existing == password) {
        return Ok(false);
    }
    passwords.push(password.to_string());
    save_to(path, &passwords)?;
    Ok(true)
}

/// Append one password. Returns true when it was added, false when it was empty or already
/// present.
pub fn add_password(password: &str) -> Result<bool, ConfigError> {
    let path = config_file_path()?;
    let legacy = legacy_config_dir();
    add_password_to(&path, legacy.as_deref(), password)
}

fn remove_password_at_from(
    path: &Path,
    legacy: Option<&Path>,
    index: usize,
) -> Result<bool, ConfigError> {
    let mut passwords = load_passwords_from(path, legacy)?;
    if index >= passwords.len() {
        return Ok(false);
    }
    passwords.remove(index);
    save_to(path, &passwords)?;
    Ok(true)
}

/// Remove the password at index (0-based). Returns false when the index is out of range.
pub fn remove_password_at(index: usize) -> Result<bool, ConfigError> {
    let path = config_file_path()?;
    let legacy = legacy_config_dir();
    remove_password_at_from(&path, legacy.as_deref(), index)
}

fn clear_passwords_at(path: &Path) -> Result<(), ConfigError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ConfigError::Io(error)),
    }
}

/// Remove the password file (idempotent).
pub fn clear_passwords() -> Result<(), ConfigError> {
    clear_passwords_at(&config_file_path()?)
}

// ---------------------------------------------------------------- v0.2.0 compatibility API

/// Load the first remembered password (v0.2.0 API; kept for source compatibility).
pub fn load_remembered_password() -> Result<Option<String>, ConfigError> {
    Ok(load_passwords()?.into_iter().next())
}

/// Replace the library with a single password (v0.2.0 API; kept for source compatibility).
pub fn save_remembered_password(password: &str) -> Result<(), ConfigError> {
    save_passwords(&[password.to_string()])
}

/// Remove the whole password file (v0.2.0 API; kept for source compatibility).
pub fn clear_remembered_password() -> Result<(), ConfigError> {
    clear_passwords()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "upzip-core-test-config-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_and_clear() {
        let dir = temp_dir("roundtrip");
        let path = dir.join(CONFIG_FILE_NAME);
        assert_eq!(load_passwords_from(&path, None).unwrap(), Vec::<String>::new());
        save_to(&path, &["123".to_string(), "share me".to_string()]).unwrap();
        assert_eq!(
            load_passwords_from(&path, None).unwrap(),
            vec!["123".to_string(), "share me".to_string()]
        );
        // Plaintext must actually be stored (documented risk).
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("123"));
        assert!(raw.contains("share me"));
        assert!(raw.contains("passwords"));
        assert!(!raw.contains("remembered_password"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        clear_passwords_at(&path).unwrap();
        assert_eq!(load_passwords_from(&path, None).unwrap(), Vec::<String>::new());
        clear_passwords_at(&path).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_sanitizes_empty_and_duplicates() {
        let dir = temp_dir("sanitize");
        let path = dir.join(CONFIG_FILE_NAME);
        save_to(
            &path,
            &[
                "a".to_string(),
                "".to_string(),
                "b".to_string(),
                "a".to_string(),
                "c".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(
            load_passwords_from(&path, None).unwrap(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_and_remove_helpers() {
        let dir = temp_dir("helpers");
        let legacy = dir.join("legacy-missing");
        let path = dir.join(CONFIG_FILE_NAME);

        assert_eq!(load_passwords_from(&path, Some(&legacy)).unwrap(), Vec::<String>::new());
        assert!(add_password_to(&path, Some(&legacy), "one").unwrap());
        assert!(!add_password_to(&path, Some(&legacy), "one").unwrap());
        assert!(!add_password_to(&path, Some(&legacy), "").unwrap());
        assert!(add_password_to(&path, Some(&legacy), "two").unwrap());
        assert_eq!(
            load_passwords_from(&path, Some(&legacy)).unwrap(),
            vec!["one".to_string(), "two".to_string()]
        );

        assert!(remove_password_at_from(&path, Some(&legacy), 0).unwrap());
        assert!(!remove_password_at_from(&path, Some(&legacy), 5).unwrap());
        assert_eq!(
            load_passwords_from(&path, Some(&legacy)).unwrap(),
            vec!["two".to_string()]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_or_empty_file_is_empty() {
        let dir = temp_dir("empty");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, br#"{}"#).unwrap();
        assert_eq!(load_passwords_from(&path, None).unwrap(), Vec::<String>::new());
        fs::write(&path, br#"{"passwords": []}"#).unwrap();
        assert_eq!(load_passwords_from(&path, None).unwrap(), Vec::<String>::new());
        // Older configs may use the camelCase JSON key.
        fs::write(&path, br#"{"rememberedPassword": "camel-pass"}"#).unwrap();
        assert_eq!(
            load_passwords_from(&path, None).unwrap(),
            vec!["camel-pass".to_string()]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn legacy_single_password_is_read_and_migrated() {
        let dir = temp_dir("legacy");
        let legacy_dir = dir.join(LEGACY_CONFIG_DIR_NAME);
        fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_path = legacy_dir.join(CONFIG_FILE_NAME);
        fs::write(&legacy_path, br#"{"remembered_password": "old-pass"}"#).unwrap();
        let new_path = dir.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME);

        assert_eq!(
            load_passwords_from(&new_path, Some(&legacy_dir)).unwrap(),
            vec!["old-pass".to_string()]
        );
        assert!(new_path.exists(), "migration must persist the new file");
        let raw = fs::read_to_string(&new_path).unwrap();
        assert!(raw.contains("old-pass"));
        // Legacy file is left untouched.
        assert!(legacy_path.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_file_path_lives_in_config_dir() {
        let path = config_file_path().unwrap();
        assert!(path.ends_with(CONFIG_FILE_NAME));
        assert!(path.parent().is_some());
        assert!(path.parent().unwrap().ends_with(CONFIG_DIR_NAME));
    }
}
