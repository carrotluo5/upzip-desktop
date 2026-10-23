//! AES-256-CTR stream decryption and BLAKE3 keyed MAC accumulation.

use aes::Aes256;
use blake3::Hasher;
use cipher::{KeyIvInit, StreamCipher};
use ctr::Ctr128BE;

type Aes256Ctr = Ctr128BE<Aes256>;

/// Streaming AES-256-CTR transform (encryption and decryption are the same XOR operation).
pub struct CtrStream {
    cipher: Aes256Ctr,
}

impl CtrStream {
    pub fn new(key: &[u8; 32], nonce: &[u8; 16]) -> Self {
        Self {
            cipher: Aes256Ctr::new(key.into(), nonce.into()),
        }
    }

    /// Apply the keystream in place. May be called repeatedly; the stream position is preserved.
    pub fn apply(&mut self, data: &mut [u8]) {
        self.cipher.apply_keystream(data);
    }
}

/// Incremental keyed BLAKE3 hasher used for the file MAC.
pub struct MacAccumulator {
    hasher: Hasher,
}

impl MacAccumulator {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            hasher: Hasher::new_keyed(key),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    pub fn finalize(self) -> [u8; 32] {
        *self.hasher.finalize().as_bytes()
    }
}

/// Compare two byte strings without leaking timing information.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctr_matches_known_keystream() {
        // Round trip through two calls must equal a single call.
        let key = [7u8; 32];
        let nonce = [9u8; 16];
        let plain: Vec<u8> = (0..97u8).collect();

        let mut a = plain.clone();
        CtrStream::new(&key, &nonce).apply(&mut a);

        let mut b = plain.clone();
        let mut stream = CtrStream::new(&key, &nonce);
        stream.apply(&mut b[..33]);
        stream.apply(&mut b[33..]);
        assert_eq!(a, b);

        let mut c = a.clone();
        CtrStream::new(&key, &nonce).apply(&mut c);
        assert_eq!(c, plain);
    }

    #[test]
    fn mac_accumulator_matches_one_shot() {
        let key = [3u8; 32];
        let data: Vec<u8> = (0..100u8).collect();
        let one_shot = Hasher::new_keyed(&key);
        let expected = {
            let mut h = one_shot;
            h.update(&data);
            *h.finalize().as_bytes()
        };
        let mut mac = MacAccumulator::new(&key);
        mac.update(&data[..10]);
        mac.update(&data[10..]);
        assert_eq!(mac.finalize(), expected);
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
