//! Registration/assertion verification.

use ciborium::Value;
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use webx_core::error::{Error, Result};
use webx_core::security::constant_time_eq;

use crate::{
    AssertionResult, CredentialRecord, RegisteredCredential, WebAuthnChallenge, WebAuthnConfig,
};

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_AT: u8 = 0x40;

/// Verify a registration response (`navigator.credentials.create`).
pub fn verify_registration(
    config: &WebAuthnConfig,
    challenge: &WebAuthnChallenge,
    client_data_json: &str,
    attestation_object: &[u8],
    credential_id: &[u8],
) -> Result<RegisteredCredential> {
    verify_client_data(config, challenge, client_data_json, "webauthn.create")?;

    let attestation: Value = ciborium::de::from_reader(attestation_object)
        .map_err(|_| Error::validation("invalid WebAuthn attestation object".to_string()))?;
    let map = attestation
        .as_map()
        .ok_or_else(|| Error::validation("attestation object is not a map".to_string()))?;
    let fmt = map_text(map, "fmt")
        .ok_or_else(|| Error::validation("attestation fmt is missing".to_string()))?;
    if fmt != "none" {
        return Err(Error::validation(format!(
            "unsupported WebAuthn attestation format '{fmt}'"
        )));
    }
    let empty_att_stmt = map
        .iter()
        .find(|(key, _)| key.as_text() == Some("attStmt"))
        .and_then(|(_, value)| value.as_map())
        .is_some_and(|statement| statement.is_empty());
    if !empty_att_stmt {
        return Err(Error::validation(
            "none attestation requires an empty attStmt".to_string(),
        ));
    }
    let auth_data = map_bytes(map, "authData")
        .ok_or_else(|| Error::validation("attestation authData is missing".to_string()))?;

    let parsed = parse_auth_data(config, auth_data, true)?;
    let Some(attested) = parsed.attested else {
        return Err(Error::validation(
            "registration authData has no attested credential data".to_string(),
        ));
    };
    if !constant_time_eq(&attested.credential_id, credential_id) {
        return Err(Error::unauthorized(
            "WebAuthn credential id mismatch".to_string(),
        ));
    }

    Ok(RegisteredCredential {
        credential: CredentialRecord {
            credential_id: attested.credential_id,
            public_key: attested.cose_key,
            sign_count: parsed.sign_count,
        },
        aaguid: attested.aaguid,
        fmt: fmt.to_string(),
    })
}

/// Verify an assertion response (`navigator.credentials.get`).
pub fn verify_assertion(
    config: &WebAuthnConfig,
    credential: &CredentialRecord,
    challenge: &WebAuthnChallenge,
    client_data_json: &str,
    authenticator_data: &[u8],
    credential_id: &[u8],
    signature: &[u8],
) -> Result<AssertionResult> {
    verify_client_data(config, challenge, client_data_json, "webauthn.get")?;
    if !constant_time_eq(&credential.credential_id, credential_id) {
        return Err(Error::unauthorized(
            "WebAuthn credential id mismatch".to_string(),
        ));
    }

    let parsed = parse_auth_data(config, authenticator_data, false)?;
    let (x, y) = parse_cose_es256(&credential.public_key)?;
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(0x04);
    sec1.extend_from_slice(&x);
    sec1.extend_from_slice(&y);
    let verifying_key = VerifyingKey::from_sec1_bytes(&sec1)
        .map_err(|_| Error::validation("invalid WebAuthn public key".to_string()))?;
    let signature = Signature::from_der(signature)
        .map_err(|_| Error::unauthorized("invalid WebAuthn signature".to_string()))?;

    let mut message = authenticator_data.to_vec();
    message.extend_from_slice(&Sha256::digest(client_data_json.as_bytes()));
    verifying_key
        .verify(&message, &signature)
        .map_err(|_| Error::unauthorized("WebAuthn assertion signature is invalid".to_string()))?;

    // A zero counter on both sides is allowed for authenticators that do not
    // implement counters; otherwise it must advance.
    if !(parsed.sign_count == 0 && credential.sign_count == 0)
        && parsed.sign_count <= credential.sign_count
    {
        return Err(Error::unauthorized(
            "WebAuthn signature counter did not advance".to_string(),
        ));
    }
    Ok(AssertionResult {
        sign_count: parsed.sign_count,
    })
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    typ: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: Option<bool>,
}

fn verify_client_data(
    config: &WebAuthnConfig,
    challenge: &WebAuthnChallenge,
    client_data_json: &str,
    expected_type: &str,
) -> Result<()> {
    let client_data: ClientData = serde_json::from_str(client_data_json)
        .map_err(|_| Error::validation("invalid WebAuthn clientDataJSON".to_string()))?;
    if client_data.typ != expected_type {
        return Err(Error::validation(format!(
            "WebAuthn type must be {expected_type}"
        )));
    }
    if !challenge.verify(&client_data.challenge) {
        return Err(Error::unauthorized(
            "WebAuthn challenge mismatch".to_string(),
        ));
    }
    if !config
        .origins
        .iter()
        .any(|origin| origin == &client_data.origin)
    {
        return Err(Error::unauthorized(
            "WebAuthn origin is not allowed".to_string(),
        ));
    }
    if client_data.cross_origin == Some(true) {
        return Err(Error::unauthorized(
            "cross-origin WebAuthn is not allowed".to_string(),
        ));
    }
    Ok(())
}

struct ParsedAuthData {
    sign_count: u32,
    attested: Option<AttestedCredential>,
}

struct AttestedCredential {
    aaguid: Vec<u8>,
    credential_id: Vec<u8>,
    cose_key: Vec<u8>,
}

fn parse_auth_data(
    config: &WebAuthnConfig,
    auth_data: &[u8],
    expect_attested: bool,
) -> Result<ParsedAuthData> {
    if auth_data.len() < 37 {
        return Err(Error::validation(
            "WebAuthn authData is too short".to_string(),
        ));
    }
    let rp_hash = &auth_data[..32];
    if !constant_time_eq(rp_hash, &config.rp_id_hash()) {
        return Err(Error::unauthorized(
            "WebAuthn rpIdHash mismatch".to_string(),
        ));
    }
    let flags = auth_data[32];
    if flags & FLAG_UP == 0 {
        return Err(Error::unauthorized(
            "WebAuthn user presence flag is missing".to_string(),
        ));
    }
    if config.require_user_verification && flags & FLAG_UV == 0 {
        return Err(Error::unauthorized(
            "WebAuthn user verification flag is missing".to_string(),
        ));
    }
    let sign_count = u32::from_be_bytes(auth_data[33..37].try_into().expect("4 bytes"));

    if !expect_attested {
        if flags & FLAG_AT != 0 {
            return Err(Error::validation(
                "assertion authData unexpectedly contains attested data".to_string(),
            ));
        }
        return Ok(ParsedAuthData {
            sign_count,
            attested: None,
        });
    }

    if flags & FLAG_AT == 0 || auth_data.len() < 55 {
        return Err(Error::validation(
            "registration authData has no attested credential data".to_string(),
        ));
    }
    let aaguid = auth_data[37..53].to_vec();
    let credential_len = u16::from_be_bytes([auth_data[53], auth_data[54]]) as usize;
    let credential_start: usize = 55;
    let credential_end = credential_start
        .checked_add(credential_len)
        .ok_or_else(|| Error::validation("invalid credential length".to_string()))?;
    if auth_data.len() <= credential_end {
        return Err(Error::validation(
            "truncated WebAuthn credential data".to_string(),
        ));
    }
    let credential_id = auth_data[credential_start..credential_end].to_vec();
    let cose: Value = ciborium::de::from_reader(&auth_data[credential_end..])
        .map_err(|_| Error::validation("invalid WebAuthn COSE key".to_string()))?;
    parse_cose_es256_value(&cose)?;
    let mut cose_key = Vec::new();
    ciborium::ser::into_writer(&cose, &mut cose_key)
        .map_err(|_| Error::validation("cannot re-encode WebAuthn COSE key".to_string()))?;

    Ok(ParsedAuthData {
        sign_count,
        attested: Some(AttestedCredential {
            aaguid,
            credential_id,
            cose_key,
        }),
    })
}

fn parse_cose_es256(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let value: Value = ciborium::de::from_reader(bytes)
        .map_err(|_| Error::validation("invalid WebAuthn COSE key".to_string()))?;
    parse_cose_es256_value(&value)
}

fn parse_cose_es256_value(value: &Value) -> Result<(Vec<u8>, Vec<u8>)> {
    let map = value
        .as_map()
        .ok_or_else(|| Error::validation("WebAuthn COSE key is not a map".to_string()))?;
    let kty = map_integer(map, 1).ok_or_else(unsupported_key)?;
    let alg = map_integer(map, 3).ok_or_else(unsupported_key)?;
    let crv = map_integer(map, -1).ok_or_else(unsupported_key)?;
    if kty != 2 || alg != -7 || crv != 1 {
        return Err(unsupported_key());
    }
    let x = map_bytes_value(map, -2)
        .ok_or_else(unsupported_key)?
        .clone();
    let y = map_bytes_value(map, -3)
        .ok_or_else(unsupported_key)?
        .clone();
    if x.len() != 32 || y.len() != 32 {
        return Err(unsupported_key());
    }
    Ok((x, y))
}

fn unsupported_key() -> Error {
    Error::validation("unsupported WebAuthn credential key (expected EC2/P-256 ES256)".to_string())
}

fn map_integer(map: &[(Value, Value)], key: i128) -> Option<i128> {
    map_get(map, key)
        .and_then(Value::as_integer)
        .map(i128::from)
}

fn map_bytes_value(map: &[(Value, Value)], key: i128) -> Option<&Vec<u8>> {
    map_get(map, key).and_then(Value::as_bytes)
}

fn map_bytes<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Vec<u8>> {
    map.iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .and_then(|(_, v)| v.as_bytes())
}

fn map_text<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a str> {
    map.iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .and_then(|(_, v)| v.as_text())
}

fn map_get(map: &[(Value, Value)], key: i128) -> Option<&Value> {
    map.iter()
        .find(|(candidate, _)| {
            candidate
                .as_integer()
                .is_some_and(|integer| i128::from(integer) == key)
        })
        .map(|(_, value)| value)
}
