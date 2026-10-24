//! Detection: real format sniffing (not extension based), masked-archive and
//! SFX recognition, plus the "carve the payload out" safety net.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use regex::Regex;
use uuid::Uuid;

use crate::cancel::CancelToken;
use crate::error::{Result, UpError};
use crate::sevenzip::{ListResult, SevenZip};
use crate::types::{Detection, FileKind, PasswordState};
use crate::util::{fingerprint, read_at, remove_path};

pub const HEAD_SCAN: usize = 512;
pub const TAIL_SCAN: u64 = 256 * 1024;

/// Magic number table (extension independent).
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    const M: &[(&str, &[u8])] = &[
        ("zip", b"PK\x03\x04"),
        ("zip", b"PK\x05\x06"),
        ("zip", b"PK\x07\x08"),
        ("rar", b"Rar!\x1a\x07\x00"),
        ("rar5", b"Rar!\x1a\x07\x01\x00"),
        ("7z", b"7z\xbc\xaf\x27\x1c"),
        ("cab", b"MSCF"),
        ("gzip", b"\x1f\x8b\x08"),
        ("xz", b"\xfd7zXZ\x00"),
        ("bzip2", b"BZh9"),
        ("dmg", b"koly"),
    ];
    for &(name, pat) in M {
        if head.starts_with(pat) {
            return Some(name);
        }
    }
    if head.len() > 265 && &head[257..262] == &b"ustar"[..] {
        return Some("tar");
    }
    None
}

#[derive(Debug, Clone, Default)]
pub struct TailZip {
    pub eocd_pos: u64,
    pub cd_start: u64,
    pub entries: u64,
    pub comment: String,
    pub first_name: Vec<u8>,
    pub valid: bool,
    /// ZIP64 tail (ECD64 record + EOCD64 locator) detected.
    pub zip64: bool,
    /// Absolute start of the archive, computed & verified against the local
    /// file-header signature (`Some(0)` when the archive is the whole file).
    pub base: Option<u64>,
    /// Absolute end of the archive (EOCD + comment length); trims trailing junk.
    pub zip_end: u64,
}

/// Look for a ZIP end-of-central-directory at the tail of a file. This is how a
/// `.mp4`-disguised or appended ZIP is recognised without trusting the name.
///
/// Also understands the ZIP64 tail (ECD64 record + locator) and derives the
/// exact archive byte range. Needed because 7-Zip 25.01 refuses prefixed
/// archives that store standard (relative) offsets — carving the range into a
/// standalone file makes them readable again.
pub fn analyze_tail(path: &Path, size: u64) -> Result<Option<TailZip>> {
    if size < 22 {
        return Ok(None);
    }
    let take = TAIL_SCAN.min(size);
    let mut f = File::open(path).map_err(|e| UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut buf = vec![0u8; take as usize];
    let n = read_at(&mut f, size - take, &mut buf).map_err(|e| UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    buf.truncate(n);
    let Some(idx) = rfind(&buf, b"PK\x05\x06") else {
        return Ok(None);
    };
    if idx + 22 > buf.len() {
        return Ok(None);
    }
    let e = &buf[idx..];
    let mut entries = u16::from_le_bytes([e[10], e[11]]) as u64;
    let mut cd_size = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
    let mut cd_off = u32::from_le_bytes([e[16], e[17], e[18], e[19]]) as u64;
    let clen = u16::from_le_bytes([e[20], e[21]]) as usize;
    let eocd_pos = size - take + idx as u64;
    let mut comment = String::new();
    if idx + 22 + clen <= buf.len() {
        comment = String::from_utf8_lossy(&buf[idx + 22..idx + 22 + clen]).to_string();
    }
    let zip_end = (eocd_pos + 22 + clen as u64).min(size);

    // ZIP64: the locator sits directly before the EOCD, the ECD64 record
    // directly before the locator. Prefer its 64-bit counts/offsets.
    let mut zip64 = false;
    let mut ecd64_pos: Option<u64> = None;
    let mut locator_off: Option<u64> = None;
    if idx >= 76
        && &buf[idx - 20..idx - 16] == b"PK\x06\x07"
        && &buf[idx - 76..idx - 72] == b"PK\x06\x06"
    {
        let z = &buf[idx - 76..];
        let z_entries = u64_at(z, 32);
        let z_cdsize = u64_at(z, 40);
        let z_cdoff = u64_at(z, 48);
        let l_off = u64_at(&buf[idx - 20..], 8);
        if z_cdsize <= size && z_cdoff <= size {
            zip64 = true;
            ecd64_pos = Some(eocd_pos - 76);
            locator_off = Some(l_off);
            entries = z_entries;
            cd_size = z_cdsize;
            cd_off = z_cdoff;
        }
    }

    // Locate the central directory; prefer the layout where the CD sits
    // directly before the ZIP64 records.
    let mut cd_start = eocd_pos.saturating_sub(cd_size);
    let mut valid = false;
    let is_cd_sig = |f: &mut File, off: u64| -> bool {
        let mut s = [0u8; 4];
        read_at(f, off, &mut s).unwrap_or(0) == 4 && &s == b"PK\x01\x02"
    };
    let mut found: Option<u64> = None;
    if let Some(zp) = ecd64_pos {
        if let Some(c) = zp.checked_sub(cd_size) {
            if c < size && is_cd_sig(&mut f, c) {
                found = Some(c);
            }
        }
    }
    if found.is_none() {
        let c = eocd_pos.saturating_sub(cd_size);
        if c < size && is_cd_sig(&mut f, c) {
            found = Some(c);
        }
    }
    if found.is_none() && cd_off > 0 && cd_off < size && is_cd_sig(&mut f, cd_off) {
        // Some writers store an absolute offset.
        found = Some(cd_off);
    }
    if let Some(c) = found {
        cd_start = c;
        valid = true;
    }

    // Archive start: candidates from the relative CD offset and the locator,
    // each verified against the local file-header signature.
    let mut base: Option<u64> = None;
    if valid {
        let mut cands: Vec<u64> = Vec::new();
        if let Some(b) = cd_start.checked_sub(cd_off) {
            cands.push(b);
        }
        if let (Some(zp), Some(l)) = (ecd64_pos, locator_off) {
            if let Some(b) = zp.checked_sub(l) {
                cands.push(b);
            }
        }
        cands.push(0);
        for b in cands {
            if b > size {
                continue;
            }
            let mut s = [0u8; 4];
            if read_at(&mut f, b, &mut s).unwrap_or(0) == 4 && &s == b"PK\x03\x04" {
                base = Some(b);
                break;
            }
        }
    }

    let mut first_name = Vec::new();
    if valid {
        let mut hdr = [0u8; 46];
        if read_at(&mut f, cd_start, &mut hdr).unwrap_or(0) == 46 {
            let nlen = u16::from_le_bytes([hdr[28], hdr[29]]) as usize;
            if nlen > 0 && nlen < 4096 {
                let mut nb = vec![0u8; nlen];
                if read_at(&mut f, cd_start + 46, &mut nb).unwrap_or(0) == nlen {
                    first_name = nb;
                }
            }
        }
    }
    Ok(Some(TailZip {
        eocd_pos,
        cd_start,
        entries,
        comment,
        first_name,
        valid,
        zip64,
        base,
        zip_end,
    }))
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    if o + 8 > b.len() {
        return 0;
    }
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let mut i = hay.len() - needle.len();
    loop {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

#[derive(Debug, Clone, Default)]
pub struct PeInfo {
    pub machine: u16,
    pub sections: u16,
    pub overlay: u64,
    pub signed: bool,
}

/// Minimal PE parse: confirms the file really is a Windows executable.
pub fn pe_info(path: &Path) -> Result<Option<PeInfo>> {
    let mut f = File::open(path).map_err(|e| UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut head = [0u8; 0x40];
    if read_at(&mut f, 0, &mut head).unwrap_or(0) < 0x40 || &head[0..2] != &b"MZ"[..] {
        return Ok(None);
    }
    let lfanew = u32::from_le_bytes([head[0x3c], head[0x3d], head[0x3e], head[0x3f]]) as u64;
    let mut coff = [0u8; 24];
    if read_at(&mut f, lfanew, &mut coff).unwrap_or(0) < 24 || &coff[0..4] != &b"PE\x00\x00"[..] {
        return Ok(None);
    }
    let machine = u16::from_le_bytes([coff[4], coff[5]]);
    let nsec = u16::from_le_bytes([coff[6], coff[7]]);
    let osz = u16::from_le_bytes([coff[20], coff[21]]) as u64;

    let opt = lfanew + 24;
    let mut magic = [0u8; 2];
    let mut signed = false;
    let mut overlay = 0u64;
    if read_at(&mut f, opt, &mut magic).unwrap_or(0) == 2 {
        let dd_base = match u16::from_le_bytes(magic) {
            0x10b => opt + 96,
            0x20b => opt + 112,
            _ => opt + 96,
        };
        let mut cert = [0u8; 8];
        if read_at(&mut f, dd_base + 4 * 8, &mut cert).unwrap_or(0) == 8 {
            let cert_off = u32::from_le_bytes([cert[0], cert[1], cert[2], cert[3]]);
            let cert_size = u32::from_le_bytes([cert[4], cert[5], cert[6], cert[7]]);
            signed = cert_off != 0 && cert_size != 0;
        }
    }
    let sec_base = opt + osz;
    for i in 0..nsec as u64 {
        let mut s = [0u8; 40];
        if read_at(&mut f, sec_base + i * 40, &mut s).unwrap_or(0) < 40 {
            break;
        }
        let raw_size = u32::from_le_bytes([s[16], s[17], s[18], s[19]]) as u64;
        let raw_ptr = u32::from_le_bytes([s[20], s[21], s[22], s[23]]) as u64;
        overlay = overlay.max(raw_ptr + raw_size);
    }
    Ok(Some(PeInfo {
        machine,
        sections: nsec,
        overlay,
        signed,
    }))
}

/// UI labels that look like passwords but are not.
const HINT_BLACKLIST: &[&str] = &[
    "password", "passwd", "pwd", "invalid", "error", "incorrect", "密码", "密码错误",
    "无效的密码", "输入密码", "显示密码", "确认密码", "解压密码", "错误", "失败",
];

/// Password-ish hints found in a ZIP comment or companion text.
pub fn hints_from_text(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Ok(re) = Regex::new(
        r#"(?i)(?:解压密码|解压码|提取码|密码|password|passwd|pwd)\s*[:：=]\s*["“']?([^\s"”'，。、；;\\]{1,64})"#,
    ) else {
        return out;
    };
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let v = m.as_str().trim_matches(|c: char| {
                c.is_whitespace() || c == '.' || c == ',' || c == ':' || c == '：' || c == '"' || c == '\''
            });
            if v.is_empty() || v.len() > 64 {
                continue;
            }
            let low = v.to_lowercase();
            if low.starts_with("http") || low.contains("://") || HINT_BLACKLIST.contains(&low.as_str())
            {
                continue;
            }
            if !out.iter().any(|x| x == v) {
                out.push(v.to_string());
            }
        }
    }
    out
}

/// Marker strings that identify common self-extractor/installer stubs.
const STUB_MARKERS: &[(&str, &str)] = &[
    ("BANDIZIPSFX", "Bandizip SFX"),
    ("WinRAR", "WinRAR SFX"),
    ("7zS.sfx", "7-Zip SFX"),
    ("7zS2.sfx", "7-Zip SFX"),
    ("Nullsoft", "NSIS installer"),
    ("Inno Setup", "Inno Setup installer"),
    ("InstallShield", "InstallShield installer"),
    ("SFXWizard", "SFXWizard"),
];

#[derive(Debug, Default)]
pub struct StubScan {
    pub hints: Vec<String>,
    pub markers: Vec<String>,
}

/// Scan the head of an executable for SFX markers and password hints.
pub fn scan_stub(path: &Path) -> StubScan {
    let mut scan = StubScan::default();
    let Ok(mut f) = File::open(path) else {
        return scan;
    };
    let cap = 8 * 1024 * 1024usize;
    let mut buf = vec![0u8; cap];
    let n = read_at(&mut f, 0, &mut buf).unwrap_or(0);
    buf.truncate(n);
    let ascii = String::from_utf8_lossy(&buf).into_owned();
    for (needle, label) in STUB_MARKERS {
        if ascii.contains(needle) {
            scan.markers.push(format!("stub marker: {label}"));
            break;
        }
    }
    let utf16 = decode_utf16le_lossy(&buf);
    let mut text = ascii;
    text.push('\n');
    text.push_str(&utf16);
    scan.hints = hints_from_text(&text);
    scan
}

fn decode_utf16le_lossy(buf: &[u8]) -> String {
    let mut s = String::with_capacity(buf.len() / 2);
    let mut i = 0;
    while i + 1 < buf.len() {
        let u = u16::from_le_bytes([buf[i], buf[i + 1]]);
        if (0x20..0xD800).contains(&u) || (0xE000..0xFFFD).contains(&u) {
            if let Some(c) = char::from_u32(u as u32) {
                s.push(c);
            }
        } else {
            s.push('\n');
        }
        i += 2;
    }
    s
}

fn find_local_header(path: &Path, name: &[u8], size: u64) -> Result<Option<u64>> {
    let mut f = File::open(path).map_err(|e| UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    const CHUNK: usize = 4 * 1024 * 1024;
    const OVERLAP: usize = 64;
    let mut buf = vec![0u8; CHUNK];
    let mut pos = 0u64;
    let mut tried = 0usize;
    while pos < size && tried < 1024 {
        let n = read_at(&mut f, pos, &mut buf).unwrap_or(0);
        if n < 4 {
            break;
        }
        let mut i = 0usize;
        while i + 30 <= n {
            if &buf[i..i + 4] == &b"PK\x03\x04"[..] {
                let nlen = u16::from_le_bytes([buf[i + 26], buf[i + 27]]) as usize;
                if i + 30 + nlen <= n {
                    if &buf[i + 30..i + 30 + nlen] == name {
                        return Ok(Some(pos + i as u64));
                    }
                } else if nlen > 0 && nlen < 4096 {
                    let mut nb = vec![0u8; nlen];
                    if read_at(&mut f, pos + i as u64 + 30, &mut nb).unwrap_or(0) == nlen
                        && nb == name
                    {
                        return Ok(Some(pos + i as u64));
                    }
                }
                tried += 1;
                if tried >= 1024 {
                    return Ok(None);
                }
            }
            i += 1;
        }
        if n < CHUNK {
            break;
        }
        pos += (CHUNK - OVERLAP) as u64;
    }
    Ok(None)
}

/// Copy `src[start..end]` into a standalone temp file so an offset archive can
/// be read standalone (7-Zip refuses ZIP64 archives that carry a prefix).
///
/// `key` makes the copy reusable: repeated detections of the same source reuse
/// the cached carve instead of copying several gigabytes again. Entries older
/// than 72 h are pruned opportunistically on the next carve.
pub fn carve_to_temp(src: &Path, start: u64, end: u64, key: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("paranoia-carve");
    fs::create_dir_all(&dir).map_err(|e| UpError::Io {
        path: dir.clone(),
        source: e,
    })?;
    prune_carve_cache(&dir);

    let size = fs::metadata(src)
        .map_err(|e| UpError::Io {
            path: src.to_path_buf(),
            source: e,
        })?
        .len();
    let start = start.min(size);
    let end = end.min(size);
    let want = end.saturating_sub(start);
    if want == 0 {
        return Err(UpError::Other("carve range is empty".into()));
    }

    let out = dir.join(format!("{key}-{start:x}.zip"));
    if let Ok(m) = fs::metadata(&out) {
        if m.len() == want {
            return Ok(out);
        }
    }

    let part = dir.join(format!(".part-{}", Uuid::new_v4().simple()));
    let copied = (|| -> std::io::Result<()> {
        let mut in_f = File::open(src)?;
        in_f.seek(SeekFrom::Start(start))?;
        let mut out_f = File::create(&part)?;
        let mut buf = vec![0u8; 1 << 20];
        let mut remaining = want;
        while remaining > 0 {
            let chunk = buf.len().min(remaining as usize);
            let n = in_f.read(&mut buf[..chunk])?;
            if n == 0 {
                break;
            }
            out_f.write_all(&buf[..n])?;
            remaining -= n as u64;
        }
        out_f.flush()?;
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = fs::remove_file(&part);
        return Err(UpError::Io {
            path: out.clone(),
            source: e,
        });
    }
    fs::rename(&part, &out).map_err(|e| UpError::Io {
        path: out.clone(),
        source: e,
    })?;
    Ok(out)
}

/// Drop carve-cache entries older than 72 h (best effort).
fn prune_carve_cache(dir: &Path) {
    let now = SystemTime::now();
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > Duration::from_secs(72 * 3600) {
            let _ = remove_path(&e.path());
        }
    }
}

/// Recognise a Paranoia File Encryption v4 (`SSEFE`) container from its header
/// without touching 7-Zip.
///
/// Files with a `.enc` extension whose magic is not `SSEFE` are deliberately
/// *not* classified here: they fall through to the normal archive sniffing, so a
/// renamed ZIP/RAR still works.
pub fn ssefe_detection(head: &[u8], size: u64, path: &Path) -> Option<Detection> {
    let probe = crate::ssefe::probe_head(head, size, path)?;
    let notes = vec![
        probe.describe(),
        format!(
            "Argon2id customParams=0x{:02x} (tm={}, mm={})",
            probe.custom_params, probe.tm, probe.mm
        ),
        "需要 Paranoia v4 明文口令；解密后按内层 ZIP 解压".to_string(),
        format!(
            "SSEFE header version={} algorithm=0x{:02x}",
            probe.version, probe.algorithm
        ),
    ];
    Some(Detection {
        path: path.to_path_buf(),
        size,
        kind: FileKind::SsefeEncrypted,
        container: "ssefe".to_string(),
        format: Some("paranoia-enc-v4".to_string()),
        password_state: PasswordState::NeedsPassword,
        fingerprint: fingerprint(path).unwrap_or_default(),
        notes,
        ..Default::default()
    })
}

/// Full detection for one file.
pub async fn detect(sz: &SevenZip, path: &Path, cancel: &CancelToken) -> Result<Detection> {
    let meta = fs::metadata(path).map_err(|e| UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let size = meta.len();

    let mut f = File::open(path).map_err(|e| UpError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut head = vec![0u8; HEAD_SCAN];
    let n = read_at(&mut f, 0, &mut head).unwrap_or(0);
    head.truncate(n);
    drop(f);

    // Paranoia File Encryption v4 must never reach 7-Zip: the plaintext ZIP only
    // exists after decryption, so listing the ciphertext would fail noisily.
    if let Some(det) = ssefe_detection(&head, size, path) {
        return Ok(det);
    }

    let magic = sniff(&head).map(|s| s.to_string());
    let pe = if head.starts_with(b"MZ") {
        pe_info(path).unwrap_or(None)
    } else {
        None
    };
    let container = if pe.is_some() {
        "pe"
    } else if head.len() >= 8 && &head[4..8] == &b"ftyp"[..] {
        "mp4"
    } else if magic.is_some() {
        "plain"
    } else {
        "other"
    };

    let tail = analyze_tail(path, size).unwrap_or(None);

    let mut det = Detection {
        path: path.to_path_buf(),
        size,
        kind: FileKind::Unknown,
        container: container.to_string(),
        format: magic.clone(),
        fingerprint: fingerprint(path).unwrap_or_default(),
        ..Default::default()
    };

    if let Some(t) = &tail {
        if !t.comment.is_empty() {
            det.hints = hints_from_text(&t.comment);
            det.notes
                .push(format!("zip comment: {} bytes", t.comment.len()));
        }
        if t.valid {
            det.notes.push(format!(
                "zip central directory at {} ({} entries)",
                t.cd_start, t.entries
            ));
        }
        if let Some(b) = t.base {
            if b > 0 {
                det.notes.push(format!(
                    "archive with a {b}-byte prefix before the archive start"
                ));
            }
        }
    }
    if let Some(p) = &pe {
        det.notes.push(format!(
            "PE machine=0x{:04x} sections={} overlay={} signed={}",
            p.machine, p.sections, p.overlay, p.signed
        ));
    }

    // SFX stubs and archive comments often carry the password in plain text.
    if pe.is_some() {
        let scan = scan_stub(path);
        for h in scan.hints {
            if !det.hints.contains(&h) {
                det.hints.push(h);
            }
        }
        det.notes.extend(scan.markers);
    }

    let mut listing: Option<ListResult> = None;
    let mut list_err: Option<UpError> = None;
    match sz.list(path, None, cancel).await {
        Ok(l) => listing = Some(l),
        Err(e) => list_err = Some(e),
    }

    // Fallback: carve the payload out when 7-Zip cannot open the container
    // directly. 7-Zip 25.01 refuses masked archives that keep standard
    // (relative) offsets inside a prefixed file — both ZIP32 and ZIP64
    // layouts; a standalone byte-range copy always reads.
    if listing.is_none() {
        if let Some(crate::error::UpError::Cancelled) = &list_err {
            return Err(UpError::Cancelled);
        }
        let carveable = matches!(list_err.as_ref(), Some(UpError::NotAnArchive(_)))
            || matches!(list_err.as_ref(), Some(UpError::SevenZipFailed { .. }))
            || matches!(list_err.as_ref(), Some(UpError::PasswordRequired));
        if carveable {
            if let Some(t) = &tail {
                if t.valid {
                    let mut ranges: Vec<(u64, u64)> = Vec::new();
                    if let Some(b) = t.base {
                        let end = t.zip_end.min(size);
                        if end > b {
                            ranges.push((b, end));
                        }
                    } else if !t.first_name.is_empty() {
                        // No verified base (legacy layouts): find the first
                        // local header by name and carve to EOF.
                        if let Ok(Some(off)) = find_local_header(path, &t.first_name, size) {
                            if off < size {
                                ranges.push((off, size));
                            }
                        }
                    }
                    for (from, to) in ranges {
                        let Ok(tmp) = carve_to_temp(path, from, to, &det.fingerprint) else {
                            continue;
                        };
                        match sz.list(&tmp, None, cancel).await {
                            Ok(l2) => {
                                listing = Some(l2);
                                det.carved_from = Some(from);
                                det.carved_path = Some(tmp);
                                let what = if t.zip64 { "ZIP64 payload" } else { "payload" };
                                det.notes.push(format!("{what} carved from offset {from}"));
                                break;
                            }
                            Err(_) => {
                                let _ = fs::remove_file(&tmp);
                            }
                        }
                    }
                }
            }
        }
    }

    let needs_pw = matches!(list_err.as_ref(), Some(UpError::PasswordRequired));
    let not_archive = matches!(list_err.as_ref(), Some(UpError::NotAnArchive(_)));

    if let Some(l) = &listing {
        det.listing_ok_no_pw = true;
        if l.offset > 0 {
            det.offset = Some(l.offset);
        }
        if l.stub_size > 0 {
            det.stub_size = Some(l.stub_size);
        }
        if !l.archive_type.is_empty() {
            det.format = Some(l.archive_type.clone());
        }
        det.total_entries = l.file_count();
        det.encrypted_entries = l.encrypted_entries().count();
        det.top_level = l.top_level(true);
        det.total_uncompressed = l.total_uncompressed();
        det.quick_entry = l.smallest_encrypted().map(|e| e.raw_path.clone());
        det.password_state = if det.encrypted_entries > 0 {
            PasswordState::NeedsPassword
        } else {
            PasswordState::NoPassword
        };
        if l.looks_like_office() {
            det.office_like = true;
            det.notes
                .push("entry layout looks like OOXML/JAR (document container)".into());
        }
        det.kind = if container == "pe" {
            if l.archive_type.eq_ignore_ascii_case("PE") {
                // 7-Zip opened the file only as a PE container (section list):
                // there is no payload we could extract.
                det.notes
                    .push("PE 自身（仅区段列表），未发现可读的内嵌归档".into());
                FileKind::NotArchive
            } else {
                FileKind::SfxExecutable
            }
        } else if det.offset.is_some() || det.carved_from.is_some() {
            FileKind::MaskedArchive
        } else {
            FileKind::Archive
        };
    } else if needs_pw {
        det.password_state = PasswordState::NeedsPassword;
        det.kind = if container == "pe" {
            FileKind::SfxExecutable
        } else if container == "plain" {
            FileKind::Archive
        } else {
            FileKind::MaskedArchive
        };
        if tail.as_ref().map(|t| t.valid).unwrap_or(false) && container != "plain" {
            det.notes
                .push("header encrypted: password needed before listing".into());
        }
    } else if not_archive {
        det.kind = if magic.is_some() {
            FileKind::Archive
        } else {
            FileKind::NotArchive
        };
        if pe.is_some() {
            det.notes.push(
                "PE executable without a readable appended archive (installer or packed)".into(),
            );
        }
    } else {
        det.kind = if magic.is_some() {
            FileKind::Archive
        } else {
            FileKind::Unknown
        };
    }

    Ok(det)
}

/// Convenience: list an archive tolerating a missing password.
pub async fn try_list(
    sz: &SevenZip,
    path: &Path,
    password: Option<&str>,
    cancel: &CancelToken,
) -> Option<ListResult> {
    sz.list(path, password, cancel).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_detects_zip_inside_mp4_like_head() {
        assert_eq!(sniff(b"\x00\x00\x00\x18ftypmp42"), None);
        assert_eq!(sniff(b"PK\x03\x04rest"), Some("zip"));
        assert_eq!(sniff(b"7z\xbc\xaf\x27\x1c"), Some("7z"));
    }

    #[test]
    fn hints_extract_from_bandizip_comment() {
        let text = "; Bandizip sfx settings\r\nMESSAGE=\"此资源由B站 可可HXL 整理\\r\\n解压码：kekehxl\\r\\n点击免费获取更多资源：\\r\\nhttps://vlink.cc/kekehxl\"";
        let h = hints_from_text(text);
        assert!(h.contains(&"kekehxl".to_string()), "hints = {h:?}");
    }

    #[test]
    fn hints_ignore_urls_and_ui_labels() {
        assert!(hints_from_text("密码：https://example.com").is_empty());
        let h = hints_from_text("ERR_INVALID_PASSWORD = 无效的密码\nSTATIC_LABEL = 输入密码");
        assert!(h.is_empty(), "hints = {h:?}");
    }

    #[test]
    fn hints_keep_real_password_next_to_labels() {
        let text = "; Bandizip sfx settings\r\nMESSAGE=\"解压码：kekehxl\"\r\n[LANG]\r\nERR_INVALID_PASSWORD = 无效的密码\r\n";
        let h = hints_from_text(text);
        assert_eq!(h, vec!["kekehxl".to_string()], "hints = {h:?}");
    }

    // ---- ZIP tail analysis (masked archives) --------------------------------

    fn le16(v: u16) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }
    fn le32(v: u32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }
    fn le64(v: u64) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    /// prefix + one-entry ZIP archive (optionally ZIP64 records) + junk.
    /// Returns (bytes, base_offset, zip_end_offset).
    fn build_masked_zip(prefix: usize, junk: usize, zip64: bool) -> (Vec<u8>, u64, u64) {
        let name = b"payload.rar";
        let mut body: Vec<u8> = Vec::new();
        // local file header (+ zip64 extra)
        body.extend_from_slice(b"PK\x03\x04");
        body.extend(le16(45));
        body.extend(le16(0x0800));
        body.extend(le16(0));
        body.extend(le16(0));
        body.extend(le16(0));
        body.extend(le32(0xDEAD_BEEF));
        body.extend(le32(0xFFFF_FFFF));
        body.extend(le32(0xFFFF_FFFF));
        body.extend(le16(name.len() as u16));
        body.extend(le16(20));
        body.extend_from_slice(name);
        body.extend(le16(1));
        body.extend(le16(16));
        body.extend(le64(12_345));
        body.extend(le64(23_456));
        body.extend_from_slice(&[0x42u8; 64]);
        let cd_off = body.len() as u64;
        // central directory entry
        let mut cd: Vec<u8> = Vec::new();
        cd.extend_from_slice(b"PK\x01\x02");
        cd.extend(le16(45));
        cd.extend(le16(45));
        cd.extend(le16(0x0800));
        cd.extend(le16(0));
        cd.extend(le16(0));
        cd.extend(le16(0));
        cd.extend(le32(0xDEAD_BEEF));
        cd.extend(le32(0xFFFF_FFFF));
        cd.extend(le32(0xFFFF_FFFF));
        cd.extend(le16(name.len() as u16));
        cd.extend(le16(20));
        cd.extend(le16(0));
        cd.extend(le16(0));
        cd.extend(le16(0));
        cd.extend(le32(0));
        cd.extend(le32(0));
        cd.extend_from_slice(name);
        cd.extend(le16(1));
        cd.extend(le16(16));
        cd.extend(le64(12_345));
        cd.extend(le64(23_456));
        let cd_size = cd.len() as u64;
        body.extend_from_slice(&cd);
        if zip64 {
            let ecd64_pos = body.len() as u64;
            body.extend_from_slice(b"PK\x06\x06");
            body.extend(le64(44));
            body.extend(le16(45));
            body.extend(le16(45));
            body.extend(le32(0));
            body.extend(le32(0));
            body.extend(le64(1));
            body.extend(le64(1));
            body.extend(le64(cd_size));
            body.extend(le64(cd_off));
            body.extend_from_slice(b"PK\x06\x07");
            body.extend(le32(0));
            body.extend(le64(ecd64_pos));
            body.extend(le32(1));
        }
        let comment = b"note: hi";
        body.extend_from_slice(b"PK\x05\x06");
        body.extend(le16(0));
        body.extend(le16(0));
        body.extend(le16(1));
        body.extend(le16(1));
        body.extend(le32(cd_size as u32));
        body.extend(le32(cd_off as u32));
        body.extend(le16(comment.len() as u16));
        body.extend_from_slice(comment);
        let zip_end = prefix as u64 + body.len() as u64;
        let mut bytes = vec![0xABu8; prefix];
        bytes.extend_from_slice(&body);
        bytes.extend(vec![0x77u8; junk]);
        (bytes, prefix as u64, zip_end)
    }

    fn temp_probe(name: &str, bytes: &[u8]) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("paranoia-test-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        (dir, p)
    }

    #[test]
    fn zip64_tail_with_prefix_yields_exact_range() {
        let (bytes, base, zip_end) = build_masked_zip(4_096, 1_000, true);
        let (dir, p) = temp_probe("masked.mp4", &bytes);
        let t = analyze_tail(&p, bytes.len() as u64).unwrap().unwrap();
        assert!(t.valid, "tail = {t:?}");
        assert!(t.zip64);
        assert_eq!(t.entries, 1);
        assert_eq!(t.base, Some(base));
        assert_eq!(t.zip_end, zip_end);
        assert_eq!(t.first_name, b"payload.rar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appended_zip_tail_still_computes_its_base() {
        let (bytes, base, zip_end) = build_masked_zip(2_048, 0, false);
        let (dir, p) = temp_probe("appended.mp4", &bytes);
        let t = analyze_tail(&p, bytes.len() as u64).unwrap().unwrap();
        assert!(t.valid, "tail = {t:?}");
        assert!(!t.zip64);
        assert_eq!(t.base, Some(base));
        assert_eq!(t.zip_end, zip_end);
        assert_eq!(t.first_name, b"payload.rar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn carve_copies_exact_byte_range_and_reuses_cache() {
        let (bytes, base, zip_end) = build_masked_zip(1_024, 300, true);
        let (dir, p) = temp_probe("src.mp4", &bytes);
        let key = format!("testkey-{}", Uuid::new_v4().simple());
        let c1 = carve_to_temp(&p, base, zip_end, &key).unwrap();
        let data = std::fs::read(&c1).unwrap();
        assert_eq!(data.len() as u64, zip_end - base);
        assert_eq!(&data[..4], b"PK\x03\x04");
        let c2 = carve_to_temp(&p, base, zip_end, &key).unwrap();
        assert_eq!(c1, c2, "second carve should reuse the cached copy");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&c1);
    }

    fn ssefe_header(custom: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(104);
        v.extend_from_slice(b"SSEFE");
        v.push(4); // format version
        v.push(0); // algorithm: AES-256
        v.push(custom);
        v.extend_from_slice(&[0u8; 32]); // salt
        v.extend_from_slice(&[0xAAu8; 32]); // checkcode
        v.extend_from_slice(&[0xBBu8; 32]); // MAC
        v
    }

    #[test]
    fn ssefe_header_is_detected_as_paranoia_v4() {
        let head = ssefe_header(0x00);
        let det = ssefe_detection(&head, head.len() as u64, std::path::Path::new("x.enc"))
            .expect("ssefe header must be recognised");
        assert_eq!(det.kind, FileKind::SsefeEncrypted);
        assert_eq!(det.container, "ssefe");
        assert_eq!(det.format.as_deref(), Some("paranoia-enc-v4"));
        assert_eq!(det.password_state, PasswordState::NeedsPassword);
        assert_eq!(det.size, 104);
        assert!(det
            .notes
            .iter()
            .any(|n| n.contains("Paranoia File Encryption v4 (SSEFE) AES-256")));
    }

    #[test]
    fn non_ssefe_or_short_files_are_not_classified_as_ssefe() {
        // A renamed ZIP must fall through to content sniffing.
        let zip = b"PK\x03\x04rest";
        assert!(ssefe_detection(zip, zip.len() as u64, std::path::Path::new("x.enc")).is_none());
        // Magic is right but the file is shorter than the 104-byte minimum.
        let short = &ssefe_header(0x00)[..40];
        assert!(ssefe_detection(short, 40, std::path::Path::new("x.enc")).is_none());
        // Wrong SSEFE version.
        let mut bad = ssefe_header(0x00);
        bad[5] = 3;
        assert!(ssefe_detection(&bad, bad.len() as u64, std::path::Path::new("x.enc")).is_none());
        // Wrong algorithm.
        let mut bad_alg = ssefe_header(0x00);
        bad_alg[6] = 1;
        assert!(
            ssefe_detection(&bad_alg, bad_alg.len() as u64, std::path::Path::new("x.enc"))
                .is_none()
        );
    }
}
