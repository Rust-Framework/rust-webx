//! WeChat Pay platform certificates: download-response parsing, public-key
//! extraction and the rotation-aware serial cache.
//!
//! `GET /v3/certificates` returns a list of platform certificates, each PEM body
//! encrypted with the APIv3 key (`AEAD_AES_256_GCM`, associated data
//! `"certificate"`). The cache is keyed by the `serial_no` that WeChat sends in
//! the `Wechatpay-Serial` response/notification header. An unknown serial
//! triggers one refresh (rate-limited to [`CERTIFICATE_REFRESH_INTERVAL`]); if
//! the serial is still unknown afterwards, verification fails closed.
//!
//! Public-key extraction uses `x509-cert` for the certificate structure and
//! `der` for the SubjectPublicKeyInfo encoding, then `rsa` parses the SPKI. That
//! is the same object official SDKs verify against (`X509Certificate
//! .getPublicKey()`): the RSA key inside the certificate's
//! `subjectPublicKeyInfo`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use der::Encode;
use rsa::pkcs8::DecodePublicKey;
use rsa::PublicKey;
use serde_json::Value;
use x509_cert::der::DecodePem;
use x509_cert::Certificate;

use crate::crypto::{decrypt_aead_aes_256_gcm, AEAD_ALGORITHM, API_V3_KEY_LEN};

/// Minimum interval between two platform-certificate refreshes.
///
/// Official guidance is to refresh well below 12 hours; this bound protects the
/// endpoint from request-driven refresh storms when a serial stays unknown.
pub const CERTIFICATE_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Associated data WeChat Pay uses for encrypted platform certificates.
const PLATFORM_CERTIFICATE_AAD: &str = "certificate";

/// Serial number → platform public key cache for one merchant account.
#[derive(Default)]
pub(crate) struct CertificateCache {
    keys: RwLock<BTreeMap<String, Arc<PublicKey>>>,
    last_refresh: Mutex<Option<Instant>>,
}

impl CertificateCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Public key for `serial`, if cached.
    pub(crate) fn get(&self, serial: &str) -> Option<Arc<PublicKey>> {
        self.keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&normalize_serial(serial))
            .cloned()
    }

    /// Add/replace the key for `serial`.
    pub(crate) fn insert(&self, serial: &str, key: PublicKey) {
        self.keys
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(normalize_serial(serial), Arc::new(key));
    }

    /// Number of cached platform certificates.
    pub(crate) fn len(&self) -> usize {
        self.keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Elect the caller that performs the next refresh.
    ///
    /// Returns `false` while the current rate-limit window is still open: the
    /// caller must fail closed instead of hammering `GET /v3/certificates`.
    pub(crate) fn try_begin_refresh(&self) -> bool {
        let mut last_refresh = self
            .last_refresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(last) = *last_refresh {
            if last.elapsed() < CERTIFICATE_REFRESH_INTERVAL {
                return false;
            }
        }
        *last_refresh = Some(Instant::now());
        true
    }
}

/// WeChat Pay serial numbers are hexadecimal and compared case-insensitively.
pub(crate) fn normalize_serial(serial: &str) -> String {
    serial.trim().to_ascii_uppercase()
}

/// Extract the RSA public key from a PEM platform certificate.
pub(crate) fn parse_certificate_public_key(certificate_pem: &str) -> Result<PublicKey, String> {
    let certificate = Certificate::from_pem(certificate_pem.trim().as_bytes())
        .map_err(|error| format!("platform certificate is not valid PEM X.509: {error}"))?;
    let subject_public_key_info = certificate.tbs_certificate.subject_public_key_info;
    let spki_der = subject_public_key_info.to_der().map_err(|error| {
        format!("platform certificate has no valid SubjectPublicKeyInfo: {error}")
    })?;
    PublicKey::from_public_key_der(&spki_der)
        .map_err(|_| "platform certificate does not contain an RSA public key".to_string())
}

/// Decrypt and parse a `GET /v3/certificates` JSON response.
///
/// Returns `(serial, public key)` pairs in response order. The caller decides
/// which trust anchors it had before this download; this function only performs
/// the documented AEAD decryption and X.509 extraction.
pub(crate) fn parse_certificates_response(
    api_v3_key: &[u8],
    response: &Value,
) -> Result<Vec<(String, PublicKey)>, String> {
    if api_v3_key.len() != API_V3_KEY_LEN {
        return Err(format!("APIv3 key must be exactly {API_V3_KEY_LEN} bytes"));
    }
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "certificates response has no `data` array".to_string())?;
    if data.is_empty() {
        return Err("certificates response contains no platform certificates".to_string());
    }
    let mut certificates = Vec::with_capacity(data.len());
    for entry in data {
        let serial = entry
            .get("serial_no")
            .and_then(Value::as_str)
            .map(normalize_serial)
            .filter(|serial| !serial.is_empty())
            .ok_or_else(|| "platform certificate entry has no serial_no".to_string())?;
        let encrypted = entry
            .get("encrypt_certificate")
            .ok_or_else(|| format!("platform certificate `{serial}` has no encrypt_certificate"))?;
        let algorithm = encrypted
            .get("algorithm")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if algorithm != AEAD_ALGORITHM {
            return Err(format!(
                "platform certificate `{serial}` uses unsupported algorithm `{algorithm}`"
            ));
        }
        let nonce = encrypted
            .get("nonce")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("platform certificate `{serial}` has no nonce"))?;
        let associated_data = encrypted
            .get("associated_data")
            .and_then(Value::as_str)
            .unwrap_or(PLATFORM_CERTIFICATE_AAD);
        let ciphertext = encrypted
            .get("ciphertext")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("platform certificate `{serial}` has no ciphertext"))?;
        let pem = decrypt_aead_aes_256_gcm(api_v3_key, nonce, associated_data, ciphertext)
            .map_err(|error| {
                format!("platform certificate `{serial}` decryption failed: {error}")
            })?;
        let pem = String::from_utf8(pem)
            .map_err(|_| format!("platform certificate `{serial}` plaintext is not UTF-8 PEM"))?;
        let key = parse_certificate_public_key(&pem)
            .map_err(|error| format!("platform certificate `{serial}`: {error}"))?;
        certificates.push((serial, key));
    }
    Ok(certificates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn cache_roundtrips_serials_case_insensitively() {
        let cache = CertificateCache::new();
        assert_eq!(cache.len(), 0);
        cache.insert(
            "5157f09efdc096de15ebe81a47057a7232f1b8e1",
            test_support::key_pair().public.as_ref().clone(),
        );
        assert_eq!(cache.len(), 1);
        assert!(cache
            .get("5157F09EFDC096DE15EBE81A47057A7232F1B8E1")
            .is_some());
        assert!(cache
            .get(" 5157f09efdc096de15ebe81a47057a7232f1b8e1 ")
            .is_some());
        assert!(cache.get("unknown").is_none());
    }

    #[test]
    fn refresh_is_elected_once_per_rate_limit_window() {
        let cache = CertificateCache::new();
        assert!(cache.try_begin_refresh());
        assert!(!cache.try_begin_refresh());
    }

    #[test]
    fn rejects_invalid_certificate_pem() {
        assert!(parse_certificate_public_key("not a certificate").is_err());
        assert!(parse_certificate_public_key(
            "-----BEGIN CERTIFICATE-----\nbm90LWEtY2VydA==\n-----END CERTIFICATE-----"
        )
        .is_err());
    }

    #[test]
    fn extracts_the_same_rsa_key_as_the_certificate_subject_public_key_info() {
        use rsa::pkcs8::EncodePublicKey;

        let key = test_support::key_pair();
        let pem = test_support::certificate_pem(&key.public);
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
        let extracted = parse_certificate_public_key(&pem).unwrap();
        let expected = key.public.to_public_key_der().unwrap();
        let actual = extracted.to_public_key_der().unwrap();
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn decrypts_and_parses_a_certificate_response_fixture() {
        let key_pair = test_support::key_pair();
        let certificate_pem = test_support::certificate_pem(&key_pair.public);
        let serial = "5157F09EFDC096DE15EBE81A47057A7232F1B8E1";
        let api_v3_key = test_support::api_v3_key();
        let nonce = "61f9c719728a";
        let ciphertext = crate::crypto::encrypt_aead_aes_256_gcm(
            &api_v3_key,
            nonce,
            "certificate",
            certificate_pem.as_bytes(),
        );
        let response = serde_json::json!({
            "data": [{
                "serial_no": serial,
                "effective_time": "2018-06-08T10:34:56+08:00",
                "expire_time": "2023-06-08T10:34:56+08:00",
                "encrypt_certificate": {
                    "algorithm": "AEAD_AES_256_GCM",
                    "nonce": nonce,
                    "associated_data": "certificate",
                    "ciphertext": ciphertext,
                }
            }]
        });
        let parsed = parse_certificates_response(&api_v3_key, &response).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, serial);
        use rsa::pkcs8::EncodePublicKey;
        assert_eq!(
            parsed[0].1.to_public_key_der().unwrap().as_bytes(),
            key_pair.public.to_public_key_der().unwrap().as_bytes()
        );
    }

    #[test]
    fn certificate_response_errors_are_explicit() {
        let api_v3_key = test_support::api_v3_key();
        let certificate_pem = test_support::certificate_pem(&test_support::key_pair().public);
        let ciphertext = crate::crypto::encrypt_aead_aes_256_gcm(
            &api_v3_key,
            "61f9c719728a",
            "certificate",
            certificate_pem.as_bytes(),
        );
        let good_encrypt = serde_json::json!({
            "algorithm": "AEAD_AES_256_GCM",
            "nonce": "61f9c719728a",
            "associated_data": "certificate",
            "ciphertext": ciphertext,
        });
        assert!(parse_certificates_response(&api_v3_key, &serde_json::json!({})).is_err());
        assert!(
            parse_certificates_response(&api_v3_key, &serde_json::json!({ "data": [] })).is_err()
        );
        assert!(parse_certificates_response(
            &api_v3_key,
            &serde_json::json!({ "data": [{ "serial_no": "AA" }] })
        )
        .is_err());
        let bad_algorithm = serde_json::json!({
            "data": [{
                "serial_no": "AA",
                "encrypt_certificate": {
                    "algorithm": "AES_128_CBC",
                    "nonce": "61f9c719728a",
                    "associated_data": "certificate",
                    "ciphertext": ciphertext,
                }
            }]
        });
        assert!(parse_certificates_response(&api_v3_key, &bad_algorithm).is_err());
        let wrong_key = serde_json::json!({
            "data": [{
                "serial_no": "AA",
                "encrypt_certificate": good_encrypt,
            }]
        });
        assert!(parse_certificates_response(&[9u8; 32], &wrong_key).is_err());
        assert!(parse_certificates_response(&[9u8; 31], &wrong_key).is_err());
    }
}
