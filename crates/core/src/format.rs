//! Parsing of the fixed Paranoia "SSEFE" v4 file header.

use crate::error::DecryptError;

pub const MAGIC: [u8; 5] = *b"SSEFE";
pub const FORMAT_VERSION: u8 = 4;
pub const ALGORITHM_AES_256: u8 = 0x00;
pub const HEADER_LEN: usize = 8;
pub const SALT_LEN: usize = 32;
pub const CHECKCODE_LEN: usize = 32;
pub const MAC_LEN: usize = 32;
/// Magic of the optional Paranoia recovery container, stored just before the MAC.
pub const RECOVERY_MAGIC: [u8; 8] = *b"PFTEREC1";

/// Parsed 8-byte header plus the 32-byte salt that follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub algorithm: u8,
    pub custom_params: u8,
    pub salt: [u8; SALT_LEN],
    pub raw: [u8; HEADER_LEN],
}

/// Argon2id parameters decoded from the header `customParams` byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argon2Params {
    pub iterations: u32,
    pub memory_kib: u32,
    pub parallelism: u32,
    pub output_len: usize,
}

impl Argon2Params {
    /// Encode back to the `customParams` byte. Used by tests only.
    #[cfg(test)]
    pub fn to_custom_params(&self) -> u8 {
        let mut tm: u8 = 0;
        let mut t = 10u32;
        while t < self.iterations && tm < 15 {
            t *= 2;
            tm += 1;
        }
        let mut mm: u8 = 0;
        let mut m = 10240u32;
        while m < self.memory_kib && mm < 15 {
            m *= 2;
            mm += 1;
        }
        (mm << 4) | (tm & 0x0f)
    }
}

/// Parse the 8-byte header and 32-byte salt. Validates magic, version, algorithm and KDF params.
///
/// `bytes` must contain at least the first `HEADER_LEN + SALT_LEN` (40) bytes of the file.
pub fn parse_header(bytes: &[u8]) -> Result<Header, DecryptError> {
    if bytes.len() < HEADER_LEN + SALT_LEN {
        return Err(DecryptError::TooShort);
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(DecryptError::BadMagic);
    }
    let version = bytes[5];
    if version != FORMAT_VERSION {
        return Err(DecryptError::UnsupportedVersion(version));
    }
    let algorithm = bytes[6];
    if algorithm != ALGORITHM_AES_256 {
        return Err(DecryptError::UnsupportedAlgorithm(algorithm));
    }
    let custom_params = bytes[7];
    // Validate that the KDF parameters are representable.
    parse_argon2_params(custom_params)?;

    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[HEADER_LEN..HEADER_LEN + SALT_LEN]);
    let mut raw = [0u8; HEADER_LEN];
    raw.copy_from_slice(&bytes[..HEADER_LEN]);
    Ok(Header {
        version,
        algorithm,
        custom_params,
        salt,
        raw,
    })
}

/// Decode the Argon2id parameters from the header `customParams` byte.
///
/// Encoding (Paranoia File Encryption format version 3+): low nibble = iteration exponent,
/// high nibble = memory exponent, parallelism is fixed to 4 for file encryption.
pub fn parse_argon2_params(custom_params: u8) -> Result<Argon2Params, DecryptError> {
    let tm = custom_params & 0x0f;
    let mm = (custom_params >> 4) & 0x0f;
    if tm > 6 || mm > 6 {
        return Err(DecryptError::InvalidCustomParams(custom_params));
    }
    let iterations = 10u32
        .checked_mul(2u32.saturating_pow(tm as u32))
        .ok_or(DecryptError::InvalidCustomParams(custom_params))?;
    let memory_kib = 10240u32
        .checked_mul(2u32.saturating_pow(mm as u32))
        .ok_or(DecryptError::InvalidCustomParams(custom_params))?;
    Ok(Argon2Params {
        iterations,
        memory_kib,
        parallelism: 4,
        output_len: 256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_bytes(version: u8, algorithm: u8, custom_params: u8) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&MAGIC);
        v.push(version);
        v.push(algorithm);
        v.push(custom_params);
        v.extend_from_slice(&[0xABu8; SALT_LEN]);
        v
    }

    #[test]
    fn parses_valid_header() {
        let bytes = header_bytes(FORMAT_VERSION, ALGORITHM_AES_256, 0x11);
        let header = parse_header(&bytes).expect("valid header");
        assert_eq!(header.version, 4);
        assert_eq!(header.algorithm, 0);
        assert_eq!(header.custom_params, 0x11);
        assert_eq!(header.raw, [b'S', b'S', b'E', b'F', b'E', 4, 0, 0x11]);
        assert_eq!(header.salt, [0xABu8; SALT_LEN]);
        let params = parse_argon2_params(header.custom_params).expect("params");
        assert_eq!(params.iterations, 20);
        assert_eq!(params.memory_kib, 20480);
        assert_eq!(params.parallelism, 4);
        assert_eq!(params.output_len, 256);
    }

    #[test]
    fn rejects_short_input() {
        let bytes = header_bytes(FORMAT_VERSION, ALGORITHM_AES_256, 0x11);
        assert!(matches!(
            parse_header(&bytes[..39]),
            Err(DecryptError::TooShort)
        ));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = header_bytes(FORMAT_VERSION, ALGORITHM_AES_256, 0x11);
        bytes[0] = b'X';
        assert!(matches!(parse_header(&bytes), Err(DecryptError::BadMagic)));
    }

    #[test]
    fn rejects_unsupported_version() {
        let bytes = header_bytes(3, ALGORITHM_AES_256, 0x11);
        assert!(matches!(
            parse_header(&bytes),
            Err(DecryptError::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn rejects_non_aes256_algorithm() {
        let bytes = header_bytes(FORMAT_VERSION, 0x02, 0x11);
        assert!(matches!(
            parse_header(&bytes),
            Err(DecryptError::UnsupportedAlgorithm(0x02))
        ));
    }

    #[test]
    fn rejects_invalid_custom_params() {
        let bytes = header_bytes(FORMAT_VERSION, ALGORITHM_AES_256, 0xFF);
        assert!(matches!(
            parse_header(&bytes),
            Err(DecryptError::InvalidCustomParams(0xFF))
        ));
    }

    #[test]
    fn custom_params_round_trip() {
        for cp in [0x00u8, 0x01, 0x10, 0x11, 0x22, 0x33] {
            let params = parse_argon2_params(cp).expect("valid params");
            assert_eq!(params.to_custom_params(), cp, "round trip for {cp:#04x}");
        }
    }
}
