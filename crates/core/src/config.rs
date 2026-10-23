//! Password memory ("remember password") storage.
//!
//! The remembered password is stored in **plain text** in the current user's configuration
//! directory so the GUI can pre-fill it on the next launch. It never leaves the machine and the
//! configuration file is excluded from version control. The GUI/CLI expose a one-click clear.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

const CONFIG_FILE_NAME: &str = "config.json";
const CONFIG_DIR_NAME: &str = "paranoia-enc-decryptor";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(rename = "remembered_password", default)]
    remembered_password: Option<String>,
}

/// Directory used for the password store. \`PARANOIA_ENC_CONFIG_DIR\` overrides everything.
pub fn config_dir() -> Result<PathBuf, ConfigError> {
    if let Some(override_dir) = std::env::var_os("PARANOIA_ENC_CONFIG_DIR") {
        if !override_dir.is_empty() {
            return Ok(PathBuf::from(override_dir));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or(ConfigError::NoConfigDir)?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(CONFIG_DIR_NAME))
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("LOCALAPPDATA"))
            .ok_or(ConfigError::NoConfigDir)?;
        Ok(PathBuf::from(base).join(CONFIG_DIR_NAME))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join(CONFIG_DIR_NAME));
            }
        }
        let home = std::env::var_os("HOME").ok_or(ConfigError::NoConfigDir)?;
        Ok(PathBuf::from(home).join(".config").join(CONFIG_DIR_NAME))
    }
}

pub fn config_file_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}

fn load_from(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read(path) {
        Ok(bytes) => {
            let parsed: ConfigFile = serde_json::from_slice(&bytes)?;
            Ok(parsed.remembered_password.filter(|value| !value.is_empty()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ConfigError::Io(error)),
    }
}

fn save_to(path: &Path, password: &str) -> Result<(), ConfigError> {
    let parent = path.parent().ok_or(ConfigError::NoConfigDir)?;
    fs::create_dir_all(parent)?;
    let payload = ConfigFile {
        remembered_password: Some(password.to_string()),
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

fn clear_at(path: &Path) -> Result<(), ConfigError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ConfigError::Io(error)),
    }
}

/// Load the remembered password, if any.
pub fn load_remembered_password() -> Result<Option<String>, ConfigError> {
    load_from(&config_file_path()?)
}

/// Persist the password in plain text for the next launch.
pub fn save_remembered_password(password: &str) -> Result<(), ConfigError> {
    save_to(&config_file_path()?, password)
}

/// Remove the remembered password (idempotent).
pub fn clear_remembered_password() -> Result<(), ConfigError> {
    clear_at(&config_file_path()?)
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
            "paranoia-enc-test-config-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_and_clear() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("config.json");
        assert_eq!(load_from(&path).unwrap(), None);
        save_to(&path, "123").unwrap();
        assert_eq!(load_from(&path).unwrap(), Some("123".to_string()));
        // Plaintext must actually be stored (documented risk).
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("123"));
        assert!(raw.contains("remembered_password"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        clear_at(&path).unwrap();
        assert_eq!(load_from(&path).unwrap(), None);
        clear_at(&path).unwrap(); // idempotent
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_or_missing_file_yields_none() {
        let dir = temp_dir("empty");
        let path = dir.join("config.json");
        fs::write(&path, br#"{}"#).unwrap();
        assert_eq!(load_from(&path).unwrap(), None);
        fs::write(&path, br#"{"remembered_password": ""}"#).unwrap();
        assert_eq!(load_from(&path).unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn config_file_path_lives_in_config_dir() {
        let path = config_file_path().unwrap();
        assert!(path.ends_with("config.json"));
        assert!(path.parent().is_some());
    }
}
