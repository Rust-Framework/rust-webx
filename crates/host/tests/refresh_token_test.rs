//! 2.5 M2 — refresh token rotation, reuse detection and family revocation.

use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use webx_core::metrics::MemoryMeter;
use webx_core::security::{InMemoryRefreshTokenStore, RefreshTokenRecord, RefreshTokenStatus};
use webx_host::refresh::RefreshTokenService;

fn service() -> Arc<RefreshTokenService> {
    Arc::new(RefreshTokenService::new(
        Arc::new(InMemoryRefreshTokenStore::new()),
        Duration::from_secs(3600),
        Duration::from_secs(24 * 3600),
    ))
}

fn hash(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn issue_stores_only_the_hash_and_rotation_returns_a_new_token() {
    let service = service();
    let first = service
        .issue(
            "user-1",
            Some("tenant-1".to_string()),
            Some("web".to_string()),
        )
        .await
        .expect("issue");

    let (token_id, secret) = first.token.split_once('.').expect("token shape");
    let record = service
        .store()
        .get(token_id)
        .await
        .expect("get")
        .expect("record");
    assert_ne!(record.secret_hash, secret, "the secret must not be stored");
    assert_eq!(record.secret_hash, hash(secret));
    assert_eq!(record.generation, 0);
    assert_eq!(record.status, RefreshTokenStatus::Active);

    let second = service.rotate(&first.token).await.expect("rotate");
    assert_ne!(second.token, first.token);
    assert_eq!(second.family_id, first.family_id);
    assert!(service
        .rotate(&first.token)
        .await
        .expect_err("reuse must fail")
        .to_string()
        .contains("reuse"));
}

#[tokio::test]
async fn reuse_revokes_the_whole_family() {
    let service = service();
    let first = service.issue("user-1", None, None).await.expect("issue");
    let second = service.rotate(&first.token).await.expect("rotate");

    assert!(service.rotate(&first.token).await.is_err(), "reuse");
    assert!(
        service.rotate(&second.token).await.is_err(),
        "the rotated token belongs to the revoked family"
    );
}

#[tokio::test]
async fn a_wrong_secret_does_not_revoke_the_family() {
    let service = service();
    let issued = service.issue("user-1", None, None).await.expect("issue");
    let (token_id, _) = issued.token.split_once('.').expect("shape");

    let forged = format!("{token_id}.{}", "0".repeat(64));
    assert!(service.rotate(&forged).await.is_err());

    // The legitimate token is still usable: a random guess must not log the
    // user out.
    service
        .rotate(&issued.token)
        .await
        .expect("legitimate rotate");
}

#[tokio::test]
async fn malformed_tokens_are_rejected() {
    let service = service();
    for token in ["", "no-dot", ".secret", "id.", "a.b.c"] {
        assert!(service.rotate(token).await.is_err(), "accepted {token:?}");
    }
}

#[tokio::test]
async fn expired_token_is_rejected() {
    let service = service();
    let token_id = "expired-id";
    let secret = "s".repeat(64);
    let now = now_secs();
    service
        .store()
        .insert(RefreshTokenRecord {
            token_id: token_id.to_string(),
            secret_hash: hash(&secret),
            family_id: "family".to_string(),
            generation: 0,
            subject: "user-1".to_string(),
            tenant_id: None,
            client_id: None,
            issued_at_unix_secs: now - 100,
            expires_at_unix_secs: now - 1,
            used_at_unix_secs: None,
            status: RefreshTokenStatus::Active,
            family_created_at_unix_secs: now - 100,
        })
        .await
        .expect("insert");

    assert!(service
        .rotate(&format!("{token_id}.{secret}"))
        .await
        .is_err());
}

#[tokio::test]
async fn concurrent_rotation_has_exactly_one_winner() {
    let service = service();
    let issued = service.issue("user-1", None, None).await.expect("issue");
    let token = issued.token.clone();

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let service = Arc::clone(&service);
        let token = token.clone();
        tasks.push(tokio::spawn(async move { service.rotate(&token).await }));
    }
    let mut winners = Vec::new();
    for task in tasks {
        if let Ok(Ok(rotated)) = task.await {
            winners.push(rotated);
        }
    }
    assert_eq!(winners.len(), 1, "exactly one caller may rotate");
    assert!(
        service.rotate(&winners[0].token).await.is_err(),
        "losing rotations are reuse and revoke the new token's family"
    );
}

#[tokio::test]
async fn revoke_all_for_subject_only_affects_that_subject() {
    let service = service();
    let alice = service.issue("alice", None, None).await.expect("alice");
    let bob = service.issue("bob", None, None).await.expect("bob");

    assert_eq!(service.revoke_all_for_subject("alice").await.unwrap(), 1);
    assert!(service.rotate(&alice.token).await.is_err());
    service.rotate(&bob.token).await.expect("bob remains valid");
}

#[tokio::test]
async fn refresh_metrics_count_issue_rotate_and_reuse() {
    let meter = MemoryMeter::new();
    let service = RefreshTokenService::new(
        Arc::new(InMemoryRefreshTokenStore::new()),
        Duration::from_secs(3600),
        Duration::from_secs(24 * 3600),
    )
    .with_meter(&meter);

    let issued = service.issue("user-1", None, None).await.expect("issue");
    let rotated = service.rotate(&issued.token).await.expect("rotate");
    assert!(service.rotate(&issued.token).await.is_err(), "reuse");

    assert_eq!(
        meter.counter_value("auth.refresh.events", &[("outcome", "issued")]),
        1
    );
    assert_eq!(
        meter.counter_value("auth.refresh.events", &[("outcome", "rotated")]),
        1
    );
    assert_eq!(
        meter.counter_value("auth.refresh.events", &[("outcome", "reuse")]),
        1
    );
    // The reuse revoked the rotated token's family.
    assert!(service.rotate(&rotated.token).await.is_err());
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}
