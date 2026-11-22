//! `AEAD_AES_256_GCM` resource decryption (callback notifications and platform
//! certificates).
//!
//! WeChat Pay encrypts sensitive resources with the 32-byte APIv3 key using
//! AES-256-GCM. The wire object carries `algorithm`, `ciphertext`, `nonce` and
//! `associated_data`; the base64 ciphertext contains the 16-byte authentication
//! tag appended to the ciphertext (the same layout produced by Java's
//! `Cipher.getInstance("AES/GCM/NoPadding")`), which is exactly what the
//! `aes-gcm` crate expects.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// The only algorithm WeChat Pay APIv3 currently supports.
pub const AEAD_ALGORITHM: &str = "AEAD_AES_256_GCM";

/// APIv3 key length in bytes (characters).
pub const API_V3_KEY_LEN: usize = 32;

/// Decrypt an `AEAD_AES_256_GCM` resource.
///
/// `nonce` is the 12-byte initialization vector as a UTF-8 string (WeChat Pay
/// sends 12 ASCII characters), `associated_data` is the additional
/// authenticated data (empty string when absent), and `ciphertext_base64` is the
/// base64 ciphertext with the trailing 16-byte tag.
pub fn decrypt_aead_aes_256_gcm(
    key: &[u8],
    nonce: &str,
    associated_data: &str,
    ciphertext_base64: &str,
) -> Result<Vec<u8>, String> {
    if key.len() != API_V3_KEY_LEN {
        return Err(format!(
            "APIv3 key must be exactly {API_V3_KEY_LEN} bytes, got {}",
            key.len()
        ));
    }
    let nonce_bytes = nonce.as_bytes();
    if nonce_bytes.len() != 12 {
        return Err(format!(
            "AEAD nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        ));
    }
    let ciphertext = STANDARD
        .decode(ciphertext_base64.trim())
        .map_err(|_| "ciphertext is not valid base64".to_string())?;
    if ciphertext.len() < 16 {
        return Err("ciphertext is shorter than the 16-byte authentication tag".to_string());
    }
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| "invalid AES-256 key length".to_string())?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: &ciphertext,
                aad: associated_data.as_bytes(),
            },
        )
        .map_err(|_| "AEAD_AES_256_GCM authentication failed".to_string())
}

/// Test-only counterpart used to build local fixtures with the same wire layout.
#[cfg(test)]
pub(crate) fn encrypt_aead_aes_256_gcm(
    key: &[u8],
    nonce: &str,
    associated_data: &str,
    plaintext: &[u8],
) -> String {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte test key");
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(nonce.as_bytes()),
            Payload {
                msg: plaintext,
                aad: associated_data.as_bytes(),
            },
        )
        .expect("test encryption");
    STANDARD.encode(ciphertext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_hex(value: &str) -> Vec<u8> {
        assert!(value.len().is_multiple_of(2));
        value
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn encode_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// NIST GCM validation-program vector: AES-256 with an all-zero key, an
    /// all-zero 96-bit IV, empty AAD and a 128-bit all-zero plaintext.
    ///
    /// The IV is encoded as 12 NUL characters because the adapter's entry point
    /// takes the nonce as the UTF-8 string WeChat Pay sends; NUL is valid UTF-8
    /// and maps to the 12 zero bytes of the vector. The wire ciphertext is
    /// `base64(ciphertext || tag)`, matching WeChat Pay's layout. Expected
    /// ciphertext+tag were cross-checked against an independent OpenSSL-backed
    /// implementation.
    #[test]
    fn decrypts_nist_aes_256_gcm_known_answer_vector() {
        let key = [0u8; 32];
        let nonce = "\u{0}".repeat(12);
        let expected = decode_hex(concat!(
            "cea7403d4d606b6e074ec5d3baf39d18",
            "d0d1c8a799996bf0265b98b5d48ab919"
        ));
        let wire = STANDARD.encode(&expected);
        let plaintext =
            decrypt_aead_aes_256_gcm(&key, &nonce, "", &wire).expect("known-answer decrypt");
        assert_eq!(plaintext, vec![0u8; 16]);
        // The crate's encrypt path produces the identical wire layout.
        let reencrypted = encrypt_aead_aes_256_gcm(&key, &nonce, "", &plaintext);
        assert_eq!(reencrypted, wire);
        assert_eq!(
            encode_hex(&STANDARD.decode(reencrypted).unwrap()),
            encode_hex(&expected)
        );
    }

    #[test]
    fn tampered_ciphertext_or_tag_is_rejected() {
        let key = [7u8; 32];
        let ciphertext = encrypt_aead_aes_256_gcm(&key, "abcdefghijkl", "certificate", b"payload");
        let mut bytes = STANDARD.decode(&ciphertext).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert!(decrypt_aead_aes_256_gcm(
            &key,
            "abcdefghijkl",
            "certificate",
            &STANDARD.encode(bytes)
        )
        .is_err());
        // Wrong associated data changes the tag input.
        assert!(decrypt_aead_aes_256_gcm(&key, "abcdefghijkl", "other", &ciphertext).is_err());
        // Wrong key.
        assert!(
            decrypt_aead_aes_256_gcm(&[8u8; 32], "abcdefghijkl", "certificate", &ciphertext)
                .is_err()
        );
    }

    #[test]
    fn rejects_invalid_key_nonce_and_ciphertext_shapes() {
        assert!(decrypt_aead_aes_256_gcm(&[0u8; 31], "abcdefghijkl", "", "AAAA").is_err());
        assert!(decrypt_aead_aes_256_gcm(&[0u8; 32], "short", "", "AAAA").is_err());
        assert!(decrypt_aead_aes_256_gcm(&[0u8; 32], "abcdefghijkl", "", "not base64!").is_err());
        assert!(decrypt_aead_aes_256_gcm(&[0u8; 32], "abcdefghijkl", "", "AAAA").is_err());
    }

    #[test]
    fn roundtrips_a_locally_encrypted_payload() {
        let key = crate::test_support::api_v3_key();
        let ciphertext = encrypt_aead_aes_256_gcm(&key, "61f9c719728a", "certificate", b"hello");
        let plaintext =
            decrypt_aead_aes_256_gcm(&key, "61f9c719728a", "certificate", &ciphertext).unwrap();
        assert_eq!(plaintext, b"hello");
    }
}
