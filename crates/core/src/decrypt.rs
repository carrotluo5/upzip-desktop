//! End-to-end decryption of a single Paranoia v4 AES-256 \`.enc\` file.
//!
//! Pipeline (all in memory-bounded streaming form):
//! 1. parse and validate the plaintext 8-byte header + 32-byte salt,
//! 2. detect the optional PFTEREC1 recovery container and reject it,
//! 3. derive the keys (Argon2id + HKDF),
//! 4. decrypt AES-256-CTR checkcode + inner ZIP into a temporary file, validating the
//!    password checkcode on the first block,
//! 5. verify the keyed BLAKE3 MAC over \`nonce || header || salt || ciphertext\`,
//! 6. extract the inner ZIP into the output directory (temp file + rename per entry).
//!
//! Any error path removes the temporary files, so no partial output is ever left behind.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

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

/// Inspect the plaintext header of an \`.enc\` file without decrypting it.
pub fn inspect_file(path: &Path) -> Result<Header, DecryptError> {
    let len = fs::metadata(path)?.len();
    if len < PREFIX_LEN {
        return Err(DecryptError::TooShort);
    }
    let mut prefix = [0u8; HEADER_LEN + SALT_LEN];
    File::open(path)?.read_exact(&mut prefix)?;
    parse_header(&prefix)
}

/// Validate a file before adding it to the batch list: \`.enc\` extension, regular file,
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

/// Decrypt one file. \`progress\` receives (processed ciphertext bytes, total ciphertext bytes).
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
    let keys = kdf::derive_keys(password, &header.salt, &params)?;

    fs::create_dir_all(&opts.output_dir)?;
    let temp_zip = util::unique_temp_path(&opts.output_dir, ".paranoia-enc-decrypt-");
    let temp_guard = util::TempGuard::new(temp_zip.clone());

    file.seek(SeekFrom::Start(PREFIX_LEN))?;
    let mut reader = BufReader::with_capacity(CHUNK_SIZE, file.take(ciphertext_len));
    let mut writer = BufWriter::with_capacity(CHUNK_SIZE, File::create(&temp_zip)?);

    let mut mac = MacAccumulator::new(&keys.auth_key);
    mac.update(&keys.nonce);
    mac.update(&header.raw);
    mac.update(&header.salt);
    let mut cipher = CtrStream::new(&keys.enc_key, &keys.nonce);

    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut processed: u64 = 0;
    let mut first_chunk = true;
    while processed < ciphertext_len {
        let want = std::cmp::min(CHUNK_SIZE as u64, ciphertext_len - processed) as usize;
        reader.read_exact(&mut buffer[..want])?;
        mac.update(&buffer[..want]);
        cipher.apply(&mut buffer[..want]);
        if first_chunk {
            // The first 32 plaintext bytes must be the [a-z0-9] checkcode.
            if !util::is_valid_checkcode(&buffer[..CHECKCODE_LEN]) {
                return Err(DecryptError::WrongPassword);
            }
            first_chunk = false;
        }
        writer.write_all(&buffer[..want])?;
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
            "paranoia-enc-test-decrypt-{tag}-{}-{nanos}",
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
            !name.starts_with(".paranoia-enc-")
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
}
