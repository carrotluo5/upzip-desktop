//! Split / multi-volume archive families.
//!
//! 7-Zip writes <stem>.7z.001, <stem>.zip.001, ...; WinRAR writes
//! <stem>.part1.rar, ... . Only the first volume is a valid 7-Zip entry point:
//! 7-Zip locates the sibling volumes by name in the same directory.
//!
//! This module parses those names, discovers the whole family on disk (including
//! SSEFE-encrypted volumes such as <stem>.7z.001.enc) and groups batch inputs
//! so one logical archive becomes one task.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Archive formats that can be split into numbered volumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SplitKind {
    SevenZip,
    Zip,
    Rar,
}

impl SplitKind {
    /// Archive extension (7z, zip, rar).
    pub fn ext(self) -> &'static str {
        match self {
            SplitKind::SevenZip => "7z",
            SplitKind::Zip => "zip",
            SplitKind::Rar => "rar",
        }
    }

    /// Detection.container value for the first volume of this family.
    pub fn container(self) -> &'static str {
        match self {
            SplitKind::SevenZip => "7z-volume",
            SplitKind::Zip => "zip-volume",
            SplitKind::Rar => "rar-volume",
        }
    }

    fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "7z" => Some(SplitKind::SevenZip),
            "zip" => Some(SplitKind::Zip),
            "rar" => Some(SplitKind::Rar),
            _ => None,
        }
    }
}

/// A parsed split-volume file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeName {
    /// Family key: <stem>.<ext> (e.g. 3138.7z, x.rar).
    pub base: String,
    pub kind: SplitKind,
    /// 1-based volume number.
    pub index: u32,
    /// True when the volume itself is a Paranoia v4 container (....001.enc).
    pub encrypted: bool,
    /// File name exactly as it appears on disk.
    pub name: String,
}

impl VolumeName {
    /// Archive stem of the family (3138.7z -> 3138, x.rar -> x).
    pub fn stem(&self) -> String {
        self.base
            .rsplit_once('.')
            .map(|(stem, _)| stem.to_string())
            .unwrap_or_else(|| self.base.clone())
    }

    /// Name of the decrypted plain volume (x.7z.001.enc -> x.7z.001).
    pub fn plain_name(&self) -> String {
        if self.encrypted {
            self.name
                .strip_suffix(".enc")
                .map(str::to_string)
                .unwrap_or_else(|| self.name.clone())
        } else {
            self.name.clone()
        }
    }
}

/// Parse a file name that belongs to a split family.
///
/// Accepted shapes:
/// * name.7z.001 / name.zip.012 (7-Zip style, any digit width),
/// * name.part1.rar / name.part01.rar (WinRAR style),
/// * either of the above with a trailing .enc (SSEFE-encrypted volume).
///
/// A plain name.7z / name.zip / name.rar is *not* a split volume.
pub fn parse_volume_name(name: &str) -> Option<VolumeName> {
    if name.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let (inner, encrypted) = match lower.strip_suffix(".enc") {
        Some(stem) => (&name[..stem.len()], true),
        None => (name, false),
    };
    let inner_lower = inner.to_ascii_lowercase();

    // WinRAR style: <stem>.part<N>.rar
    if let Some(prefix) = inner_lower.strip_suffix(".rar") {
        let (stem, part) = prefix.rsplit_once(".part")?;
        let digits = part.trim_start_matches('0');
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) || digits.is_empty() {
            return None;
        }
        let index: u32 = part.parse().ok()?;
        if index == 0 || stem.is_empty() {
            return None;
        }
        let base = format!("{}.rar", &inner[..stem.len()]);
        return Some(VolumeName {
            base,
            kind: SplitKind::Rar,
            index,
            encrypted,
            name: name.to_string(),
        });
    }

    // 7-Zip style: <stem>.<ext>.<digits>
    let (prefix, number) = inner_lower.rsplit_once('.')?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) || number.len() > 5 {
        return None;
    }
    let index: u32 = number.parse().ok()?;
    if index == 0 {
        return None;
    }
    let ext = prefix.rsplit_once('.').map(|(_, ext)| ext)?;
    let kind = SplitKind::from_ext(ext)?;
    let stem = &inner[..prefix.len() - ext.len() - 1];
    if stem.is_empty() {
        return None;
    }
    Some(VolumeName {
        base: inner[..prefix.len()].to_string(),
        kind,
        index,
        encrypted,
        name: name.to_string(),
    })
}

/// One existing volume on disk.
#[derive(Clone, Debug)]
pub struct VolumeEntry {
    pub index: u32,
    pub path: PathBuf,
    pub size: u64,
}

/// A complete family discovered in one directory.
#[derive(Clone, Debug)]
pub struct SplitFamily {
    pub kind: SplitKind,
    /// Family key (3138.7z, x.rar).
    pub base: String,
    /// True when every volume is an SSEFE container.
    pub encrypted: bool,
    /// Volumes sorted by index (may start above 1 when the first one is missing).
    pub volumes: Vec<VolumeEntry>,
    pub total_bytes: u64,
    pub dir: PathBuf,
}

impl SplitFamily {
    pub fn first(&self) -> Option<&VolumeEntry> {
        self.volumes.iter().find(|v| v.index == 1)
    }

    pub fn missing_first(&self) -> bool {
        self.first().is_none()
    }

    pub fn volume_count(&self) -> usize {
        self.volumes.len()
    }

    /// Short human readable summary used in detection notes and errors.
    pub fn describe(&self) -> String {
        format!(
            "分卷压缩包（{} 卷，合计 {}）",
            self.volumes.len(),
            crate::util::human_bytes(self.total_bytes)
        )
    }

    /// Expected first-volume name, used when the user dragged a later volume only.
    pub fn expected_first_name(&self) -> String {
        match self.kind {
            SplitKind::Rar => format!("{}.part1.rar", self.stem()),
            SplitKind::SevenZip | SplitKind::Zip => {
                let suffix = if self.encrypted { ".001.enc" } else { ".001" };
                format!("{}{suffix}", self.base)
            }
        }
    }

    /// Archive stem without volume number / extension (3138.7z -> 3138).
    pub fn stem(&self) -> String {
        self.base
            .rsplit_once('.')
            .map(|(stem, _)| stem.to_string())
            .unwrap_or_else(|| self.base.clone())
    }
}

/// Discover the whole family that path belongs to by scanning its directory.
///
/// Only volumes with the same family key and the same encrypted flag are
/// collected: plain siblings next to .enc volumes (a previous manual decryption)
/// must not be mixed into an encrypted family.
pub fn discover(path: &Path) -> Option<SplitFamily> {
    let name = path.file_name()?.to_str()?;
    let probe = parse_volume_name(name)?;
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut volumes: Vec<VolumeEntry> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(candidate) = parse_volume_name(file_name) else {
                continue;
            };
            if candidate.base != probe.base
                || candidate.kind != probe.kind
                || candidate.encrypted != probe.encrypted
            {
                continue;
            }
            if !seen.insert(candidate.index) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            volumes.push(VolumeEntry {
                index: candidate.index,
                path: entry.path(),
                size,
            });
        }
    }
    if volumes.is_empty() {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        volumes.push(VolumeEntry {
            index: probe.index,
            path: path.to_path_buf(),
            size,
        });
    }
    volumes.sort_by_key(|v| v.index);
    let total_bytes = volumes.iter().map(|v| v.size).sum();
    Some(SplitFamily {
        kind: probe.kind,
        base: probe.base,
        encrypted: probe.encrypted,
        volumes,
        total_bytes,
        dir,
    })
}

/// Build a family from a list of entries that all belong to one split family.
///
/// Used by the nested cascade: an archive may contain <stem>.7z.001 +
/// <stem>.7z.002 as separate files. Returns None when the entries are not all
/// volumes of the same family or when the first volume is missing.
pub fn family_from_entries(entries: &[PathBuf]) -> Option<SplitFamily> {
    let mut kind: Option<SplitKind> = None;
    let mut base = String::new();
    let mut encrypted = false;
    let mut volumes: Vec<VolumeEntry> = Vec::new();
    let mut dir: Option<PathBuf> = None;
    for path in entries {
        let name = path.file_name()?.to_str()?;
        let parsed = parse_volume_name(name)?;
        match kind {
            None => {
                kind = Some(parsed.kind);
                base = parsed.base.clone();
                encrypted = parsed.encrypted;
            }
            Some(k) => {
                if parsed.kind != k || parsed.base != base || parsed.encrypted != encrypted {
                    return None;
                }
            }
        }
        if dir.is_none() {
            dir = path.parent().map(Path::to_path_buf);
        }
        volumes.push(VolumeEntry {
            index: parsed.index,
            path: path.clone(),
            size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        });
    }
    volumes.sort_by_key(|v| v.index);
    if volumes.is_empty() || volumes[0].index != 1 {
        return None;
    }
    let total_bytes = volumes.iter().map(|v| v.size).sum();
    Some(SplitFamily {
        kind: kind?,
        base,
        encrypted,
        volumes,
        total_bytes,
        dir: dir.unwrap_or_else(|| PathBuf::from(".")),
    })
}

/// Group batch inputs: all volumes of one family collapse into a single task whose
/// path is the first volume (or, when the first volume is not part of the batch,
/// the lowest-numbered one so the engine can report the actionable skip reason).
pub fn group_inputs(files: &[PathBuf]) -> Vec<PathBuf> {
    struct FamilyInfo {
        first: Option<PathBuf>,
        lowest: (u32, PathBuf),
    }

    let parsed: Vec<Option<(String, u32)>> = files.iter().map(|file| family_key(file)).collect();
    let mut families: HashMap<String, FamilyInfo> = HashMap::new();
    for (file, parsed) in files.iter().zip(&parsed) {
        let Some((key, index)) = parsed else { continue };
        let info = families.entry(key.clone()).or_insert_with(|| FamilyInfo {
            first: None,
            lowest: (*index, file.clone()),
        });
        if *index == 1 {
            info.first = Some(file.clone());
        }
        if *index < info.lowest.0 {
            info.lowest = (*index, file.clone());
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();
    let mut emitted: HashSet<String> = HashSet::new();
    for (file, parsed) in files.iter().zip(&parsed) {
        match parsed {
            None => out.push(file.clone()),
            Some((key, _)) => {
                if emitted.insert(key.clone()) {
                    let representative = families
                        .get(key)
                        .map(|info| {
                            info.first
                                .clone()
                                .unwrap_or_else(|| info.lowest.1.clone())
                        })
                        .unwrap_or_else(|| file.clone());
                    out.push(representative);
                }
            }
        }
    }
    out
}

fn family_key(file: &Path) -> Option<(String, u32)> {
    let name = file.file_name()?.to_str()?;
    let parsed = parse_volume_name(name)?;
    Some((
        format!("{}|{:?}|{}", parsed.base, parsed.kind, parsed.encrypted),
        parsed.index,
    ))
}

/// Expected number of volumes of a split 7z archive, derived from its start header.
///
/// A 7z stream's logical length is 32 + NextHeaderOffset + NextHeaderSize; the
/// first volume is a fixed-size chunk of that stream, so the volume count is
/// ceil(logical / first_volume_size). This lets us tell "missing volume" apart
/// from "corrupt archive" without trusting 7-Zip's error text.
pub fn expected_7z_volumes(first: &Path) -> Option<u32> {
    use std::io::Read;
    const SIGNATURE: &[u8; 6] = b"\x37\x7a\xbc\xaf\x27\x1c";
    let first_size = fs::metadata(first).ok()?.len();
    if first_size == 0 {
        return None;
    }
    let mut head = [0u8; 32];
    let mut file = fs::File::open(first).ok()?;
    file.read_exact(&mut head).ok()?;
    if &head[..6] != SIGNATURE {
        return None;
    }
    let offset = u64::from_le_bytes(head[12..20].try_into().ok()?);
    let size = u64::from_le_bytes(head[20..28].try_into().ok()?);
    let logical = 32u64.checked_add(offset)?.checked_add(size)?;
    if logical == 0 {
        return None;
    }
    let expected = (logical + first_size - 1) / first_size;
    u32::try_from(expected).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "unparanoia-split-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_sevenzip_zip_and_rar_volume_names() {
        let v = parse_volume_name("3138.7z.001").unwrap();
        assert_eq!(v.base, "3138.7z");
        assert_eq!(v.kind, SplitKind::SevenZip);
        assert_eq!(v.index, 1);
        assert!(!v.encrypted);
        assert_eq!(v.plain_name(), "3138.7z.001");

        let v = parse_volume_name("a.zip.012").unwrap();
        assert_eq!(v.base, "a.zip");
        assert_eq!(v.kind, SplitKind::Zip);
        assert_eq!(v.index, 12);

        let v = parse_volume_name("x.part1.rar").unwrap();
        assert_eq!(v.base, "x.rar");
        assert_eq!(v.kind, SplitKind::Rar);
        assert_eq!(v.index, 1);
        assert_eq!(v.plain_name(), "x.part1.rar");

        let v = parse_volume_name("x.part007.rar").unwrap();
        assert_eq!(v.index, 7);

        let v = parse_volume_name("3138.7z.002.enc").unwrap();
        assert_eq!(v.base, "3138.7z");
        assert_eq!(v.index, 2);
        assert!(v.encrypted);
        assert_eq!(v.plain_name(), "3138.7z.002");

        // Extension case is preserved in the base but matched case-insensitively.
        let v = parse_volume_name("Demo.7Z.003").unwrap();
        assert_eq!(v.base, "Demo.7Z");
        assert_eq!(v.index, 3);
    }

    #[test]
    fn rejects_plain_and_malformed_names() {
        for name in [
            "a.7z",
            "a.zip",
            "a.rar",
            "a.7z.000",
            "a.7z.abc",
            "a.7z.001.tmp",
            "a.part1.zip",
            "random.txt",
            "",
            ".7z.001",
        ] {
            assert!(
                parse_volume_name(name).is_none(),
                "{name:?} must not parse as a split volume"
            );
        }
    }

    #[test]
    fn discover_collects_siblings_and_ignores_other_families() {
        let dir = temp_dir("discover");
        for name in [
            "3138.7z.001",
            "3138.7z.002",
            "3138.7z.003",
            "3138.7z.004.enc",
            "other.7z.001",
            "3138.7z.001.bak",
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let family = discover(&dir.join("3138.7z.001")).unwrap();
        assert_eq!(family.kind, SplitKind::SevenZip);
        assert_eq!(family.base, "3138.7z");
        assert_eq!(family.volume_count(), 3);
        assert_eq!(family.first().unwrap().index, 1);
        assert!(!family.encrypted);
        assert_eq!(family.stem(), "3138");
        assert!(family.describe().contains("3 卷"));

        let encrypted = discover(&dir.join("3138.7z.004.enc")).unwrap();
        assert_eq!(encrypted.volume_count(), 1);
        assert!(encrypted.encrypted);
        assert!(encrypted.missing_first());
        assert_eq!(encrypted.expected_first_name(), "3138.7z.001.enc");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn group_inputs_merges_families_and_keeps_first_volume_order() {
        let files = vec![
            PathBuf::from("/in/b.7z.002"),
            PathBuf::from("/in/a.7z.001"),
            PathBuf::from("/in/a.7z.002"),
            PathBuf::from("/in/plain.zip"),
            PathBuf::from("/in/b.7z.001"),
            PathBuf::from("/in/a.7z.001"),
        ];
        let grouped = group_inputs(&files);
        assert_eq!(
            grouped,
            vec![
                PathBuf::from("/in/b.7z.001"),
                PathBuf::from("/in/a.7z.001"),
                PathBuf::from("/in/plain.zip"),
            ]
        );
    }

    #[test]
    fn group_inputs_keeps_later_volume_representative_when_first_missing() {
        let files = vec![
            PathBuf::from("/in/x.7z.003"),
            PathBuf::from("/in/x.7z.002"),
        ];
        let grouped = group_inputs(&files);
        assert_eq!(grouped, vec![PathBuf::from("/in/x.7z.002")]);
    }

    #[test]
    fn family_from_entries_requires_same_family_and_first_volume() {
        let dir = temp_dir("entries");
        for name in ["p.7z.001", "p.7z.002"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let entries = vec![dir.join("p.7z.002"), dir.join("p.7z.001")];
        let family = family_from_entries(&entries).unwrap();
        assert_eq!(family.volume_count(), 2);
        assert_eq!(family.first().unwrap().index, 1);

        assert!(family_from_entries(&[dir.join("p.7z.002")]).is_none());
        assert!(family_from_entries(&[dir.join("p.7z.001"), dir.join("q.7z.002")]).is_none());
        assert!(family_from_entries(&[dir.join("p.7z.001"), dir.join("notes.txt")]).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn expected_7z_volumes_uses_start_header() {
        let dir = temp_dir("expected");
        let first = dir.join("a.7z.001");
        // 100-byte first volume; logical stream = 32 + 100 + 50 = 182 -> 2 volumes.
        let mut head = Vec::new();
        head.extend_from_slice(b"\x37\x7a\xbc\xaf\x27\x1c");
        head.extend_from_slice(&[0u8; 6]); // version + crc
        head.extend_from_slice(&100u64.to_le_bytes()); // next header offset
        head.extend_from_slice(&50u64.to_le_bytes()); // next header size
        head.extend_from_slice(&[0u8; 4]); // crc
        head.resize(100, 0);
        fs::write(&first, &head).unwrap();
        assert_eq!(expected_7z_volumes(&first), Some(2));

        // Not a 7z stream.
        fs::write(&first, b"not-a-7z").unwrap();
        assert_eq!(expected_7z_volumes(&first), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
