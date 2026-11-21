//! Browser-level WebAuthn e2e (Chromium virtual authenticator).
//!
//! The browser captures a real `navigator.credentials.create/get` pair with a
//! CTAP2 virtual authenticator; this test feeds the exact wire bytes into the
//! Rust RP verifier. Generate the JSON with `crates/webauthn/tests/browser_e2e.js`
//! (Playwright) and run:
//!
//! ```powershell
//! $env:WEBAUTHN_BROWSER_JSON = "...\browser_webauthn.json"
//! cargo test -p rust-webx-webauthn --test browser_e2e_test -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use base64::Engine as _;
use serde_json::Value;
use webx_webauthn::verify::{verify_assertion, verify_registration};
use webx_webauthn::{WebAuthnChallenge, WebAuthnConfig};

fn decode(value: &Value, key: &str) -> Vec<u8> {
    let text = value[key].as_str().expect(key);
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .expect(key)
}

fn decode_text(value: &Value, key: &str) -> String {
    String::from_utf8(decode(value, key)).expect(key)
}

#[test]
#[ignore = "requires a browser capture; run manually with --ignored"]
fn browser_registration_and_assertion_verify() {
    let path = std::env::var("WEBAUTHN_BROWSER_JSON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("browser_webauthn.json"));
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("browser JSON")).expect("json");
    let config = WebAuthnConfig {
        rp_id: document["rp_id"].as_str().expect("rp_id").to_string(),
        origins: vec![document["origin"].as_str().expect("origin").to_string()],
        require_user_verification: true,
    };
    let registration = &document["registration"];
    let challenge = WebAuthnChallenge::from_bytes(decode(&document, "reg_challenge"));
    let registered = verify_registration(
        &config,
        &challenge,
        &decode_text(registration, "clientDataJSON"),
        &decode(registration, "attestationObject"),
        &decode(registration, "rawId"),
    )
    .expect("registration verifies");

    let assertion = &document["assertion"];
    let challenge = WebAuthnChallenge::from_bytes(decode(&document, "auth_challenge"));
    let result = verify_assertion(
        &config,
        &registered.credential,
        &challenge,
        &decode_text(assertion, "clientDataJSON"),
        &decode(assertion, "authenticatorData"),
        &decode(assertion, "rawId"),
        &decode(assertion, "signature"),
    )
    .expect("assertion verifies");
    assert!(result.sign_count >= registered.credential.sign_count);
    eprintln!(
        "browser WebAuthn OK: credential_bytes={} sign_count={}",
        registered.credential.credential_id.len(),
        result.sign_count
    );
}
