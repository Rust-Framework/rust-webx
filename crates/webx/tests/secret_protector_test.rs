//! At-rest secret protection: host registration, round-trip, fail-closed.
//!
//! The framework registers no protector by default, so a missing adapter fails
//! at DI resolution instead of silently persisting plaintext.

use std::sync::Arc;

use webx::*;

fn key(byte: u8) -> Vec<u8> {
    vec![byte; 32]
}

#[tokio::test]
async fn a_host_registers_the_protector_and_round_trips_a_secret() {
    let protector = Arc::new(
        AesGcmSecretProtector::from_keys(
            vec![
                ("2026-11".to_string(), key(1)),
                ("2026-12".to_string(), key(2)),
            ],
            "2026-12",
        )
        .expect("valid key ring"),
    );

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_secret_protector(protector)
        .build();

    let port = host
        .provider()
        .get::<dyn ISecretProtector>()
        .expect("the protector port is registered");
    assert_eq!(port.active_key_id(), "2026-12");

    let ciphertext = port.protect("totp-seed").await.expect("protect");
    assert!(ciphertext.starts_with("webx1:2026-12:"));
    assert_ne!(ciphertext, "totp-seed");
    assert_eq!(
        port.unprotect(&ciphertext)
            .await
            .expect("unprotect")
            .expose(),
        "totp-seed"
    );
}

#[tokio::test]
async fn a_host_without_a_protector_never_stores_plaintext_silently() {
    let host = Host::builder().mode(AppMode::Development).no_spa().build();
    assert!(
        host.provider().get::<dyn ISecretProtector>().is_err(),
        "a missing protector must fail at resolution"
    );
}

#[tokio::test]
async fn rotation_keeps_old_rows_readable_and_new_rows_on_the_new_key() {
    let old = AesGcmSecretProtector::from_keys(vec![("2026-11".to_string(), key(1))], "2026-11")
        .expect("old ring");
    let rotated = AesGcmSecretProtector::from_keys(
        vec![
            ("2026-11".to_string(), key(1)),
            ("2026-12".to_string(), key(2)),
        ],
        "2026-12",
    )
    .expect("rotated ring");

    let old_row = old.protect("legacy").await.expect("protect");
    assert_eq!(
        rotated
            .unprotect(&old_row)
            .await
            .expect("old row readable")
            .expose(),
        "legacy"
    );
    let new_row = rotated.protect("fresh").await.expect("protect");
    assert!(new_row.starts_with("webx1:2026-12:"));
}
