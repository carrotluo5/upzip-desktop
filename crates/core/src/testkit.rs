//! Test-only helpers that build real Paranoia v4 AES-256 `.enc` files.
//!
//! This module is gated behind the `testkit` Cargo feature and is never compiled into a
//! shipped build: the product is a read-only decryptor. It exists so the test suites and the
//! fixture generator (`cargo run -p upzip-core --features testkit --example make_enc`)
//! can create authentic containers without the original Paranoia application.

use std::fs;
use std::io::{self, Cursor, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::crypto::{CtrStream, MacAccumulator};
use crate::format::{
    parse_argon2_params, ALGORITHM_AES_256, CHECKCODE_LEN, FORMAT_VERSION, MAGIC, MAC_LEN,
    SALT_LEN,
};
use crate::kdf;

/// 32-byte checkcode made only of `[a-z0-9]`, as required by the format.
pub const FIXTURE_CHECKCODE: &[u8; CHECKCODE_LEN] = b"0123456789abcdefghijklmnopqrstuv";

static SALT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh pseudo-random salt. Not cryptographically significant: fixtures only.
pub fn random_salt() -> [u8; SALT_LEN] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = SALT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!("paranoia-fixture-{}-{nanos}-{counter}", std::process::id());
    let digest = blake3::hash(seed.as_bytes());
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(digest.as_bytes());
    salt
}

/// Build a STORED ZIP archive in memory. STORED keeps the ciphertext deterministic for the
/// provided contents, which makes fixtures easy to reason about.
pub fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in entries {
        writer
            .start_file(*name, options)
            .expect("zip entry name is valid");
        writer.write_all(content).expect("write zip entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

/// Encrypt an already built inner ZIP archive into a full Paranoia v4 container.
///
/// * `zip` is the plaintext inner archive (the payload of the container),
/// * `custom_params` is the raw header byte (e.g. `0x00` for t=10, m=10240 KiB),
/// * returns `header || salt || ciphertext || MAC`.
pub fn encrypt_zip_bytes(zip: &[u8], password: &str, salt: &[u8; SALT_LEN], custom_params: u8) -> Vec<u8> {
    let params = parse_argon2_params(custom_params).expect("customParams must be in the accepted range");
    let keys = kdf::derive_keys(password, salt, &params).expect("derive keys");

    let mut header = [0u8; 8];
    header[..5].copy_from_slice(&MAGIC);
    header[5] = FORMAT_VERSION;
    header[6] = ALGORITHM_AES_256;
    header[7] = custom_params;

    let mut plaintext = Vec::with_capacity(CHECKCODE_LEN + zip.len());
    plaintext.extend_from_slice(FIXTURE_CHECKCODE);
    plaintext.extend_from_slice(zip);

    let mut ciphertext = plaintext;
    CtrStream::new(&keys.enc_key, &keys.nonce).apply(&mut ciphertext);

    let mut mac = MacAccumulator::new(&keys.auth_key);
    mac.update(&keys.nonce);
    mac.update(&header);
    mac.update(salt);
    mac.update(&ciphertext);
    let mac = mac.finalize();

    let mut out = Vec::with_capacity(header.len() + SALT_LEN + ciphertext.len() + MAC_LEN);
    out.extend_from_slice(&header);
    out.extend_from_slice(salt);
    out.extend_from_slice(&ciphertext);
    out.extend_from_slice(&mac);
    out
}

/// Build a container whose inner ZIP holds `entries`, then write it to `output`.
pub fn write_enc_entries(
    output: &Path,
    password: &str,
    custom_params: u8,
    entries: &[(&str, &[u8])],
) -> io::Result<()> {
    let zip = build_zip(entries);
    let salt = random_salt();
    let bytes = encrypt_zip_bytes(&zip, password, &salt, custom_params);
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(output, bytes)
}

/// Build a container that stores `input` as a single entry named after the input file, then
/// write it to `output`. This mirrors how Paranoia wraps a file: decrypting and extracting the
/// result yields the original file.
pub fn write_enc_file(input: &Path, output: &Path, password: &str, custom_params: u8) -> io::Result<()> {
    let name = input
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "input path has no valid UTF-8 file name"))?
        .to_string();
    let data = fs::read(input)?;
    write_enc_entries(output, password, custom_params, &[(name.as_str(), data.as_slice())])
}
