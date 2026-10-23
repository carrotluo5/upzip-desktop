//! Test-only helpers that build real Paranoia v4 AES-256 \`.enc\` files in memory.

use std::io::{Cursor, Write};

use crate::crypto::{CtrStream, MacAccumulator};
use crate::format::{
    parse_argon2_params, ALGORITHM_AES_256, FORMAT_VERSION, MAGIC, SALT_LEN,
};
use crate::kdf;

/// 32-byte password checkcode used by the reference implementation.
pub(crate) const TEST_CHECKCODE: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";

fn build_zip(entries: &[(&str, &[u8])], compression: zip::CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(compression);
    for (name, content) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Build an encrypted file with one inner ZIP containing \`entries\`.
pub(crate) fn make_enc_file(
    password: &str,
    salt: &[u8; SALT_LEN],
    custom_params: u8,
    entries: &[(&str, &[u8])],
) -> Vec<u8> {
    make_enc_file_with_zip(
        password,
        salt,
        custom_params,
        build_zip(entries, zip::CompressionMethod::Deflated),
    )
}

pub(crate) fn make_enc_file_stored(
    password: &str,
    salt: &[u8; SALT_LEN],
    custom_params: u8,
    name: &str,
    content: &[u8],
) -> Vec<u8> {
    make_enc_file_with_zip(
        password,
        salt,
        custom_params,
        build_zip(&[(name, content)], zip::CompressionMethod::Stored),
    )
}

/// Build an encrypted file from an already constructed inner ZIP archive.
pub(crate) fn make_enc_file_with_zip(
    password: &str,
    salt: &[u8; SALT_LEN],
    custom_params: u8,
    zip: Vec<u8>,
) -> Vec<u8> {
    let params = parse_argon2_params(custom_params).expect("valid custom params");
    let keys = kdf::derive_keys(password, salt, &params).expect("derive keys");

    let mut header = [0u8; 8];
    header[..5].copy_from_slice(&MAGIC);
    header[5] = FORMAT_VERSION;
    header[6] = ALGORITHM_AES_256;
    header[7] = custom_params;

    let mut plaintext = Vec::with_capacity(TEST_CHECKCODE.len() + zip.len());
    plaintext.extend_from_slice(TEST_CHECKCODE);
    plaintext.extend_from_slice(&zip);

    let mut ciphertext = plaintext;
    CtrStream::new(&keys.enc_key, &keys.nonce).apply(&mut ciphertext);

    let mut mac = MacAccumulator::new(&keys.auth_key);
    mac.update(&keys.nonce);
    mac.update(&header);
    mac.update(salt);
    mac.update(&ciphertext);
    let mac = mac.finalize();

    let mut out = Vec::with_capacity(8 + SALT_LEN + ciphertext.len() + 32);
    out.extend_from_slice(&header);
    out.extend_from_slice(salt);
    out.extend_from_slice(&ciphertext);
    out.extend_from_slice(&mac);
    out
}

/// Fixed random-ish salt for deterministic tests.
pub(crate) fn test_salt(tag: u8) -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    for (index, byte) in salt.iter_mut().enumerate() {
        *byte = tag.wrapping_add(index as u8);
    }
    salt
}

/// Placeholder so unused helpers do not warn when a test binary only uses some of them.
#[allow(dead_code)]
pub(crate) fn _keep_helpers() {
    let _ = make_enc_file_stored;
}
