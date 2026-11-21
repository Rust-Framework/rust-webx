//! 2.5 M3 — WebAuthn/Passkey registration and assertion verification (ES256).

use ciborium::Value;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::Generate as _;
use sha2::{Digest as _, Sha256};
use webx_core::error::Error;
use webx_webauthn::verify::{verify_assertion, verify_registration};
use webx_webauthn::{WebAuthnChallenge, WebAuthnConfig};

const ORIGIN: &str = "https://app.example.com";
const RP_ID: &str = "example.com";

fn int(value: i32) -> Value {
    Value::Integer(value.into())
}

fn cose_key(signing: &SigningKey) -> Value {
    // SEC1 uncompressed point: 0x04 || x(32) || y(32).
    let encoded = signing.verifying_key().to_sec1_bytes();
    assert_eq!(encoded.first(), Some(&0x04), "uncompressed SEC1 point");
    let x = encoded[1..33].to_vec();
    let y = encoded[33..65].to_vec();
    Value::Map(vec![
        (int(1), int(2)),           // kty: EC2
        (int(3), int(-7)),          // alg: ES256
        (int(-1), int(1)),          // crv: P-256
        (int(-2), Value::Bytes(x)), // x
        (int(-3), Value::Bytes(y)), // y
    ])
}

fn auth_data(
    rp_id: &str,
    flags: u8,
    sign_count: u32,
    attested: Option<(&[u8], &[u8], &Value)>,
) -> Vec<u8> {
    let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
    data.push(flags);
    data.extend_from_slice(&sign_count.to_be_bytes());
    if let Some((aaguid, credential_id, cose)) = attested {
        data.extend_from_slice(aaguid);
        data.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        data.extend_from_slice(credential_id);
        ciborium::ser::into_writer(cose, &mut data).expect("cose");
    }
    data
}

fn attestation(auth_data: &[u8]) -> Vec<u8> {
    let object = Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("none".into())),
        (Value::Text("attStmt".into()), Value::Map(Vec::new())),
        (
            Value::Text("authData".into()),
            Value::Bytes(auth_data.to_vec()),
        ),
    ]);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&object, &mut bytes).expect("attestation");
    bytes
}

fn client_data(kind: &str, challenge: &WebAuthnChallenge, origin: &str) -> String {
    format!(
        r#"{{"type":"{kind}","challenge":"{}","origin":"{origin}","crossOrigin":false}}"#,
        challenge.to_base64url()
    )
}

fn config() -> WebAuthnConfig {
    WebAuthnConfig::new(RP_ID, ORIGIN)
        .expect("config")
        .require_user_verification(true)
}

#[test]
fn registration_and_assertion_round_trip() {
    let signing = SigningKey::generate_from_rng(&mut rand::rng());
    let cose = cose_key(&signing);
    let credential_id = b"credential-1".to_vec();
    let challenge = WebAuthnChallenge::generate();
    let auth = auth_data(
        RP_ID,
        0x45,
        0,
        Some((&[0u8; 16], credential_id.as_slice(), &cose)),
    );
    let client = client_data("webauthn.create", &challenge, ORIGIN);
    let registered = verify_registration(
        &config(),
        &challenge,
        &client,
        &attestation(&auth),
        &credential_id,
    )
    .expect("registration");

    assert_eq!(registered.fmt, "none");
    assert_eq!(registered.credential.credential_id, credential_id);
    assert_eq!(registered.credential.sign_count, 0);
    assert_eq!(registered.aaguid, vec![0u8; 16]);

    let challenge = WebAuthnChallenge::generate();
    let auth = auth_data(RP_ID, 0x05, 1, None);
    let client = client_data("webauthn.get", &challenge, ORIGIN);
    let mut message = auth.clone();
    message.extend_from_slice(&Sha256::digest(client.as_bytes()));
    let signature: Signature = signing.sign(&message);
    let signature = signature.to_der().as_bytes().to_vec();

    let result = verify_assertion(
        &config(),
        &registered.credential,
        &challenge,
        &client,
        &auth,
        &credential_id,
        &signature,
    )
    .expect("assertion");
    assert_eq!(result.sign_count, 1);
}

#[test]
fn registration_rejects_wrong_challenge_origin_rp_and_uv() {
    let signing = SigningKey::generate_from_rng(&mut rand::rng());
    let cose = cose_key(&signing);
    let credential_id = b"credential-2".to_vec();
    let challenge = WebAuthnChallenge::generate();
    let auth = auth_data(
        RP_ID,
        0x45,
        0,
        Some((&[0u8; 16], credential_id.as_slice(), &cose)),
    );
    let client = client_data("webauthn.create", &challenge, ORIGIN);
    let attestation_bytes = attestation(&auth);

    let other = WebAuthnChallenge::generate();
    assert!(verify_registration(
        &config(),
        &other,
        &client,
        &attestation_bytes,
        &credential_id
    )
    .is_err());
    assert!(verify_registration(
        &config(),
        &challenge,
        &client_data("webauthn.create", &challenge, "https://evil.example.com"),
        &attestation_bytes,
        &credential_id
    )
    .is_err());

    // RP ID hash mismatch.
    let wrong_rp = auth_data(
        "other.example.com",
        0x45,
        0,
        Some((&[0u8; 16], credential_id.as_slice(), &cose)),
    );
    assert!(verify_registration(
        &config(),
        &challenge,
        &client,
        &attestation(&wrong_rp),
        &credential_id
    )
    .is_err());

    // require_user_verification but UV flag is missing (UP|AT only).
    let no_uv = auth_data(
        RP_ID,
        0x41,
        0,
        Some((&[0u8; 16], credential_id.as_slice(), &cose)),
    );
    assert!(verify_registration(
        &config(),
        &challenge,
        &client,
        &attestation(&no_uv),
        &credential_id
    )
    .is_err());
}

#[test]
fn assertion_rejects_counter_credential_and_signature_failures() {
    let signing = SigningKey::generate_from_rng(&mut rand::rng());
    let cose = cose_key(&signing);
    let credential_id = b"credential-3".to_vec();
    let challenge = WebAuthnChallenge::generate();
    let auth = auth_data(
        RP_ID,
        0x45,
        0,
        Some((&[0u8; 16], credential_id.as_slice(), &cose)),
    );
    let client = client_data("webauthn.create", &challenge, ORIGIN);
    let registered = verify_registration(
        &config(),
        &challenge,
        &client,
        &attestation(&auth),
        &credential_id,
    )
    .expect("registration");

    let challenge = WebAuthnChallenge::generate();
    let auth = auth_data(RP_ID, 0x05, 1, None);
    let client = client_data("webauthn.get", &challenge, ORIGIN);
    let mut message = auth.clone();
    message.extend_from_slice(&Sha256::digest(client.as_bytes()));
    let signature: Signature = signing.sign(&message);
    let signature = signature.to_der().as_bytes().to_vec();

    // Counter did not advance.
    let stale = auth_data(RP_ID, 0x05, 0, None);
    let mut stale_message = stale.clone();
    stale_message.extend_from_slice(&Sha256::digest(client.as_bytes()));
    let stale_signature: Signature = signing.sign(&stale_message);
    let stale_signature = stale_signature.to_der().as_bytes().to_vec();
    let mut stored = registered.credential.clone();
    stored.sign_count = 1;

    assert!(verify_assertion(
        &config(),
        &stored,
        &challenge,
        &client,
        &stale,
        &credential_id,
        &stale_signature
    )
    .is_err());

    // Wrong credential id.
    assert!(verify_assertion(
        &config(),
        &stored,
        &challenge,
        &client,
        &auth,
        b"other-id",
        &signature
    )
    .is_err());

    // Tampered signature.
    let mut tampered = signature.clone();
    tampered[0] ^= 0x01;
    assert!(verify_assertion(
        &config(),
        &stored,
        &challenge,
        &client,
        &auth,
        &credential_id,
        &tampered
    )
    .is_err());

    // Tampered clientData.
    assert!(verify_assertion(
        &config(),
        &stored,
        &challenge,
        &client_data("webauthn.get", &challenge, "https://evil.example.com"),
        &auth,
        &credential_id,
        &signature
    )
    .is_err());
}

#[test]
fn malformed_attestation_and_assertion_bytes_never_panic() {
    let config = config();
    let challenge = WebAuthnChallenge::generate();
    let client = client_data("webauthn.create", &challenge, ORIGIN);
    let credential = webx_webauthn::CredentialRecord {
        credential_id: b"credential".to_vec(),
        public_key: vec![0xa1, 0x01, 0x02],
        sign_count: 0,
    };
    let assertion_client = client_data("webauthn.get", &challenge, ORIGIN);
    let mut state = 0x1234_5678_9abc_def0_u64;
    for length in [0usize, 1, 2, 7, 32, 64, 255, 1024] {
        let mut bytes = Vec::with_capacity(length);
        for _ in 0..length {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            bytes.push((state & 0xff) as u8);
        }
        let _ = verify_registration(&config, &challenge, &client, &bytes, b"credential");
        let _ = verify_assertion(
            &config,
            &credential,
            &challenge,
            &assertion_client,
            &bytes,
            b"credential",
            &bytes,
        );
    }
}

#[test]
fn none_attestation_requires_an_empty_att_stmt() {
    let signing = SigningKey::generate_from_rng(&mut rand::rng());
    let cose = cose_key(&signing);
    let credential_id = b"credential-5".to_vec();
    let auth = auth_data(
        RP_ID,
        0x45,
        0,
        Some((&[0u8; 16], credential_id.as_slice(), &cose)),
    );
    let challenge = WebAuthnChallenge::generate();
    let client = client_data("webauthn.create", &challenge, ORIGIN);
    let object = Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("none".into())),
        (
            Value::Text("attStmt".into()),
            Value::Map(vec![(Value::Text("unexpected".into()), Value::Bool(true))]),
        ),
        (Value::Text("authData".into()), Value::Bytes(auth)),
    ]);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&object, &mut bytes).expect("attestation");
    let error = verify_registration(&config(), &challenge, &client, &bytes, &credential_id)
        .expect_err("non-empty attStmt");
    assert!(matches!(error, Error::Validation(_)), "{error:?}");
}

#[test]
fn registration_rejects_non_none_attestation_and_short_data() {
    let (aaguid, cose) = {
        let signing = SigningKey::generate_from_rng(&mut rand::rng());
        (vec![0u8; 16], cose_key(&signing))
    };
    let credential_id = b"credential-4".to_vec();
    let auth = auth_data(
        RP_ID,
        0x45,
        0,
        Some((aaguid.as_slice(), credential_id.as_slice(), &cose)),
    );
    let challenge = WebAuthnChallenge::generate();
    let client = client_data("webauthn.create", &challenge, ORIGIN);

    let packed = Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("packed".into())),
        (Value::Text("attStmt".into()), Value::Map(Vec::new())),
        (Value::Text("authData".into()), Value::Bytes(auth.clone())),
    ]);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&packed, &mut bytes).expect("packed");
    let error = verify_registration(&config(), &challenge, &client, &bytes, &credential_id)
        .expect_err("packed must be rejected");
    assert!(matches!(error, Error::Validation(_)), "{error:?}");

    assert!(
        verify_registration(&config(), &challenge, &client, b"not-cbor", &credential_id).is_err()
    );
}
