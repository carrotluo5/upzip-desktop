//! Cross-implementation KDF/header vectors produced by the independent Java oracle.
//!
//! The oracle (`verification/oracle/`) is a separate, non-Rust implementation of the
//! Paranoia v4 container built from the official Java sources; it emits
//! `verification/oracle/test-vectors.json`. This test replays the header/KDF stage for
//! both provided samples and asserts that the Rust implementation derives byte-identical
//! intermediates and subkeys. It needs no sample files (only the committed vectors).

use std::path::PathBuf;

use upzip_core::{
    derive_keys, derive_l0_password_hash, parse_argon2_params, parse_header,
};

fn vectors_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PARANOIA_ORACLE_VECTORS") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../verification/oracle/test-vectors.json")
}

fn hex_decode(text: &str) -> Vec<u8> {
    assert!(text.len() % 2 == 0, "odd hex length");
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[test]
fn rust_kdf_matches_java_oracle_vectors() {
    let path = vectors_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let vectors: serde_json::Value = serde_json::from_str(&raw).expect("vectors json");
    let samples = vectors["samples"].as_object().expect("samples object");
    assert!(!samples.is_empty());

    for (label, sample) in samples {
        let header_hex = sample["headerHex"].as_str().expect("headerHex");
        let salt_hex = sample["saltHex"].as_str().expect("saltHex");
        let mut prefix = hex_decode(header_hex);
        prefix.extend(hex_decode(salt_hex));
        assert_eq!(prefix.len(), 40);

        // Header parsing (including the Argon2 parameter decode) must match the oracle.
        let header = parse_header(&prefix).expect("parse header");
        assert_eq!(header.raw, hex_decode(header_hex).as_slice(), "{label} raw header");
        assert_eq!(hex_encode(&header.salt), salt_hex, "{label} salt");
        let params = parse_argon2_params(header.custom_params).expect("argon2 params");
        assert_eq!(
            params.iterations as u64,
            sample["argon2Params"]["t"].as_u64().unwrap(),
            "{label} t"
        );
        assert_eq!(
            params.memory_kib as u64,
            sample["argon2Params"]["mKiB"].as_u64().unwrap(),
            "{label} m"
        );
        assert_eq!(
            params.parallelism as u64,
            sample["argon2Params"]["p"].as_u64().unwrap(),
            "{label} p"
        );
        assert_eq!(
            params.output_len as u64,
            sample["argon2Params"]["outputLength"].as_u64().unwrap(),
            "{label} output length"
        );

        // Full KDF chain: l0 password hash -> Argon2id -> three HKDF-SHA3 subkeys.
        let l0 = derive_l0_password_hash("123");
        assert_eq!(
            hex_encode(&l0),
            sample["l0PWHashV3Hex"].as_str().expect("l0PWHashV3Hex"),
            "{label} l0PWHashV3"
        );
        let keys = derive_keys("123", &header.salt, &params).expect("derive keys");
        assert_eq!(
            hex_encode(&keys.enc_key),
            sample["encKeyHex"].as_str().expect("encKeyHex"),
            "{label} encKey"
        );
        assert_eq!(
            hex_encode(&keys.nonce),
            sample["nonceHex"].as_str().expect("nonceHex"),
            "{label} nonce"
        );
        assert_eq!(
            hex_encode(&keys.auth_key),
            sample["authKeyHex"].as_str().expect("authKeyHex"),
            "{label} authKey"
        );
    }
}
