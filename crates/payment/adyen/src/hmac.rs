//! Adyen notification HMAC-SHA256 signing and verification.
//!
//! The standard webhook signature is **not** computed over the raw body. Adyen
//! documents the following algorithm
//! ([Verify HMAC signatures](https://docs.adyen.com/development-resources/webhooks/secure-webhooks/verify-hmac-signatures)):
//!
//! 1. Take these fields in this order: `pspReference`, `originalReference`,
//!    `merchantAccountCode`, `merchantReference`, `amount.value`,
//!    `amount.currency`, `eventCode`, `success`. Missing fields are empty
//!    strings.
//! 2. Escape the standard escaping characters: `\` becomes `\\`, then `:` becomes
//!    `\:` (the official Java/PHP/.NET libraries do exactly this and nobody can
//!    include an unescaped delimiter in a signed value).
//! 3. Join the escaped values with `:`.
//! 4. Convert the Customer Area HMAC key from hexadecimal to bytes and calculate
//!    HMAC-SHA256 over the UTF-8 signing string.
//! 5. Encode the digest with Base64; the result is
//!    `notificationItems[].NotificationRequestItem.additionalData.hmacSignature`.
//!
//! The signature is compared in constant time. For compatibility with callers
//! that reproduce the task-level "hex lowercase" contract the verifier also
//! accepts a lowercase hex digest; Adyen itself always sends Base64.

use data_encoding::{BASE64, HEXLOWER, HEXLOWER_PERMISSIVE};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use webx_core::security::constant_time_eq;
use webx_pay::{PaymentError, ProviderId};

const PROVIDER: ProviderId = ProviderId::Adyen;

type HmacSha256 = Hmac<Sha256>;

/// The fields covered by Adyen's notification HMAC, in signing order.
///
/// All values are the *unescaped* field values; [`signing_string`] applies the
/// documented escaping before joining.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationHmacFields<'a> {
    /// `pspReference` (Adyen's 16-character transaction reference).
    pub psp_reference: &'a str,
    /// `originalReference` (empty for an AUTHORISATION).
    pub original_reference: &'a str,
    /// `merchantAccountCode`.
    pub merchant_account_code: &'a str,
    /// `merchantReference` (the merchant order reference).
    pub merchant_reference: &'a str,
    /// `amount.value` in minor units, as a decimal string.
    pub amount_value: &'a str,
    /// `amount.currency`.
    pub amount_currency: &'a str,
    /// `eventCode`, for example `AUTHORISATION`.
    pub event_code: &'a str,
    /// `success` as `"true"` / `"false"` (empty when absent).
    pub success: &'a str,
}

/// Escape one HMAC field value (`\` -> `\\`, then `:` -> `\:`).
pub fn escape_field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            ':' => escaped.push_str("\\:"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Build the colon-joined signing string for `fields`.
pub fn signing_string(fields: &NotificationHmacFields<'_>) -> String {
    [
        fields.psp_reference,
        fields.original_reference,
        fields.merchant_account_code,
        fields.merchant_reference,
        fields.amount_value,
        fields.amount_currency,
        fields.event_code,
        fields.success,
    ]
    .iter()
    .map(|value| escape_field(value))
    .collect::<Vec<_>>()
    .join(":")
}

/// Decode the Customer Area HMAC key (hexadecimal) into raw key bytes.
///
/// Fails with a configuration error (never echoing the key) when the key is
/// empty or not valid hexadecimal.
pub fn decode_hmac_key(hex_key: &str) -> Result<Vec<u8>, PaymentError> {
    let key = hex_key.trim();
    if key.is_empty() {
        return Err(PaymentError::config(PROVIDER, "hmac_key must not be empty"));
    }
    HEXLOWER_PERMISSIVE.decode(key.as_bytes()).map_err(|_| {
        PaymentError::config(
            PROVIDER,
            "hmac_key must be a hexadecimal string (Customer Area HMAC key)",
        )
    })
}

/// Calculate the Base64-encoded HMAC-SHA256 signature (Adyen's documented form).
pub fn sign_notification(fields: &NotificationHmacFields<'_>, hmac_key: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(hmac_key).expect("HMAC accepts keys of any length");
    mac.update(signing_string(fields).as_bytes());
    BASE64.encode(&mac.finalize().into_bytes())
}

/// Calculate the HMAC-SHA256 signature as lowercase hexadecimal.
///
/// Adyen itself sends Base64; this helper exists for callers/tests that pin the
/// hexadecimal representation (which [`verify_notification`] also accepts).
pub fn sign_notification_hex(fields: &NotificationHmacFields<'_>, hmac_key: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(hmac_key).expect("HMAC accepts keys of any length");
    mac.update(signing_string(fields).as_bytes());
    HEXLOWER.encode(&mac.finalize().into_bytes())
}

/// Verify `signature` against the notification fields in constant time.
///
/// Accepts Adyen's Base64 digest (documented and always produced by Adyen) and,
/// as a compatibility fallback, a lowercase hex digest. An empty signature is a
/// signature failure; callers should treat any failure as "reject the whole
/// delivery".
pub fn verify_notification(
    fields: &NotificationHmacFields<'_>,
    hmac_key: &[u8],
    signature: &str,
) -> Result<(), PaymentError> {
    let signature = signature.trim();
    if signature.is_empty() {
        return Err(PaymentError::signature(PROVIDER, "missing HMAC signature"));
    }
    let mut mac = HmacSha256::new_from_slice(hmac_key).expect("HMAC accepts keys of any length");
    mac.update(signing_string(fields).as_bytes());
    let digest = mac.finalize().into_bytes();
    let base64 = BASE64.encode(&digest);
    let hex = HEXLOWER.encode(&digest);
    let base64_match = constant_time_eq(signature.as_bytes(), base64.as_bytes());
    let hex_match = constant_time_eq(signature.as_bytes(), hex.as_bytes());
    if base64_match || hex_match {
        Ok(())
    } else {
        Err(PaymentError::signature(PROVIDER, "signature mismatch"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published Adyen test vector
    /// (`docs.adyen.com` > Verify HMAC signatures > Example): the HMAC key and
    /// the AUTHORISATION notification use these exact values.
    const VECTOR_KEY: &str = "44782DEF547AAA06C910C43932B1EB0C71FC68D9D0C057550C48EC2ACF6BA056";
    const VECTOR_SIGNATURE: &str = "coqCmt/IZ4E3CzPvMY8zTjQVL5hYJUiBRg8UU+iCWo0=";

    fn vector_fields() -> NotificationHmacFields<'static> {
        NotificationHmacFields {
            psp_reference: "7914073381342284",
            original_reference: "",
            merchant_account_code: "TestMerchant",
            merchant_reference: "TestPayment-1407325143704",
            amount_value: "1130",
            amount_currency: "EUR",
            event_code: "AUTHORISATION",
            success: "true",
        }
    }

    #[test]
    fn matches_the_official_documented_test_vector() {
        let key = decode_hmac_key(VECTOR_KEY).unwrap();
        assert_eq!(
            signing_string(&vector_fields()),
            "7914073381342284::TestMerchant:TestPayment-1407325143704:1130:EUR:AUTHORISATION:true"
        );
        assert_eq!(sign_notification(&vector_fields(), &key), VECTOR_SIGNATURE);
        assert!(verify_notification(&vector_fields(), &key, VECTOR_SIGNATURE).is_ok());
    }

    #[test]
    fn escaping_handles_backslashes_before_colons() {
        let fields = NotificationHmacFields {
            psp_reference: "A:B",
            original_reference: "C\\D",
            merchant_account_code: "M",
            merchant_reference: "R:S\\T",
            amount_value: "100",
            amount_currency: "EUR",
            event_code: "X",
            success: "false",
        };
        assert_eq!(
            signing_string(&fields),
            "A\\:B:C\\\\D:M:R\\:S\\\\T:100:EUR:X:false"
        );
        assert_eq!(escape_field("plain"), "plain");
        assert_eq!(escape_field("\\:"), "\\\\\\:");
    }

    #[test]
    fn hex_signature_is_accepted_as_a_compatibility_fallback() {
        let key = decode_hmac_key(VECTOR_KEY).unwrap();
        let hex = sign_notification_hex(&vector_fields(), &key);
        assert_eq!(hex.len(), 64);
        assert!(verify_notification(&vector_fields(), &key, &hex).is_ok());
        assert!(verify_notification(&vector_fields(), &key, &format!(" {hex} ")).is_ok());
    }

    #[test]
    fn tampered_fields_fail_verification() {
        let key = decode_hmac_key(VECTOR_KEY).unwrap();
        let tampered = NotificationHmacFields {
            amount_value: "1131",
            ..vector_fields()
        };
        let error = verify_notification(&tampered, &key, VECTOR_SIGNATURE).unwrap_err();
        assert_eq!(error.public_detail(), Some("signature mismatch"));
        assert_eq!(
            verify_notification(&vector_fields(), &key, "")
                .unwrap_err()
                .public_detail(),
            Some("missing HMAC signature")
        );
    }

    #[test]
    fn invalid_keys_are_configuration_errors_without_echoing_the_key() {
        assert!(decode_hmac_key("").is_err());
        assert!(decode_hmac_key("not hexadecimal").is_err());
        assert!(decode_hmac_key("abc").is_err());
        assert_eq!(decode_hmac_key("AABB").unwrap(), vec![0xAA, 0xBB]);
        let error = decode_hmac_key("zz").unwrap_err();
        assert!(!error.to_string().contains("zz"));
    }
}
