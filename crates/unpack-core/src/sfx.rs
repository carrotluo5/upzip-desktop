//! Opt-in Windows downgrade path: run a self-extracting executable.
//!
//! The engine always tries to read SFX payloads as *data* first (that covers
//! Bandizip / WinRAR / 7-Zip / WinZip style SFX and requires no execution).
//! This module exists for the leftovers — installers and obfuscated stubs whose
//! payload 7-Zip cannot decode. It is deliberately fenced off:
//!
//! * off by default (`RunConfig::allow_execute_sfx`),
//! * Windows only (a PE cannot be executed on macOS),
//! * the child starts with its working directory set to a fresh `.unparanoia-tmp-*`
//!   folder, so whatever it drops lands there,
//! * stdin is closed and stdout/stderr are discarded (no dialog can block
//!   forever): a stall watchdog plus a hard timeout kill the process instead.
//!
//! Silent-switch support differs between SFX generators, and these variants
//! have NOT yet been verified on a real Windows machine. If a generator needs
//! different flags, extend `variants()` — that is the only place to touch.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::cancel::CancelToken;
use crate::error::{Result, UpError};

/// Hard cap on one execution attempt.
const SFX_TIMEOUT: Duration = Duration::from_secs(600);
/// If nothing appeared in the destination after this long, assume the program
/// is sitting on an interactive prompt and kill it.
const NO_OUTPUT_GRACE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_secs(15);
/// Passwords tried per run (each one costs a full attempt).
const MAX_PASSWORDS: usize = 2;

#[derive(Debug, Clone)]
pub struct Variant {
    pub label: &'static str,
    pub args: Vec<String>,
}

/// Candidate command lines, tried in order. `dest` is the staging folder.
pub fn variants(dest: &Path, password: Option<&str>) -> Vec<Variant> {
    let dest = dest.to_string_lossy().to_string();
    let pw_args: Vec<String> = match password {
        Some(p) if !p.is_empty() => vec![format!("-p{p}")],
        _ => Vec::new(),
    };

    // 7-Zip SFX modules understand the extractor switches (`-o<dir>`, `-y`).
    let mut seven = vec!["-y".to_string(), format!("-o{dest}")];
    seven.extend(pw_args.clone());

    // WinRAR-style SFX: silent mode, overwrite; destination = working directory.
    let mut winrar = vec!["-s2".to_string(), "-y".to_string(), "-o+".to_string()];
    winrar.extend(pw_args);

    vec![
        Variant {
            label: "7z-sfx",
            args: seven,
        },
        Variant {
            label: "winrar-sfx",
            args: winrar,
        },
    ]
}

/// Run the executable until one variant drops files into `dest`.
/// Returns the number of top-level items produced.
pub async fn run_sfx(
    exe: &Path,
    passwords: &[String],
    dest: &Path,
    cancel: &CancelToken,
) -> Result<usize> {
    let list: Vec<&str> = passwords
        .iter()
        .take(MAX_PASSWORDS)
        .map(|p| p.as_str())
        .filter(|p| !p.is_empty())
        .collect();
    let attempts: Vec<Option<&str>> = if list.is_empty() {
        vec![None]
    } else {
        list.into_iter().map(Some).collect()
    };

    let mut failures: Vec<String> = Vec::new();
    for pw in attempts {
        for variant in variants(dest, pw) {
            if cancel.is_cancelled() {
                return Err(UpError::Cancelled);
            }
            match run_one(exe, &variant, dest, cancel).await {
                Ok(()) => {
                    let n = produced_items(dest);
                    if n > 0 {
                        return Ok(n);
                    }
                    failures.push(format!("{}：未产生文件", variant.label));
                }
                Err(UpError::Cancelled) => return Err(UpError::Cancelled),
                Err(e) => failures.push(format!("{}：{e}", variant.label)),
            }
        }
    }
    Err(UpError::Other(if failures.is_empty() {
        "没有可用的静默参数组合".to_string()
    } else {
        failures.join("；")
    }))
}

enum Waited {
    Done,
    Cancelled,
    TimedOut,
    Stalled,
    Failed(String),
}

async fn run_one(exe: &Path, variant: &Variant, dest: &Path, cancel: &CancelToken) -> Result<()> {
    let mut child = Command::new(exe)
        .args(&variant.args)
        .current_dir(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| UpError::Other(format!("无法启动 {}：{e}", exe.display())))?;

    let outcome = tokio::select! {
        _ = cancel.wait_cancelled() => Waited::Cancelled,
        r = tokio::time::timeout(SFX_TIMEOUT, child.wait()) => match r {
            Ok(Ok(_status)) => Waited::Done,
            Ok(Err(e)) => Waited::Failed(e.to_string()),
            Err(_) => Waited::TimedOut,
        },
        _ = stalled(dest) => Waited::Stalled,
    };

    match outcome {
        Waited::Done => Ok(()),
        Waited::Cancelled => {
            let _ = child.kill().await;
            Err(UpError::Cancelled)
        }
        Waited::TimedOut => {
            let _ = child.kill().await;
            Err(UpError::Other("执行超时".into()))
        }
        Waited::Stalled => {
            let _ = child.kill().await;
            if produced_items(dest) > 0 {
                // Files landed; the program is only parked on a dialog now.
                Ok(())
            } else {
                Err(UpError::Other(
                    "长时间没有输出，可能仍在等待交互输入；已终止".into(),
                ))
            }
        }
        Waited::Failed(e) => Err(UpError::Other(e)),
    }
}

/// Resolves once the destination has stopped changing for the whole grace
/// window. Progress is measured as (files, bytes), so a big file that is still
/// being written keeps the watchdog quiet, while a program parked on an
/// interactive prompt (which we cannot answer) gets killed by the caller.
async fn stalled(dest: &Path) {
    let mut last = snapshot(dest);
    let mut idle = Duration::ZERO;
    loop {
        tokio::time::sleep(POLL).await;
        let now = snapshot(dest);
        if now != last {
            last = now;
            idle = Duration::ZERO;
            continue;
        }
        idle += POLL;
        if idle >= NO_OUTPUT_GRACE {
            return;
        }
    }
}

/// Cheap progress fingerprint: (files, bytes) under the destination.
fn snapshot(dest: &Path) -> (u64, u64) {
    crate::util::count_tree(dest)
}

/// Number of usable top-level entries the executable produced.
pub fn produced_items(dest: &Path) -> usize {
    std::fs::read_dir(dest)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    !crate::layout::is_junk_name(&name)
                })
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_flag_only_when_present() {
        let v = variants(Path::new("/tmp/x"), Some("pw"));
        assert!(v.iter().all(|x| x.args.iter().any(|a| a == "-ppw")));
        assert!(v[0].args.contains(&"-o/tmp/x".to_string()));
        let v2 = variants(Path::new("/tmp/x"), None);
        assert!(v2
            .iter()
            .all(|x| !x.args.iter().any(|a| a.starts_with("-p"))));
    }

    #[test]
    fn produced_items_counts_top_level() {
        let dir = std::env::temp_dir().join(format!("paranoia-sfx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(produced_items(&dir), 0);
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join(".DS_Store"), b"x").unwrap();
        assert_eq!(produced_items(&dir), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_tracks_file_growth() {
        let dir = std::env::temp_dir().join(format!("paranoia-sfx-snap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let before = snapshot(&dir);
        std::fs::write(dir.join("x.bin"), vec![0u8; 4096]).unwrap();
        assert_ne!(before, snapshot(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
