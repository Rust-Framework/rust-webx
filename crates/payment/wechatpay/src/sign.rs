//! WeChat Pay APIv3 signatures.
//!
//! Two message layouts exist:
//!
//! * requests: `METHOD \n URL \n TIMESTAMP \n NONCE \n BODY \n` (five lines,
//!   every line terminated by `\n`, empty body is an empty fifth line);
//! * responses and notifications: `TIMESTAMP \n NONCE \n BODY \n` (three lines).
//!
//! Both are signed with RSA-SHA256 (PKCS#1 v1.5) using the merchant API
//! certificate private key; WeChat verifies with the matching certificate
//! public key. Responses/notifications are verified with the WeChat Pay platform
//! key. RSA verification is not a secret-dependent comparison, so no
//! constant-time primitive is needed (the public key is public data).

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rsa::pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::sha2::{Digest, Sha256};
use rsa::{Pkcs1v15Sign, PrivateKey, PublicKey};

/// `Authorization` authentication scheme required by APIv3.
pub const AUTHORIZATION_SCHEMA: &str = "WECHATPAY2-SHA256-RSA2048";

/// Parse a merchant API certificate private key from PEM.
///
/// PKCS#8 (`BEGIN PRIVATE KEY`) is the documented format produced by the
/// WeChat Pay certificate tools; PKCS#1 (`BEGIN RSA PRIVATE KEY`) is accepted as
/// well because older tooling emits it.
pub fn parse_private_key_pem(pem: &str) -> Result<PrivateKey, String> {
    let pem = pem.trim();
    PrivateKey::from_pkcs8_pem(pem)
        .or_else(|_| PrivateKey::from_pkcs1_pem(pem))
        .map_err(|_| "merchant private key is not valid PKCS#8 or PKCS#1 PEM".to_string())
}

/// Parse a PEM public key (WeChat Pay public-key mode, SPKI or PKCS#1).
pub fn parse_public_key_pem(pem: &str) -> Result<PublicKey, String> {
    let pem = pem.trim();
    PublicKey::from_public_key_pem(pem)
        .or_else(|_| PublicKey::from_pkcs1_pem(pem))
        .map_err(|_| "WeChat Pay public key is not valid SPKI or PKCS#1 PEM".to_string())
}

/// Message signed for an API request.
pub fn request_signature_message(
    method: &str,
    url: &str,
    timestamp: i64,
    nonce: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut message = Vec::with_capacity(method.len() + url.len() + nonce.len() + body.len() + 48);
    message.extend_from_slice(method.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(url.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(timestamp.to_string().as_bytes());
    message.push(b'\n');
    message.extend_from_slice(nonce.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(body);
    message.push(b'\n');
    message
}

/// Message signed for an API response or callback notification.
pub fn response_signature_message(timestamp: &str, nonce: &str, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(timestamp.len() + nonce.len() + body.len() + 3);
    message.extend_from_slice(timestamp.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(nonce.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(body);
    message.push(b'\n');
    message
}

/// Message signed for in-app payment invocation of the App channel
/// (`appId\ntimeStamp\nnonceStr\nprepayId\n`).
pub fn app_client_signature_message(
    app_id: &str,
    timestamp: &str,
    nonce: &str,
    prepay_id: &str,
) -> Vec<u8> {
    client_signature_message(app_id, timestamp, nonce, prepay_id)
}

/// Message signed for JSAPI/小程序 payment invocation
/// (`appId\ntimeStamp\nnonceStr\npackage\n`, where `package` is
/// `prepay_id=<id>`).
pub fn jsapi_client_signature_message(
    app_id: &str,
    timestamp: &str,
    nonce: &str,
    package: &str,
) -> Vec<u8> {
    client_signature_message(app_id, timestamp, nonce, package)
}

fn client_signature_message(app_id: &str, timestamp: &str, nonce: &str, last: &str) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(app_id.len() + timestamp.len() + nonce.len() + last.len() + 4);
    message.extend_from_slice(app_id.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(timestamp.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(nonce.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(last.as_bytes());
    message.push(b'\n');
    message
}

/// RSA-SHA256 sign `message` with the merchant key, returning base64.
pub fn sign_message_base64(private_key: &PrivateKey, message: &[u8]) -> Result<String, String> {
    let digest = Sha256::digest(message);
    let signature = private_key
        .sign(Pkcs1v15Sign::new::<Sha256>(), &digest)
        .map_err(|_| "RSA signing failed".to_string())?;
    Ok(STANDARD.encode(signature))
}

/// Verify a base64 RSA-SHA256 signature with a platform public key.
pub fn verify_message_base64(
    public_key: &PublicKey,
    message: &[u8],
    signature_base64: &str,
) -> Result<(), String> {
    let signature = STANDARD
        .decode(signature_base64.trim())
        .map_err(|_| "signature is not valid base64".to_string())?;
    let digest = Sha256::digest(message);
    public_key
        .verify(Pkcs1v15Sign::new::<Sha256>(), &digest, &signature)
        .map_err(|_| "signature mismatch".to_string())
}

/// Build the `Authorization` header value for `mch_id`/`serial_no`.
pub fn authorization_header(
    mch_id: &str,
    serial_no: &str,
    timestamp: i64,
    nonce: &str,
    signature: &str,
) -> String {
    format!(
        "{AUTHORIZATION_SCHEMA} mchid=\"{mch_id}\",nonce_str=\"{nonce}\",signature=\"{signature}\",timestamp=\"{timestamp}\",serial_no=\"{serial_no}\""
    )
}

/// Random 32-character nonce (`uuid` v4 without dashes, as recommended).
pub(crate) fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Validate a `Wechatpay-Timestamp` header against the local clock.
///
/// WeChat Pay recommends a maximum of five minutes of clock skew for both
/// responses and callbacks (replay protection).
pub fn validate_timestamp(
    timestamp: &str,
    now: i64,
    tolerance_seconds: i64,
) -> Result<i64, &'static str> {
    let parsed: i64 = timestamp
        .trim()
        .parse()
        .map_err(|_| "invalid signature timestamp")?;
    if (now - parsed).abs() > tolerance_seconds {
        return Err("timestamp outside tolerance");
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn request_signature_message_has_the_five_documented_lines() {
        let message = request_signature_message(
            "POST",
            "/v3/pay/transactions/jsapi",
            1_554_208_460,
            "593BEC0C930BF1AFEB40B4A08C8FB242",
            br#"{"appid":"wx1"}"#,
        );
        assert_eq!(
            String::from_utf8(message).unwrap(),
            concat!(
                "POST\n",
                "/v3/pay/transactions/jsapi\n",
                "1554208460\n",
                "593BEC0C930BF1AFEB40B4A08C8FB242\n",
                "{\"appid\":\"wx1\"}\n",
            )
        );
    }

    #[test]
    fn empty_body_request_signature_ends_with_an_empty_line() {
        let message = request_signature_message(
            "GET",
            "/v3/certificates",
            1_554_208_460,
            "593BEC0C930BF1AFEB40B4A08C8FB242",
            b"",
        );
        assert_eq!(
            String::from_utf8(message).unwrap(),
            "GET\n/v3/certificates\n1554208460\n593BEC0C930BF1AFEB40B4A08C8FB242\n\n"
        );
    }

    #[test]
    fn query_string_is_part_of_the_signed_url() {
        let message = request_signature_message(
            "GET",
            "/v3/pay/transactions/out-trade-no/ORDER-1?mchid=1900000001",
            1_700_000_000,
            "nonce",
            b"",
        );
        assert_eq!(
            String::from_utf8(message).unwrap(),
            "GET\n/v3/pay/transactions/out-trade-no/ORDER-1?mchid=1900000001\n1700000000\nnonce\n\n"
        );
    }

    #[test]
    fn response_signature_message_has_three_documented_lines() {
        let message = response_signature_message(
            "1722850421",
            "d824f2e086d3c1df967785d13fcd22ef",
            br#"{"code_url":"weixin://wxpay/bizpayurl?pr=JyC91EIz1"}"#,
        );
        assert_eq!(
            String::from_utf8(message).unwrap(),
            concat!(
                "1722850421\n",
                "d824f2e086d3c1df967785d13fcd22ef\n",
                "{\"code_url\":\"weixin://wxpay/bizpayurl?pr=JyC91EIz1\"}\n",
            )
        );
    }

    #[test]
    fn empty_response_body_still_has_a_third_line() {
        let message = response_signature_message("1722850421", "nonce", b"");
        assert_eq!(String::from_utf8(message).unwrap(), "1722850421\nnonce\n\n");
    }

    #[test]
    fn client_signature_messages_match_the_documented_field_order() {
        let app = app_client_signature_message("wx1", "1554208460", "ABC", "wx2014");
        assert_eq!(
            String::from_utf8(app).unwrap(),
            "wx1\n1554208460\nABC\nwx2014\n"
        );
        let jsapi = jsapi_client_signature_message("wx1", "1554208460", "ABC", "prepay_id=wx2014");
        assert_eq!(
            String::from_utf8(jsapi).unwrap(),
            "wx1\n1554208460\nABC\nprepay_id=wx2014\n"
        );
    }

    #[test]
    fn authorization_header_uses_the_documented_schema_and_spacing() {
        let header = authorization_header(
            "1900007291",
            "408B07E79B8269FEC3D5D3E6AB8ED163A6A380DB",
            1_554_208_460,
            "593BEC0C930BF1AFEB40B4A08C8FB242",
            "c2lnbmF0dXJl",
        );
        assert_eq!(
            header,
            concat!(
                "WECHATPAY2-SHA256-RSA2048 ",
                "mchid=\"1900007291\",",
                "nonce_str=\"593BEC0C930BF1AFEB40B4A08C8FB242\",",
                "signature=\"c2lnbmF0dXJl\",",
                "timestamp=\"1554208460\",",
                "serial_no=\"408B07E79B8269FEC3D5D3E6AB8ED163A6A380DB\""
            )
        );
        assert!(header.starts_with(AUTHORIZATION_SCHEMA));
        assert!(!header.contains('\n'));
    }

    #[test]
    fn nonces_are_url_safe_uuid_without_dashes() {
        let first = new_nonce();
        let second = new_nonce();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn rsa_sign_verify_roundtrip_with_a_generated_key() {
        let key = test_support::key_pair();
        let message =
            request_signature_message("POST", "/v3/pay/transactions/native", 1, "n", b"{}");
        let signature = sign_message_base64(&key.private, &message).unwrap();
        verify_message_base64(&key.public, &message, &signature).unwrap();
        // Tampered message
        assert!(verify_message_base64(&key.public, b"another message", &signature).is_err());
        // Tampered signature
        let mut bytes = STANDARD.decode(&signature).unwrap();
        bytes[0] ^= 0xFF;
        assert!(verify_message_base64(&key.public, &message, &STANDARD.encode(bytes)).is_err());
        // Garbage base64
        assert!(verify_message_base64(&key.public, &message, "!!!not-base64!!!").is_err());
    }

    #[test]
    fn generated_key_pem_roundtrips_through_the_parsers() {
        let key = test_support::key_pair();
        let private = parse_private_key_pem(&key.private_pem).unwrap();
        let public = parse_public_key_pem(&key.public_pem).unwrap();
        let message = b"signed";
        verify_message_base64(
            &public,
            message,
            &sign_message_base64(&private, message).unwrap(),
        )
        .unwrap();
        assert!(parse_private_key_pem(
            "-----BEGIN PRIVATE KEY-----\nnot-a-key\n-----END PRIVATE KEY-----"
        )
        .is_err());
        assert!(parse_public_key_pem("not a pem").is_err());
    }

    #[test]
    fn timestamp_window_is_five_minutes_by_default() {
        assert!(validate_timestamp("1700000000", 1_700_000_000, 300).is_ok());
        assert!(validate_timestamp("1700000000", 1_700_000_299, 300).is_ok());
        assert_eq!(
            validate_timestamp("1700000000", 1_700_000_301, 300),
            Err("timestamp outside tolerance")
        );
        assert_eq!(
            validate_timestamp("1700000000", 1_699_999_699, 300),
            Err("timestamp outside tolerance")
        );
        assert_eq!(
            validate_timestamp("", 1_700_000_000, 300),
            Err("invalid signature timestamp")
        );
    }
}
