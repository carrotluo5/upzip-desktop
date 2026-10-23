//! Password-to-key derivation for Paranoia File Encryption format version 4.
//!
//! Pipeline (matching the official Java implementation):
//! 1. password -> code-point normalized bytes (printable ASCII kept, others as decimal code points)
//! 2. l0PWHash = HKDF-Skein1024/1024(passwordBytes, "memorySalt", "memoryInfo", 256)
//! 3. argonOut = Argon2id(v1.3, l0PWHash, salt, t, m, p=4, 256 bytes)
//! 4. encKey = HKDF-SHA3-512(argonOut, "encKeySalt", "encKeyInfo", 32)
//!    nonce  = HKDF-SHA3-512(argonOut, "nonceSalt",  "nonceInfo",  16)
//!    authKey= HKDF-SHA3-512(argonOut, "authKeySalt","authKeyInfo",32)

use argon2::{Algorithm, Argon2, Params, Version};
use digest::{Digest, FixedOutput, Update};
use sha3::Sha3_512;
use skein::Skein1024_1024;

use crate::error::DecryptError;
use crate::format::Argon2Params;

/// Derived key material for one encrypted file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    pub enc_key: [u8; 32],
    pub nonce: [u8; 16],
    pub auth_key: [u8; 32],
}

/// Convert a Rust string into the byte representation used by Paranoia:
/// printable ASCII (0x20..=0x7E) is kept verbatim; every other character is replaced by the
/// decimal digits of its Unicode code point. The result is UTF-8 encoded (ASCII here).
pub fn password_to_bytes(password: &str) -> Vec<u8> {
    // Faithful port of Encryptor.convertToCodePoints + Helpers.toBytes(pw, false) (UTF-8):
    // iterate the UTF-16 code units exactly like Java's char[] loop. Printable ASCII stays a
    // single byte; every other code point becomes the decimal digits of its value.
    let units: Vec<u16> = password.encode_utf16().collect();
    let mut out = Vec::with_capacity(units.len());
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        let mut code_point = unit as u32;
        if (0xD800..=0xDBFF).contains(&unit)
            && index + 1 < units.len()
            && (0xDC00..=0xDFFF).contains(&units[index + 1])
        {
            let high = (unit as u32) - 0xD800;
            let low = (units[index + 1] as u32) - 0xDC00;
            code_point = 0x10000 + (high << 10) + low;
        }
        if (32..=126).contains(&code_point) {
            out.push(code_point as u8);
        } else {
            out.extend_from_slice(code_point.to_string().as_bytes());
        }
        index += 1;
    }
    out
}

trait Hmac {
    fn mac(&self, key: &[u8], parts: &[&[u8]]) -> Vec<u8>;
}

struct SkeinHmac;
struct Sha3Hmac;

impl Hmac for SkeinHmac {
    fn mac(&self, key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
        const BLOCK: usize = 128;
        let mut k = [0u8; BLOCK];
        if key.len() > BLOCK {
            let digest = <Skein1024_1024 as Digest>::finalize({
                let mut h = Skein1024_1024::default();
                Update::update(&mut h, key);
                h
            });
            k.copy_from_slice(&digest);
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut ipad = [0x36u8; BLOCK];
        let mut opad = [0x5cu8; BLOCK];
        for i in 0..BLOCK {
            ipad[i] ^= k[i];
            opad[i] ^= k[i];
        }
        let mut inner = Skein1024_1024::default();
        Update::update(&mut inner, &ipad);
        for part in parts {
            Update::update(&mut inner, part);
        }
        let inner_out = Digest::finalize(inner);
        let mut outer = Skein1024_1024::default();
        Update::update(&mut outer, &opad);
        Update::update(&mut outer, &inner_out);
        Digest::finalize(outer).to_vec()
    }
}

impl Hmac for Sha3Hmac {
    fn mac(&self, key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
        const BLOCK: usize = 72;
        let mut k = [0u8; BLOCK];
        if key.len() > BLOCK {
            let digest = Sha3_512::digest(key);
            k.copy_from_slice(&digest);
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut ipad = [0x36u8; BLOCK];
        let mut opad = [0x5cu8; BLOCK];
        for i in 0..BLOCK {
            ipad[i] ^= k[i];
            opad[i] ^= k[i];
        }
        let mut inner = Sha3_512::default();
        Update::update(&mut inner, &ipad);
        for part in parts {
            Update::update(&mut inner, part);
        }
        let inner_out = FixedOutput::finalize_fixed(inner);
        let mut outer = Sha3_512::default();
        Update::update(&mut outer, &opad);
        Update::update(&mut outer, &inner_out);
        FixedOutput::finalize_fixed(outer).to_vec()
    }
}

fn hkdf<H: Hmac>(hmac: &H, ikm: &[u8], salt: &[u8], info: &[u8], out_len: usize) -> Vec<u8> {
    let prk = hmac.mac(salt, &[ikm]);
    let mut okm = Vec::with_capacity(out_len);
    let mut block: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while okm.len() < out_len {
        block = hmac.mac(&prk, &[&block, info, &[counter]]);
        okm.extend_from_slice(&block);
        counter = counter.wrapping_add(1);
    }
    okm.truncate(out_len);
    okm
}

fn hkdf_skein(ikm: &[u8], salt: &[u8], info: &[u8], out_len: usize) -> Vec<u8> {
    hkdf(&SkeinHmac, ikm, salt, info, out_len)
}

fn hkdf_sha3(ikm: &[u8], salt: &[u8], info: &[u8], out_len: usize) -> Vec<u8> {
    hkdf(&Sha3Hmac, ikm, salt, info, out_len)
}

/// Step 2 of the KDF: 256-byte HKDF-Skein1024/1024 password hash.
pub fn derive_l0_password_hash(password: &str) -> Vec<u8> {
    let password_bytes = password_to_bytes(password);
    hkdf_skein(&password_bytes, b"memorySalt", b"memoryInfo", 256)
}

/// Derive the AES key, CTR nonce and BLAKE3 authentication key for one file.
pub fn derive_keys(
    password: &str,
    salt: &[u8; 32],
    params: &Argon2Params,
) -> Result<Keys, DecryptError> {
    let l0 = derive_l0_password_hash(password);
    let argon_params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(params.output_len),
    )
    .map_err(|e| DecryptError::Crypto(format!("invalid Argon2 parameters: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut argon_out = vec![0u8; params.output_len];
    argon
        .hash_password_into(&l0, salt, &mut argon_out)
        .map_err(|e| DecryptError::Crypto(format!("Argon2 failure: {e}")))?;

    let enc_key = hkdf_sha3(&argon_out, b"encKeySalt", b"encKeyInfo", 32);
    let nonce = hkdf_sha3(&argon_out, b"nonceSalt", b"nonceInfo", 16);
    let auth_key = hkdf_sha3(&argon_out, b"authKeySalt", b"authKeyInfo", 32);

    let mut keys = Keys {
        enc_key: [0u8; 32],
        nonce: [0u8; 16],
        auth_key: [0u8; 32],
    };
    keys.enc_key.copy_from_slice(&enc_key);
    keys.nonce.copy_from_slice(&nonce);
    keys.auth_key.copy_from_slice(&auth_key);
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::parse_argon2_params;

    // Known-good vectors computed by the official Paranoia Java implementation
    // (see verification/oracle) for sample 3138.7z.001.enc, password "123".
    const SALT_001: [u8; 32] = [
        0x7d, 0xbe, 0x8c, 0x5f, 0x3f, 0x61, 0x44, 0xc9, 0x99, 0x36, 0x87, 0xd7, 0xe6, 0x88, 0x20,
        0x59, 0x7b, 0x68, 0xc4, 0xb0, 0x43, 0x79, 0xeb, 0x40, 0x52, 0x08, 0x41, 0xa9, 0x4f, 0x14,
        0x4f, 0xf0,
    ];
    const ENC_KEY_001: [u8; 32] = [
        0xe3, 0x5c, 0xac, 0x8a, 0x57, 0xd2, 0x03, 0x2c, 0xda, 0x80, 0x7a, 0xc5, 0x0e, 0xce, 0x8f,
        0x98, 0xe5, 0x34, 0x06, 0x69, 0x16, 0xad, 0x5d, 0xab, 0xd6, 0x06, 0x52, 0x93, 0x47, 0x55,
        0x10, 0x67,
    ];
    const AUTH_KEY_001: [u8; 32] = [
        0xb9, 0x23, 0x50, 0x44, 0x94, 0x79, 0x39, 0x46, 0xdd, 0x14, 0xc9, 0x4e, 0x4d, 0x42, 0x29,
        0x2c, 0xda, 0x70, 0x73, 0xfd, 0x92, 0x7e, 0xf6, 0x48, 0xdd, 0xa3, 0x2c, 0x34, 0x56, 0x4c,
        0x59, 0xe8,
    ];
    const NONCE_001: [u8; 16] = [
        0xcb, 0xfe, 0x37, 0x65, 0xd8, 0x4f, 0xf2, 0x16, 0xc3, 0xd3, 0x29, 0x5f, 0x6c, 0x71, 0xbd,
        0x36,
    ];

    #[test]
    fn password_code_point_conversion() {
        assert_eq!(password_to_bytes("123"), b"123");
        assert_eq!(password_to_bytes("ab"), b"ab");
        // Java reference: characters outside printable ASCII are replaced by the decimal
        // digits of their code point (é = U+00E9 -> "233").
        assert_eq!(password_to_bytes("a\u{e9}"), b"a233");
        // Java's char[] loop yields the supplementary code point and then the trailing lone
        // surrogate, matching Character.codePointAt() semantics.
        assert_eq!(password_to_bytes("\u{1F600}"), b"12851256832");
    }

    #[test]
    fn known_derived_keys_for_sample() {
        // Full KDF (Skein + Argon2id + SHA3 HKDF) must reproduce the reference values.
        let params = parse_argon2_params(0x11).unwrap();
        let keys = derive_keys("123", &SALT_001, &params).expect("derive");
        assert_eq!(keys.enc_key, ENC_KEY_001);
        assert_eq!(keys.auth_key, AUTH_KEY_001);
        assert_eq!(keys.nonce, NONCE_001);
    }

    #[test]
    fn wrong_password_produces_different_keys() {
        let params = parse_argon2_params(0x11).unwrap();
        let a = derive_keys("123", &SALT_001, &params).unwrap();
        let b = derive_keys("124", &SALT_001, &params).unwrap();
        assert_ne!(a.enc_key, b.enc_key);
        assert_ne!(a.auth_key, b.auth_key);
        assert_ne!(a.nonce, b.nonce);
    }

    #[test]
    fn l0_hash_is_deterministic_and_sized() {
        let a = derive_l0_password_hash("123");
        let b = derive_l0_password_hash("123");
        assert_eq!(a.len(), 256);
        assert_eq!(a, b);
        assert_ne!(a, derive_l0_password_hash("1234"));
    }
}
