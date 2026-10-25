//! Smart output layout, conflict naming and conflict-aware moves.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, UpError};
use crate::types::{ConflictPolicy, LayoutPolicy};
use crate::util::{count_tree, move_path, remove_path, sanitize_component};

pub fn is_junk_name(name: &str) -> bool {
    if name.is_empty() {
        return true;
    }
    if name.starts_with("._") {
        return true;
    }
    let low = name.to_ascii_lowercase();
    matches!(
        low.as_str(),
        "__macosx"
            | ".ds_store"
            | ".spotlight-v100"
            | ".trashes"
            | ".fseventsd"
            | "thumbs.db"
            | "desktop.ini"
    )
}

/// Sanitised archive base name: "示例 A.mp4" -> "示例 A".
///
/// Split families collapse to the family stem so every volume maps to one output
/// folder: "3138.7z.001" -> "3138", "x.part1.rar" -> "x", and SSEFE-encrypted
/// volumes ("3138.7z.001.enc") likewise.
pub fn archive_stem(archive: &Path) -> String {
    if let Some(volume) = archive
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(crate::split::parse_volume_name)
    {
        return sanitize_component(&volume.stem());
    }
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_string());
    sanitize_component(&stem)
}

#[derive(Debug, Clone)]
pub struct Plan {
    /// Folder that already exists (parent of the archive, or the output root).
    pub parent: PathBuf,
    /// Folder the extracted top-level items land in.
    pub dest_root: PathBuf,
    /// True when we created a folder named after the archive.
    pub foldered: bool,
}

/// Decide where extracted top-level items go.
///
/// * `Smart`: a single top-level item goes straight into the target folder,
///   two or more go into `<target>/<archive stem>/`.
pub fn plan_dest(
    layout: LayoutPolicy,
    archive: &Path,
    out_root: Option<&Path>,
    top_count: usize,
) -> Plan {
    let parent = out_root
        .map(Path::to_path_buf)
        .or_else(|| archive.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let foldered = match layout {
        LayoutPolicy::Direct => false,
        LayoutPolicy::Folder => true,
        LayoutPolicy::Smart => top_count > 1,
    };
    let dest_root = if foldered {
        parent.join(archive_stem(archive))
    } else {
        parent.clone()
    };
    Plan {
        parent,
        dest_root,
        foldered,
    }
}

/// Free name next to `target`: keeps the old desktop convention (`x.txt` -> `x (1).txt`),
/// directories get the counter appended to the whole name
/// (`pkg.v1.5` -> `pkg.v1.5 (1)`) — never split on the dots inside a folder name.
pub fn unique_sibling(target: &Path, source_is_dir: bool) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let parent = target.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let full_name = target
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string());

    if source_is_dir || target.is_dir() {
        for i in 1..10_000 {
            let cand = parent.join(format!("{full_name} ({i})"));
            if !cand.exists() {
                return cand;
            }
        }
        return parent.join(format!("{full_name} (copy)"));
    }

    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string());
    let ext = target
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for i in 1..10_000 {
        let name = if ext.is_empty() {
            format!("{stem} ({i})")
        } else {
            format!("{stem} ({i}).{ext}")
        };
        let cand = parent.join(name);
        if !cand.exists() {
            return cand;
        }
    }
    parent.join(format!("{stem}_copy"))
}

#[derive(Default, Debug, Clone)]
pub struct MoveStats {
    pub files: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub renamed: u64,
}

impl MoveStats {
    fn merge(&mut self, other: MoveStats) {
        self.files += other.files;
        self.bytes += other.bytes;
        self.skipped += other.skipped;
        self.renamed += other.renamed;
    }

    pub fn merge_public(&mut self, other: MoveStats) {
        self.merge(other);
    }
}

/// Recursively delete junk entries (`__MACOSX`, `.DS_Store`, `._*`, `Thumbs.db`).
pub fn strip_junk_tree(root: &Path) -> Result<()> {
    let rd = match fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if is_junk_name(&name) {
            let _ = remove_path(&p);
            continue;
        }
        if p.is_dir() {
            strip_junk_tree(&p)?;
        }
    }
    Ok(())
}

/// Move `src` onto `dst` honouring the conflict policy.
///
/// Files: rename / skip / overwrite per file. Directories: merged entry by
/// entry so an existing folder is never wiped wholesale — except a *top-level*
/// directory under `Rename`, which is moved aside as a whole (`folder_1`) so a
/// re-extraction never interleaves `_1` files into the existing folder.
pub fn move_with_policy(
    src: &Path,
    dst: &Path,
    policy: ConflictPolicy,
    top: bool,
) -> Result<MoveStats> {
    Ok(move_with_policy_landing(src, dst, policy, top)?.0)
}

/// Like [`move_with_policy`], but also reports where the item actually landed
/// (a renamed `name_1` sibling, or the merged existing directory) so callers
/// can record the produced output names for later existence checks.
pub fn move_with_policy_landing(
    src: &Path,
    dst: &Path,
    policy: ConflictPolicy,
    top: bool,
) -> Result<(MoveStats, PathBuf)> {
    let meta = fs::symlink_metadata(src).map_err(|e| UpError::Io {
        path: src.to_path_buf(),
        source: e,
    })?;
    if meta.is_dir() {
        if top && policy == ConflictPolicy::Rename && dst.exists() {
            let nd = unique_sibling(dst, true);
            move_path(src, &nd)?;
            let (f, b) = count_tree(&nd);
            return Ok((
                MoveStats {
                    files: f,
                    bytes: b,
                    renamed: 1,
                    ..Default::default()
                },
                nd,
            ));
        }
        return move_dir(src, dst, policy);
    }

    if dst.exists() {
        match policy {
            ConflictPolicy::Overwrite => {
                remove_path(dst)?;
            }
            ConflictPolicy::Skip => {
                remove_path(src)?;
                return Ok((
                    MoveStats {
                        skipped: 1,
                        ..Default::default()
                    },
                    dst.to_path_buf(),
                ));
            }
            ConflictPolicy::Rename => {
                let nd = unique_sibling(dst, false);
                move_path(src, &nd)?;
                return Ok((
                    MoveStats {
                        files: 1,
                        bytes: meta.len(),
                        renamed: 1,
                        ..Default::default()
                    },
                    nd,
                ));
            }
        }
    }
    move_path(src, dst)?;
    Ok((
        MoveStats {
            files: 1,
            bytes: meta.len(),
            ..Default::default()
        },
        dst.to_path_buf(),
    ))
}

fn move_dir(src: &Path, dst: &Path, policy: ConflictPolicy) -> Result<(MoveStats, PathBuf)> {
    if dst.exists() {
        if dst.is_dir() {
            // Merge trees entry by entry.
            let mut stats = MoveStats::default();
            let rd = fs::read_dir(src).map_err(|e| UpError::Io {
                path: src.to_path_buf(),
                source: e,
            })?;
            for entry in rd.flatten() {
                let child = entry.path();
                let target = dst.join(entry.file_name());
                stats.merge(move_with_policy_landing(&child, &target, policy, false)?.0);
            }
            let _ = fs::remove_dir(src);
            return Ok((stats, dst.to_path_buf()));
        }
        match policy {
            ConflictPolicy::Overwrite => {
                remove_path(dst)?;
            }
            ConflictPolicy::Skip => {
                remove_path(src)?;
                return Ok((
                    MoveStats {
                        skipped: 1,
                        ..Default::default()
                    },
                    dst.to_path_buf(),
                ));
            }
            ConflictPolicy::Rename => {
                let nd = unique_sibling(dst, true);
                move_path(src, &nd)?;
                let (f, b) = count_tree(&nd);
                return Ok((
                    MoveStats {
                        files: f,
                        bytes: b,
                        renamed: 1,
                        ..Default::default()
                    },
                    nd,
                ));
            }
        }
    }
    move_path(src, dst)?;
    let (f, b) = count_tree(dst);
    Ok((
        MoveStats {
            files: f,
            bytes: b,
            ..Default::default()
        },
        dst.to_path_buf(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_is_sanitized() {
        assert_eq!(archive_stem(Path::new("/tmp/示例 A.mp4")), "示例 A");
        // ":"" and "?" are both replaced, so the stem keeps a trailing underscore.
        assert_eq!(archive_stem(Path::new("/tmp/a:b?.zip")), "a_b_");
        assert_eq!(archive_stem(Path::new("/tmp/x:*?.zip")), "x___");
    }

    #[test]
    fn archive_stem_strips_split_volume_suffix() {
        assert_eq!(archive_stem(Path::new("/tmp/3138.7z.001")), "3138");
        assert_eq!(archive_stem(Path::new("/tmp/3138.7z.001.enc")), "3138");
        assert_eq!(archive_stem(Path::new("/tmp/x.part1.rar")), "x");
        assert_eq!(archive_stem(Path::new("/tmp/x.part12.rar")), "x");
        // Non-volume names keep the old behaviour (SSEFE containers included).
        assert_eq!(archive_stem(Path::new("/tmp/sample.enc")), "sample");
        assert_eq!(archive_stem(Path::new("/tmp/a.zip")), "a");
    }

    #[test]
    fn smart_layout_branches() {
        let a = Path::new("/tmp/x/示例 A.mp4");
        let p1 = plan_dest(LayoutPolicy::Smart, a, None, 1);
        assert_eq!(p1.dest_root, PathBuf::from("/tmp/x"));
        assert!(!p1.foldered);
        let p3 = plan_dest(LayoutPolicy::Smart, a, None, 3);
        assert_eq!(p3.dest_root, PathBuf::from("/tmp/x/示例 A"));
        assert!(p3.foldered);
        let pf = plan_dest(LayoutPolicy::Folder, a, Some(Path::new("/out")), 1);
        assert_eq!(pf.dest_root, PathBuf::from("/out/示例 A"));
        assert_eq!(pf.parent, PathBuf::from("/out"));
    }

    #[test]
    fn unique_sibling_counts_up() {
        let dir = std::env::temp_dir().join(format!("paranoia-layout-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(unique_sibling(&f, false), dir.join("a (1).txt"));
        std::fs::write(dir.join("a (1).txt"), b"x").unwrap();
        assert_eq!(unique_sibling(&f, false), dir.join("a (2).txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Folder names routinely contain dots (`mod v1.5`); the counter must be
    /// appended to the whole name, not inserted before "5".
    #[test]
    fn unique_sibling_for_dotted_directory_names() {
        let dir =
            std::env::temp_dir().join(format!("paranoia-layout-dir-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.join("别礼-仙女v1.5（内附切换）by hajimicat");
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(
            unique_sibling(&d, true),
            dir.join("别礼-仙女v1.5（内附切换）by hajimicat (1)")
        );

        let depot = dir.join("mod.v2.depot");
        std::fs::create_dir_all(&depot).unwrap();
        assert_eq!(unique_sibling(&depot, true), dir.join("mod.v2.depot (1)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conflict_policies_on_files() {
        let dir = std::env::temp_dir().join(format!("paranoia-move-{}", uuid::Uuid::new_v4()));
        let staging = dir.join("staging");
        let dest = dir.join("dest");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&dest).unwrap();

        std::fs::write(dest.join("a.txt"), b"OLD").unwrap();
        std::fs::write(staging.join("a.txt"), b"NEW").unwrap();
        let st = move_with_policy(
            &staging.join("a.txt"),
            &dest.join("a.txt"),
            ConflictPolicy::Skip,
            true,
        )
        .unwrap();
        assert_eq!(st.skipped, 1);
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"OLD");

        std::fs::write(staging.join("a.txt"), b"NEW").unwrap();
        let st = move_with_policy(
            &staging.join("a.txt"),
            &dest.join("a.txt"),
            ConflictPolicy::Rename,
            true,
        )
        .unwrap();
        assert_eq!(st.renamed, 1);
        assert!(dest.join("a (1).txt").exists());

        std::fs::write(staging.join("a.txt"), b"NEW").unwrap();
        move_with_policy(
            &staging.join("a.txt"),
            &dest.join("a.txt"),
            ConflictPolicy::Overwrite,
            true,
        )
        .unwrap();
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"NEW");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn top_level_dir_collision_renames_whole_folder() {
        let dir = std::env::temp_dir().join(format!("paranoia-movedir-{}", uuid::Uuid::new_v4()));
        let staging = dir.join("staging").join("pkg");
        let dest = dir.join("dest").join("pkg");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(staging.join("new.txt"), b"NEW").unwrap();
        std::fs::create_dir(staging.join("sub")).unwrap();
        std::fs::write(staging.join("sub").join("inner.txt"), b"X").unwrap();
        std::fs::write(dest.join("old.txt"), b"OLD").unwrap();

        let st = move_with_policy(&staging, &dest, ConflictPolicy::Rename, true).unwrap();
        assert_eq!(st.renamed, 1);
        assert_eq!(st.files, 2, "moved files = {}", st.files);
        // Existing folder untouched, new copy beside it.
        assert_eq!(std::fs::read(dest.join("old.txt")).unwrap(), b"OLD");
        assert!(!dest.join("new.txt").exists(), "must not interleave");
        let renamed = dir.join("dest").join("pkg (1)");
        assert!(renamed.join("new.txt").exists());
        assert!(renamed.join("sub").join("inner.txt").exists());

        // Nested collisions still merge (that is the 7-Zip-like behaviour).
        let nested = dir.join("nested");
        std::fs::create_dir_all(nested.join("pkg").join("sub")).unwrap();
        std::fs::write(nested.join("pkg").join("sub").join("inner.txt"), b"Y").unwrap();
        let st2 =
            move_with_policy(&nested.join("pkg"), &renamed, ConflictPolicy::Rename, false).unwrap();
        assert_eq!(st2.renamed, 1, "inner file renamed on merge");
        assert!(renamed.join("sub").join("inner (1).txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
