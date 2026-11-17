//! End-to-end decryption of a single Paranoia v4 AES-256 `.enc` file.
//!
//! Pipeline (all in memory-bounded streaming form):
//! 1. parse and validate the plaintext 8-byte header + 32-byte salt,
//! 2. detect the optional PFTEREC1 recovery container and reject it,
//! 3. derive the keys (Argon2id + HKDF),
//! 4. decrypt AES-256-CTR into a temporary file, validating the password checkcode on the
//!    first block and stripping it so the temporary file is the bare inner ZIP,
//! 5. verify the keyed BLAKE3 MAC over `nonce || header || salt || ciphertext`,
//! 6. extract the inner ZIP into the output directory (temp file + rename per entry).
//!
//! Any error path removes the temporary files, so no partial output is ever left behind.
//!
//! The v0.2.0 merge also exposes two smaller building blocks used by the batch unpack
//! engine: [`decrypt_to_archive`] (decrypt only, leave the inner ZIP on disk) and
//! [`verify_password`] (cheap password probe that does not touch the whole ciphertext).

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::crypto::{constant_time_eq, CtrStream, MacAccumulator};
use crate::error::{AddFileError, DecryptError};
use crate::format::{
    parse_argon2_params, parse_header, Header, CHECKCODE_LEN, HEADER_LEN, MAC_LEN, RECOVERY_MAGIC,
    SALT_LEN,
};
use crate::kdf;
use crate::output::ConflictStrategy;
use crate::util;
use crate::zip_extract;

/// Header + salt (40 bytes) + checkcode (32) + MAC (32).
const MIN_FILE_LEN: u64 = (HEADER_LEN + SALT_LEN + CHECKCODE_LEN + MAC_LEN) as u64;
/// Bytes before the ciphertext stream starts.
const PREFIX_LEN: u64 = (HEADER_LEN + SALT_LEN) as u64;
const CHUNK_SIZE: usize = 1 << 20;

#[derive(Debug, Clone)]
pub struct DecryptOptions {
    /// Directory the restored files are written to.
    pub output_dir: PathBuf,
    /// What to do when an output file already exists.
    pub conflict: ConflictStrategy,
}

#[derive(Debug, Clone)]
pub struct DecryptReport {
    pub input: PathBuf,
    pub outputs: Vec<PathBuf>,
    pub original_name: String,
    pub bytes_written: u64,
}

/// Result of [`decrypt_to_archive`]: the decrypted inner ZIP that still has to be extracted.
#[derive(Debug, Clone)]
pub struct DecryptArchiveReport {
    pub input: PathBuf,
    /// The decrypted inner ZIP written to the caller-provided path.
    pub archive: PathBuf,
    /// Size of the inner ZIP in bytes (the 32-byte checkcode is not part of it).
    pub archive_bytes: u64,
}

/// Inspect the plaintext header of an `.enc` file without decrypting it.
pub fn inspect_file(path: &Path) -> Result<Header, DecryptError> {
    let len = fs::metadata(path)?.len();
    if len < PREFIX_LEN {
        return Err(DecryptError::TooShort);
    }
    let mut prefix = [0u8; HEADER_LEN + SALT_LEN];
    File::open(path)?.read_exact(&mut prefix)?;
    parse_header(&prefix)
}

/// Validate a file before adding it to the batch list: `.enc` extension, regular file,
/// readable and a supported Paranoia v4 AES-256 header.
pub fn validate_input_path(path: &Path) -> Result<Header, AddFileError> {
    let has_enc_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("enc"))
        .unwrap_or(false);
    if !has_enc_extension {
        return Err(AddFileError::NotEncExtension);
    }
    let meta = fs::metadata(path).map_err(AddFileError::CannotRead)?;
    if !meta.is_file() {
        return Err(AddFileError::NotRegularFile);
    }
    if meta.len() < PREFIX_LEN {
        return Err(AddFileError::TooShort);
    }
    let mut prefix = [0u8; HEADER_LEN + SALT_LEN];
    let mut file = File::open(path).map_err(AddFileError::CannotRead)?;
    file.read_exact(&mut prefix).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            AddFileError::TooShort
        } else {
            AddFileError::CannotRead(error)
        }
    })?;
    parse_header(&prefix).map_err(AddFileError::from)
}

/// Parse the header and the trailing recovery marker, and decode the KDF parameters.
fn read_container(input: &Path) -> Result<(File, Header, crate::format::Argon2Params, u64), DecryptError> {
    let len = fs::metadata(input)?.len();
    if len < MIN_FILE_LEN {
        return Err(DecryptError::TooShort);
    }
    let mut file = File::open(input)?;
    let mut prefix = [0u8; HEADER_LEN + SALT_LEN];
    file.read_exact(&mut prefix)?;
    let header = parse_header(&prefix)?;

    let ciphertext_len = len - PREFIX_LEN - MAC_LEN as u64;
    if ciphertext_len < CHECKCODE_LEN as u64 {
        return Err(DecryptError::Truncated);
    }

    // Optional recovery container sits right before the MAC. Reject it explicitly.
    {
        let mut probe = File::open(input)?;
        probe.seek(SeekFrom::Start(len - MAC_LEN as u64 - 8))?;
        let mut magic = [0u8; 8];
        probe.read_exact(&mut magic)?;
        if magic == RECOVERY_MAGIC {
            return Err(DecryptError::UnsupportedRecovery);
        }
    }

    let params = parse_argon2_params(header.custom_params)?;
    Ok((file, header, params, ciphertext_len))
}

/// Streaming decryption of checkcode + inner ZIP into `out`.
///
/// Validates the password checkcode on the first block and the keyed BLAKE3 MAC after the
/// whole ciphertext has been consumed. On any error the partially written `out` is removed.
/// Returns the inner-ZIP byte count (the 32-byte checkcode is validated but not written).
fn decrypt_stream(
    input: &Path,
    password: &str,
    out: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, DecryptError> {
    let (_file, header, params, _ciphertext_len) = read_container(input)?;
    let keys = kdf::derive_keys(password, &header.salt, &params)?;
    decrypt_stream_with_keys(input, &keys, out, None, progress)
}

fn decrypt_stream_with_keys(
    input: &Path,
    keys: &kdf::Keys,
    out: &Path,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, DecryptError> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err(DecryptError::Cancelled);
    }
    let (mut file, header, _params, ciphertext_len) = read_container(input)?;

    let guard = util::TempGuard::new(out.to_path_buf());
    file.seek(SeekFrom::Start(PREFIX_LEN))?;
    let mut reader = BufReader::with_capacity(CHUNK_SIZE, file.take(ciphertext_len));
    let mut writer = BufWriter::with_capacity(CHUNK_SIZE, File::create(out)?);

    let mut mac = MacAccumulator::new(&keys.auth_key);
    mac.update(&keys.nonce);
    mac.update(&header.raw);
    mac.update(&header.salt);
    let mut cipher = CtrStream::new(&keys.enc_key, &keys.nonce);

    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut processed: u64 = 0;
    let mut strip_checkcode = true;
    while processed < ciphertext_len {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(DecryptError::Cancelled);
        }
        let want = std::cmp::min(CHUNK_SIZE as u64, ciphertext_len - processed) as usize;
        reader.read_exact(&mut buffer[..want])?;
        mac.update(&buffer[..want]);
        cipher.apply(&mut buffer[..want]);
        let mut offset = 0usize;
        if strip_checkcode {
            // The first 32 plaintext bytes are the [a-z0-9] checkcode that guards the
            // password; everything after it is the inner ZIP. Do not write the checkcode:
            // the caller wants a bare archive (7-Zip does not tolerate a prefixed ZIP).
            let checkcode = CHECKCODE_LEN.min(want);
            if !util::is_valid_checkcode(&buffer[..checkcode]) {
                return Err(DecryptError::WrongPassword);
            }
            offset = checkcode;
            strip_checkcode = false;
        }
        writer.write_all(&buffer[offset..want])?;
        processed += want as u64;
        progress(processed, ciphertext_len);
    }
    writer.flush()?;
    drop(writer);
    drop(reader);

    let mut stored_mac = [0u8; MAC_LEN];
    {
        let mut mac_file = File::open(input)?;
        mac_file.seek(SeekFrom::End(-(MAC_LEN as i64)))?;
        mac_file.read_exact(&mut stored_mac)?;
    }
    if !constant_time_eq(&mac.finalize(), &stored_mac) {
        return Err(DecryptError::Integrity);
    }

    guard.keep();
    Ok(ciphertext_len - CHECKCODE_LEN as u64)
}

/// Decrypt one `.enc` file into its inner ZIP archive without extracting it.
///
/// The 32-byte plaintext password checkcode is validated and stripped, so `out_zip` is a
/// bare ZIP archive (it starts with `PK\x03\x04`) that can be handed directly to 7-Zip.
/// This is the building block used by the batch engine. The whole ciphertext is authenticated
/// (MAC) before `Ok` is returned.
pub fn decrypt_to_archive(
    input: &Path,
    password: &str,
    out_zip: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<DecryptArchiveReport, DecryptError> {
    if let Some(parent) = out_zip.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let archive_bytes = decrypt_stream(input, password, out_zip, progress)?;
    Ok(DecryptArchiveReport {
        input: input.to_path_buf(),
        archive: out_zip.to_path_buf(),
        archive_bytes,
    })
}

/// Variant of [`decrypt_to_archive`] that reuses keys already derived by
/// [`verify_password_keys`] and supports cooperative cancellation via `cancel`.
pub fn decrypt_to_archive_with_keys(
    input: &Path,
    keys: &kdf::Keys,
    out_zip: &Path,
    cancel: Option<&AtomicBool>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<DecryptArchiveReport, DecryptError> {
    if let Some(parent) = out_zip.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let archive_bytes = decrypt_stream_with_keys(input, keys, out_zip, cancel, progress)?;
    Ok(DecryptArchiveReport {
        input: input.to_path_buf(),
        archive: out_zip.to_path_buf(),
        archive_bytes,
    })
}

/// Cheap password probe: derive the keys and check the 32-byte plaintext checkcode only.
///
/// `Ok(())` means the password is correct with overwhelming probability (false positive is
/// about 36^-32). It does not authenticate the whole ciphertext; global integrity is checked by
/// [`decrypt_to_archive`] / [`decrypt_file`].
pub fn verify_password(input: &Path, password: &str) -> Result<(), DecryptError> {
    verify_password_keys(input, password).map(|_| ())
}

/// Same as [`verify_password`], returning the derived [`kdf::Keys`] so the caller can
/// pass them straight into [`decrypt_to_archive_with_keys`] without running Argon2id twice.
pub fn verify_password_keys(input: &Path, password: &str) -> Result<kdf::Keys, DecryptError> {
    let (mut file, header, params, _ciphertext_len) = read_container(input)?;
    let keys = kdf::derive_keys(password, &header.salt, &params)?;
    file.seek(SeekFrom::Start(PREFIX_LEN))?;
    let mut buffer = [0u8; CHECKCODE_LEN];
    file.read_exact(&mut buffer)?;
    CtrStream::new(&keys.enc_key, &keys.nonce).apply(&mut buffer);
    if !util::is_valid_checkcode(&buffer) {
        return Err(DecryptError::WrongPassword);
    }
    Ok(keys)
}

/// Decrypt one file. `progress` receives (processed ciphertext bytes, total ciphertext bytes).
pub fn decrypt_file(
    input: &Path,
    password: &str,
    opts: &DecryptOptions,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<DecryptReport, DecryptError> {
    let len = fs::metadata(input)?.len();
    if len < MIN_FILE_LEN {
        return Err(DecryptError::TooShort);
    }

    fs::create_dir_all(&opts.output_dir)?;
    let temp_zip = util::unique_temp_path(&opts.output_dir, ".upzip-enc-decrypt-");
    let temp_guard = util::TempGuard::new(temp_zip.clone());

    decrypt_stream(input, password, &temp_zip, progress)?;
    let extract = zip_extract::extract_zip(&temp_zip, &opts.output_dir, opts.conflict)?;
    drop(temp_guard); // removes the temporary decrypted ZIP

    Ok(DecryptReport {
        input: input.to_path_buf(),
        outputs: extract.outputs,
        original_name: extract.original_name,
        bytes_written: extract.bytes_written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{make_enc_file, make_enc_file_stored, test_salt};
    use crate::format::FORMAT_VERSION;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "upzip-enc-test-decrypt-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn options(dir: &Path, conflict: ConflictStrategy) -> DecryptOptions {
        DecryptOptions {
            output_dir: dir.to_path_buf(),
            conflict,
        }
    }

    #[test]
    fn decrypt_round_trip_restores_file() {
        let dir = temp_dir("roundtrip");
        let input = dir.join("sample.enc");
        fs::write(
            &input,
            make_enc_file(
                "123",
                &test_salt(1),
                0x11,
                &[("hello.txt", b"hello paranoia")],
            ),
        )
        .unwrap();
        let out_dir = dir.join("out");
        let mut progress_calls = 0usize;
        let report = decrypt_file(&input, "123", &options(&out_dir, ConflictStrategy::Rename), &mut |_, _| {
            progress_calls += 1;
        })
        .expect("decrypt");
        assert_eq!(report.original_name, "hello.txt");
        assert_eq!(report.outputs, vec![out_dir.join("hello.txt")]);
        assert_eq!(fs::read(&out_dir.join("hello.txt")).unwrap(), b"hello paranoia");
        assert_eq!(report.bytes_written, 14);
        assert!(progress_calls >= 1);
        assert!(fs::read_dir(&out_dir).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            !name.starts_with(".upzip-enc-")
        }));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decrypt_stored_zip_entry() {
        let dir = temp_dir("stored");
        let input = dir.join("stored.enc");
        fs::write(
            &input,
            make_enc_file_stored("pw", &test_salt(2), 0x11, "nested.bin", &[7u8; 1000]),
        )
        .unwrap();
        let out_dir = dir.join("out");
        let report = decrypt_file(&input, "pw", &options(&out_dir, ConflictStrategy::Rename), &mut |_, _| {})
            .expect("decrypt");
        assert_eq!(report.original_name, "nested.bin");
        assert_eq!(fs::read(out_dir.join("nested.bin")).unwrap(), vec![7u8; 1000]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wrong_password_is_reported() {
        let dir = temp_dir("wrongpw");
        let input = dir.join("sample.enc");
        fs::write(
            &input,
            make_enc_file("123", &test_salt(3), 0x11, &[("a.txt", b"data")]),
        )
        .unwrap();
        let out_dir = dir.join("out");
        let error = decrypt_file(&input, "456", &options(&out_dir, ConflictStrategy::Rename), &mut |_, _| {})
            .unwrap_err();
        assert_eq!(error.kind(), "wrongPassword");
        // No partial output must remain.
        assert!(!out_dir.join("a.txt").exists());
        let leftovers: Vec<_> = fs::read_dir(&out_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tampered_ciphertext_fails_mac() {
        let dir = temp_dir("tamper");
        let input = dir.join("sample.enc");
        let mut bytes = make_enc_file("123", &test_salt(4), 0x11, &[("a.txt", b"data that is long enough")]);
        // Flip a ciphertext byte after the checkcode region.
        let offset = 40 + 32 + 3;
        bytes[offset] ^= 0xFF;
        fs::write(&input, &bytes).unwrap();
        let out_dir = dir.join("out");
        let error = decrypt_file(&input, "123", &options(&out_dir, ConflictStrategy::Rename), &mut |_, _| {})
            .unwrap_err();
        assert_eq!(error.kind(), "integrity");
        assert!(!out_dir.join("a.txt").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncated_file_is_rejected() {
        let dir = temp_dir("truncated");
        let mut bytes = make_enc_file("123", &test_salt(5), 0x11, &[("a.txt", b"data")]);
        bytes.truncate(60); // below the 104-byte minimum
        let input = dir.join("short.enc");
        fs::write(&input, &bytes).unwrap();
        let error = decrypt_file(&input, "123", &options(&dir, ConflictStrategy::Rename), &mut |_, _| {})
            .unwrap_err();
        assert_eq!(error.kind(), "tooShort");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let dir = temp_dir("version");
        let mut bytes = make_enc_file("123", &test_salt(6), 0x11, &[("a.txt", b"data")]);
        bytes[5] = FORMAT_VERSION + 1;
        let input = dir.join("future.enc");
        fs::write(&input, &bytes).unwrap();
        let error = decrypt_file(&input, "123", &options(&dir, ConflictStrategy::Rename), &mut |_, _| {})
            .unwrap_err();
        assert_eq!(error.kind(), "unsupportedVersion");
        assert!(error.to_string().contains("unsupported format version"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_aes256_algorithm_is_rejected() {
        let dir = temp_dir("algorithm");
        let mut bytes = make_enc_file("123", &test_salt(7), 0x11, &[("a.txt", b"data")]);
        bytes[6] = 0x01;
        let input = dir.join("alg.enc");
        fs::write(&input, &bytes).unwrap();
        let error = decrypt_file(&input, "123", &options(&dir, ConflictStrategy::Rename), &mut |_, _| {})
            .unwrap_err();
        assert_eq!(error.kind(), "unsupportedAlgorithm");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recovery_container_is_detected_before_decryption() {
        let dir = temp_dir("recovery");
        let mut bytes = make_enc_file("123", &test_salt(8), 0x11, &[("a.txt", b"data")]);
        let len = bytes.len();
        bytes[len - 40..len - 32].copy_from_slice(&RECOVERY_MAGIC);
        let input = dir.join("recovery.enc");
        fs::write(&input, &bytes).unwrap();
        let error = decrypt_file(&input, "123", &options(&dir, ConflictStrategy::Rename), &mut |_, _| {})
            .unwrap_err();
        assert_eq!(error.kind(), "unsupportedRecovery");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn conflict_strategies_apply_to_restored_files() {
        let dir = temp_dir("conflict");
        let bytes = make_enc_file("123", &test_salt(9), 0x11, &[("dup.txt", b"first")]);
        let input = dir.join("dup.enc");
        fs::write(&input, &bytes).unwrap();
        let out_dir = dir.join("out");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("dup.txt"), b"already here").unwrap();

        // Rename: keep the existing file and add " (1)".
        let report = decrypt_file(&input, "123", &options(&out_dir, ConflictStrategy::Rename), &mut |_, _| {})
            .unwrap();
        assert_eq!(report.outputs, vec![out_dir.join("dup (1).txt")]);
        assert_eq!(fs::read(out_dir.join("dup.txt")).unwrap(), b"already here");

        // Error: refuse to overwrite.
        let error = decrypt_file(&input, "123", &options(&out_dir, ConflictStrategy::Error), &mut |_, _| {})
            .unwrap_err();
        assert_eq!(error.kind(), "outputExists");

        // Overwrite: replace the target.
        let report = decrypt_file(&input, "123", &options(&out_dir, ConflictStrategy::Overwrite), &mut |_, _| {})
            .unwrap();
        assert_eq!(report.outputs, vec![out_dir.join("dup.txt")]);
        assert_eq!(fs::read(out_dir.join("dup.txt")).unwrap(), b"first");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn inspect_and_validate_input() {
        let dir = temp_dir("inspect");
        let bytes = make_enc_file("123", &test_salt(10), 0x11, &[("a.txt", b"data")]);
        let enc = dir.join("ok.enc");
        fs::write(&enc, &bytes).unwrap();
        let header = inspect_file(&enc).unwrap();
        assert_eq!(header.version, FORMAT_VERSION);
        assert_eq!(header.algorithm, crate::format::ALGORITHM_AES_256);
        assert_eq!(header.custom_params, 0x11);
        assert_eq!(header.salt, test_salt(10));
        assert!(validate_input_path(&enc).is_ok());

        let txt = dir.join("notenc.txt");
        fs::write(&txt, b"data").unwrap();
        assert_eq!(validate_input_path(&txt).unwrap_err().kind(), "notEncExtension");

        let bogus = dir.join("bogus.enc");
        fs::write(&bogus, b"not paranoia at all").unwrap();
        assert_eq!(validate_input_path(&bogus).unwrap_err().kind(), "tooShort");

        let bad_magic = dir.join("badmagic.enc");
        let mut bad = bytes.clone();
        bad[0] = b'X';
        fs::write(&bad_magic, &bad).unwrap();
        assert_eq!(validate_input_path(&bad_magic).unwrap_err().kind(), "badMagic");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decrypt_to_archive_writes_inner_zip() {
        let dir = temp_dir("archive");
        let input = dir.join("sample.enc");
        let bytes = make_enc_file("pw", &test_salt(11), 0x00, &[("hello.txt", b"hello archive")]);
        fs::write(&input, &bytes).unwrap();
        let out_zip = dir.join("staging/inner.zip");
        let mut calls = 0usize;
        let report = decrypt_to_archive(&input, "pw", &out_zip, &mut |done, total| {
            assert!(total > 0);
            assert!(done <= total);
            calls += 1;
        })
        .expect("decrypt to archive");
        assert!(calls >= 1);
        assert_eq!(report.archive, out_zip);
        assert_eq!(report.archive_bytes, bytes.len() as u64 - 40 - 32 - 32);
        let stored = fs::read(&out_zip).unwrap();
        assert!(stored.starts_with(b"PK\x03\x04"), "output must be a bare ZIP, got {:?}", &stored[..4]);
        let mut archive = zip::ZipArchive::new(File::open(&out_zip).unwrap()).unwrap();
        let mut content = String::new();
        archive
            .by_name("hello.txt")
            .expect("inner entry")
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "hello archive");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decrypt_to_archive_removes_partial_output_on_wrong_password() {
        let dir = temp_dir("archive-wrongpw");
        let input = dir.join("sample.enc");
        fs::write(
            &input,
            make_enc_file("pw", &test_salt(12), 0x00, &[("a.txt", b"secret")]),
        )
        .unwrap();
        let out_zip = dir.join("partial.zip");
        let error = decrypt_to_archive(&input, "nope", &out_zip, &mut |_, _| {}).unwrap_err();
        assert_eq!(error.kind(), "wrongPassword");
        assert!(!out_zip.exists(), "partial archive must be removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decrypt_to_archive_rejects_tampered_mac_without_leaving_output() {
        let dir = temp_dir("archive-tamper");
        let input = dir.join("sample.enc");
        let mut bytes = make_enc_file("pw", &test_salt(13), 0x00, &[("a.txt", b"long enough payload")]);
        let offset = 40 + 32 + 2;
        bytes[offset] ^= 0x5A;
        fs::write(&input, &bytes).unwrap();
        let out_zip = dir.join("tampered.zip");
        let error = decrypt_to_archive(&input, "pw", &out_zip, &mut |_, _| {}).unwrap_err();
        assert_eq!(error.kind(), "integrity");
        assert!(!out_zip.exists(), "unauthenticated archive must be removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_password_accepts_correct_password_and_rejects_others() {
        let dir = temp_dir("verifypw");
        let input = dir.join("sample.enc");
        let bytes = make_enc_file("correct horse", &test_salt(14), 0x00, &[("a.txt", b"data")]);
        fs::write(&input, &bytes).unwrap();
        assert!(verify_password(&input, "correct horse").is_ok());
        assert_eq!(
            verify_password(&input, "wrong").unwrap_err().kind(),
            "wrongPassword"
        );

        let truncated = dir.join("short.enc");
        fs::write(&truncated, &bytes[..60]).unwrap();
        assert_eq!(
            verify_password(&truncated, "correct horse").unwrap_err().kind(),
            "tooShort"
        );

        let bad_magic = dir.join("bad.enc");
        let mut bad = bytes.clone();
        bad[0] = b'X';
        fs::write(&bad_magic, &bad).unwrap();
        assert_eq!(
            verify_password(&bad_magic, "correct horse").unwrap_err().kind(),
            "badMagic"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_password_matches_decrypt_to_archive_password_acceptance() {
        let dir = temp_dir("verifypw-consistent");
        let input = dir.join("sample.enc");
        fs::write(
            &input,
            make_enc_file("123", &test_salt(15), 0x00, &[("a.txt", b"data")]),
        )
        .unwrap();
        let out_zip = dir.join("inner.zip");
        assert_eq!(
            verify_password(&input, "123").is_ok(),
            decrypt_to_archive(&input, "123", &out_zip, &mut |_, _| {}).is_ok()
        );
        fs::remove_file(&out_zip).unwrap();
        assert_eq!(
            verify_password(&input, "456").is_err(),
            decrypt_to_archive(&input, "456", &out_zip, &mut |_, _| {}).is_err()
        );
        assert!(!out_zip.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decrypt_to_archive_with_keys_reuses_keys_and_respects_cancel() {
        let dir = temp_dir("with-keys");
        let input = dir.join("sample.enc");
        fs::write(
            &input,
            make_enc_file("pw", &test_salt(16), 0x00, &[("hello.txt", b"reused keys")]),
        )
        .unwrap();
        let keys = verify_password_keys(&input, "pw").expect("keys");

        let out_zip = dir.join("ok.zip");
        let cancel = AtomicBool::new(false);
        let report =
            decrypt_to_archive_with_keys(&input, &keys, &out_zip, Some(&cancel), &mut |_, _| {})
                .expect("decrypt with keys");
        assert!(report.archive_bytes > 0);
        assert!(out_zip.exists());

        let cancelled_zip = dir.join("cancelled.zip");
        cancel.store(true, Ordering::Relaxed);
        let err = decrypt_to_archive_with_keys(
            &input,
            &keys,
            &cancelled_zip,
            Some(&cancel),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(err.kind(), "cancelled");
        assert!(!cancelled_zip.exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
