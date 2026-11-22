//! Alipay RSA2 signing, verification and X.509 certificate helpers.
//!
//! Behaviour follows the official Alipay SDKs:
//!
//! * **Request signing** — every request parameter except `sign` participates
//!   (including `sign_type`), sorted by key, `k=v` joined with `&`, values used
//!   exactly as sent (`AlipaySignature.getSignatureContent`).
//! * **Async-notification verification** — `sign` **and** `sign_type` are
//!   removed before the same canonicalization
//!   (`AlipaySignature.getSignCheckContentV1`).
//! * **RSA2** — PKCS#1 v1.5 with SHA-256, signatures base64 (standard
//!   alphabet).
//! * **Keys** — Alipay consoles often show a bare base64 key without PEM armor;
//!   both PKCS#8 (`PRIVATE KEY`/`PUBLIC KEY`) and PKCS#1 (`RSA PRIVATE
//!   KEY`/`RSA PUBLIC KEY`) PEMs are accepted and raw base64 is wrapped.
//! * **Certificate SN** — the official value is the lowercase, zero-padded
//!   32-character hex MD5 of `issuer distinguished name + decimal serial
//!   number` (`AntCertificationUtil.getCertSN`), **not** of the certificate DER.
//!   The issuer name is rendered with the RFC 4514 rules Java's
//!   `X500Principal.getName()` uses.
//!
//! RSA verification itself is delegated to the audited [`rsa`] crate: it is a
//! public-key operation over public data, so there is no secret-byte comparison
//! to make constant time. Untrusted *identifier* equality checks that gate
//! trust (notification `app_id`, `sign_type`) use
//! [`webx_core::security::constant_time_eq`].

use base64::Engine as _;
use md5::{Digest as _, Md5};
use rsa::pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::sha2::Sha256;
use rsa::{PrivateKey, PublicKey};
use webx_core::security::constant_time_eq;
use x509_cert::Certificate;

/// Standard base64 engine used by Alipay for signatures.
const BASE64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Whether a `sign_type` value is the RSA2 scheme this adapter implements.
pub(crate) fn sign_type_is_rsa2(value: &str) -> bool {
    constant_time_eq(value.as_bytes(), b"RSA2")
}

/// Canonical signature content for an outgoing request.
///
/// Keeps every non-empty parameter except `sign` (so `sign_type` participates,
/// matching the official SDK) and sorts by key.
pub(crate) fn request_sign_content(params: &[(String, String)]) -> String {
    sign_content(params, false)
}

/// Canonical signature content for an async notification.
///
/// Removes `sign` and `sign_type`, matching `AlipaySignature.getSignCheckContentV1`.
pub(crate) fn notify_sign_content(params: &[(String, String)]) -> String {
    sign_content(params, true)
}

fn sign_content(params: &[(String, String)], exclude_sign_type: bool) -> String {
    let mut items: Vec<(&str, &str)> = params
        .iter()
        .filter(|(key, value)| {
            !key.is_empty()
                && !value.is_empty()
                && key != "sign"
                && !(exclude_sign_type && key == "sign_type")
        })
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    items.sort_by(|left, right| left.0.cmp(right.0));
    let mut content = String::new();
    for (index, (key, value)) in items.iter().enumerate() {
        if index > 0 {
            content.push('&');
        }
        content.push_str(key);
        content.push('=');
        content.push_str(value);
    }
    content
}

/// Sign `content` with SHA256withRSA (RSA2), returning standard base64.
pub(crate) fn rsa2_sign(content: &str, private_key: &PrivateKey) -> Result<String, String> {
    let signature = private_key
        .pkcs1v15_sign::<Sha256>(content.as_bytes())
        .map_err(|error| format!("RSA2 signing failed: {error}"))?;
    Ok(BASE64.encode(signature))
}

/// Verify a base64 RSA2 signature over `content`.
pub(crate) fn rsa2_verify(
    content: &str,
    signature: &str,
    public_key: &PublicKey,
) -> Result<(), &'static str> {
    let signature = signature.trim();
    if signature.is_empty() {
        return Err("missing signature");
    }
    let bytes = BASE64
        .decode(signature.as_bytes())
        .map_err(|_| "signature is not valid base64")?;
    public_key
        .pkcs1v15_verify::<Sha256>(content.as_bytes(), &bytes)
        .map_err(|_| "signature mismatch")
}

/// Parse an application private key from PEM (PKCS#8 or PKCS#1) or raw base64.
pub(crate) fn parse_private_key(value: &str) -> Result<PrivateKey, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("private key must not be empty".to_string());
    }
    if value.contains("BEGIN") {
        return PrivateKey::from_pkcs8_pem(value)
            .or_else(|_| PrivateKey::from_pkcs1_pem(value))
            .map_err(|error| format!("cannot parse application private key: {error}"));
    }
    PrivateKey::from_pkcs8_pem(&normalize_pem(value, "PRIVATE KEY"))
        .or_else(|_| PrivateKey::from_pkcs1_pem(&normalize_pem(value, "RSA PRIVATE KEY")))
        .map_err(|error| format!("cannot parse application private key: {error}"))
}

/// Parse an Alipay public key from PEM (SubjectPublicKeyInfo or PKCS#1) or raw
/// base64.
pub(crate) fn parse_public_key(value: &str) -> Result<PublicKey, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("alipay public key must not be empty".to_string());
    }
    if value.contains("BEGIN") {
        return PublicKey::from_public_key_pem(value)
            .or_else(|_| PublicKey::from_pkcs1_pem(value))
            .map_err(|error| format!("cannot parse alipay public key: {error}"));
    }
    PublicKey::from_public_key_pem(&normalize_pem(value, "PUBLIC KEY"))
        .or_else(|_| PublicKey::from_pkcs1_pem(&normalize_pem(value, "RSA PUBLIC KEY")))
        .map_err(|error| format!("cannot parse alipay public key: {error}"))
}

/// Extract the RSA public key of the first certificate in a PEM value.
pub(crate) fn certificate_public_key(value: &str) -> Result<PublicKey, String> {
    let certificate = first_certificate(value)?;
    let key = certificate
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();
    PublicKey::from_pkcs1_der(key)
        .map_err(|error| format!("certificate does not contain an RSA public key: {error}"))
}

/// Compute the official Alipay certificate serial number (`app_cert_sn` /
/// `alipay_cert_sn`).
pub(crate) fn certificate_sn(value: &str) -> Result<String, String> {
    Ok(certificate_sn_of(&first_certificate(value)?))
}

/// Compute the official `alipay_root_cert_sn` for a PEM certificate chain.
///
/// Only certificates whose signature algorithm is RSA (OID prefix
/// `1.2.840.113549.1.1`) participate; the per-certificate SNs are joined with
/// `_` in chain order (`AntCertificationUtil.getRootCertSN`).
pub(crate) fn root_certificate_sn(value: &str) -> Result<String, String> {
    let certificates = parse_certificates(value)?;
    let mut serial_numbers = Vec::new();
    for certificate in &certificates {
        let signed_by_rsa = certificate
            .tbs_certificate
            .signature
            .oid
            .to_string()
            .starts_with("1.2.840.113549.1.1");
        if signed_by_rsa {
            serial_numbers.push(certificate_sn_of(certificate));
        }
    }
    if serial_numbers.is_empty() {
        return Err("alipay root certificate chain contains no RSA-signed certificate".to_string());
    }
    Ok(serial_numbers.join("_"))
}

fn certificate_sn_of(certificate: &Certificate) -> String {
    let issuer = certificate.tbs_certificate.issuer.to_string();
    let serial = decimal_serial(certificate.tbs_certificate.serial_number.as_bytes());
    let mut hasher = Md5::new();
    hasher.update(issuer.as_bytes());
    hasher.update(serial.as_bytes());
    hex_lower(&hasher.finalize())
}

/// Decode a positive big-endian certificate serial number to its decimal
/// representation.
///
/// Java's `X509Certificate.getSerialNumber()` is a `BigInteger`; string
/// concatenation with the issuer name therefore uses the **decimal** form.
fn decimal_serial(bytes: &[u8]) -> String {
    // Little-endian decimal digits, most significant first after the loop.
    let mut digits: Vec<u8> = vec![0];
    for byte in bytes {
        let mut carry = u32::from(*byte);
        for digit in digits.iter_mut().rev() {
            let value = u32::from(*digit) * 256 + carry;
            *digit = (value % 10) as u8;
            carry = value / 10;
        }
        while carry > 0 {
            digits.insert(0, (carry % 10) as u8);
            carry /= 10;
        }
    }
    let text: String = digits
        .iter()
        .map(|digit| char::from(b'0' + digit))
        .collect();
    let text = text.trim_start_matches('0');
    if text.is_empty() {
        "0".to_string()
    } else {
        text.to_string()
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

fn first_certificate(value: &str) -> Result<Certificate, String> {
    parse_certificates(value)?
        .into_iter()
        .next()
        .ok_or_else(|| "certificate PEM contains no certificate".to_string())
}

fn parse_certificates(value: &str) -> Result<Vec<Certificate>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("certificate must not be empty".to_string());
    }
    let normalized = if trimmed.contains("-----BEGIN CERTIFICATE") {
        trimmed.to_string()
    } else {
        normalize_pem(trimmed, "CERTIFICATE")
    };
    let certificates = Certificate::load_pem_chain(normalized.as_bytes())
        .map_err(|error| format!("cannot parse X.509 certificate: {error}"))?;
    if certificates.is_empty() {
        return Err("certificate PEM contains no certificate".to_string());
    }
    Ok(certificates)
}

/// Wrap a raw base64 body in PEM armor with 64-character lines.
///
/// PEM inputs are returned unchanged, so callers can pass configuration values
/// straight from the Alipay console (bare base64) or a PEM file.
pub(crate) fn normalize_pem(value: &str, label: &str) -> String {
    let trimmed = value.trim();
    if trimmed.contains("-----BEGIN") {
        return trimmed.to_string();
    }
    let body: String = trimmed.chars().filter(|ch| !ch.is_whitespace()).collect();
    let mut out = String::with_capacity(body.len() + label.len() * 2 + 40);
    out.push_str("-----BEGIN ");
    out.push_str(label);
    out.push_str("-----\n");
    for chunk in body.as_bytes().chunks(64) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn raw_base64_is_armored_and_pem_is_kept() {
        let armored = normalize_pem("abcd", "PRIVATE KEY");
        assert!(armored.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(armored.ends_with("-----END PRIVATE KEY-----"));
        let existing = "-----BEGIN PRIVATE KEY-----\nabcd\n-----END PRIVATE KEY-----";
        assert_eq!(normalize_pem(existing, "PRIVATE KEY"), existing);
    }

    #[test]
    fn keys_parse_from_pem_and_raw_base64() {
        use rsa::pkcs8::EncodePublicKey;

        let private = test_support::merchant_private_key_pem();
        let parsed = parse_private_key(private).expect("private key parses");
        assert_eq!(
            parsed
                .to_public_key()
                .to_public_key_der()
                .expect("der")
                .as_bytes(),
            test_support::merchant_public_key()
                .to_public_key_der()
                .expect("der")
                .as_bytes()
        );

        let raw = test_support::pem_body(private);
        assert!(parse_private_key(&raw).is_ok());

        let public = test_support::alipay_public_key_pem();
        assert!(parse_public_key(public).is_ok());
        assert!(parse_public_key(&test_support::pem_body(public)).is_ok());
    }

    #[test]
    fn pkcs1_private_keys_are_accepted() {
        use rsa::pkcs1::EncodeRsaPrivateKey;
        use rsa::pkcs8::LineEnding;
        let private = test_support::merchant_private_key();
        let pkcs1 = private
            .to_pkcs1_pem(LineEnding::LF)
            .expect("pkcs1 encoding")
            .to_string();
        assert!(pkcs1.contains("BEGIN RSA PRIVATE KEY"));
        assert!(parse_private_key(&pkcs1).is_ok());
    }

    #[test]
    fn rejects_garbage_keys_and_certificates() {
        assert!(parse_private_key("").is_err());
        assert!(parse_private_key("not a key").is_err());
        assert!(parse_public_key("not a key").is_err());
        assert!(certificate_public_key("not a certificate").is_err());
        assert!(certificate_sn("").is_err());
        assert!(root_certificate_sn("").is_err());
    }

    #[test]
    fn request_content_sorts_and_keeps_sign_type() {
        let params = vec![
            ("sign_type".to_string(), "RSA2".to_string()),
            ("sign".to_string(), "ignored".to_string()),
            ("app_id".to_string(), "2021000000000000".to_string()),
            ("empty".to_string(), String::new()),
            ("method".to_string(), "alipay.trade.query".to_string()),
            (
                "biz_content".to_string(),
                r#"{"out_trade_no":"ORDER-1"}"#.to_string(),
            ),
        ];
        let content = request_sign_content(&params);
        assert_eq!(
            content,
            "app_id=2021000000000000&biz_content={\"out_trade_no\":\"ORDER-1\"}&method=alipay.trade.query&sign_type=RSA2"
        );
    }

    #[test]
    fn notification_content_drops_sign_and_sign_type() {
        let params = vec![
            ("sign".to_string(), "sig".to_string()),
            ("sign_type".to_string(), "RSA2".to_string()),
            ("app_id".to_string(), "app".to_string()),
            ("total_amount".to_string(), "99.00".to_string()),
        ];
        assert_eq!(
            notify_sign_content(&params),
            "app_id=app&total_amount=99.00"
        );
    }

    #[test]
    fn rsa2_roundtrip_and_tamper_rejection() {
        let private = test_support::merchant_private_key();
        let public = test_support::merchant_public_key();
        let content = "app_id=app&biz_content={}&method=alipay.trade.query&sign_type=RSA2";
        let signature = rsa2_sign(content, private).expect("sign");
        assert!(rsa2_verify(content, &signature, public).is_ok());
        assert_eq!(
            rsa2_verify(
                "app_id=app&biz_content={\"x\":1}&method=alipay.trade.query&sign_type=RSA2",
                &signature,
                public
            ),
            Err("signature mismatch")
        );
        assert_eq!(
            rsa2_verify(content, "!not-base64!", public),
            Err("signature is not valid base64")
        );
        assert_eq!(rsa2_verify(content, "", public), Err("missing signature"));
    }

    #[test]
    fn sign_type_check_is_exact() {
        assert!(sign_type_is_rsa2("RSA2"));
        assert!(!sign_type_is_rsa2("RSA"));
        assert!(!sign_type_is_rsa2("rsa2"));
    }

    #[test]
    fn decimal_serial_matches_java_biginteger() {
        assert_eq!(decimal_serial(&[0]), "0");
        assert_eq!(decimal_serial(&[1]), "1");
        assert_eq!(decimal_serial(&[0x01, 0x00]), "256");
        assert_eq!(decimal_serial(&[0x00, 0x00, 0x2a]), "42");
        assert_eq!(
            decimal_serial(&[0x12, 0x34, 0x56, 0x78]),
            (0x1234_5678u64).to_string()
        );
    }

    #[test]
    fn certificate_serial_number_uses_issuer_plus_decimal_serial() {
        let fixtures = test_support::certificates();
        let expected = {
            let mut hasher = Md5::new();
            hasher.update(b"CN=alipay-test-ca,O=alipay-test,C=CN1");
            hex_lower(&hasher.finalize())
        };
        assert_eq!(certificate_sn(&fixtures.app_cert_pem).unwrap(), expected);
        assert_eq!(expected.len(), 32);
        assert!(expected
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
    }

    #[test]
    fn root_serial_number_joins_rsa_certificates() {
        let fixtures = test_support::certificates();
        let first = {
            let mut hasher = Md5::new();
            hasher.update(b"CN=alipay-root-1,O=alipay-test,C=CN1");
            hex_lower(&hasher.finalize())
        };
        let second = {
            let mut hasher = Md5::new();
            hasher.update(b"CN=alipay-root-2,O=alipay-test,C=CN2");
            hex_lower(&hasher.finalize())
        };
        assert_eq!(
            root_certificate_sn(&fixtures.root_chain_pem).unwrap(),
            format!("{first}_{second}")
        );
    }

    #[test]
    fn public_key_is_extracted_from_certificate() {
        let fixtures = test_support::certificates();
        let public = certificate_public_key(&fixtures.alipay_public_cert_pem).expect("public key");
        let signature = rsa2_sign("payload", test_support::alipay_private_key()).expect("sign");
        assert!(rsa2_verify("payload", &signature, &public).is_ok());
    }
}
