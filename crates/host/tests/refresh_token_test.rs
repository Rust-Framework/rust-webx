//! 2.5 M2 — refresh token rotation, reuse detection and family revocation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use webx_core::error::{Error, Result};
use webx_core::metrics::MemoryMeter;
use webx_core::security::{
    IRefreshTokenStore, ISessionStore, InMemoryRefreshTokenStore, RefreshTokenRecord,
    RefreshTokenStatus,
};
use webx_host::refresh::{revoke_session_and_family, RefreshTokenService, RotateOutcome};

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

/// Upstream feedback §20: a reuse must identify the family so the caller can
/// revoke the matching session row (otherwise the leaked access token stays
/// valid until it expires).
#[tokio::test]
async fn rotate_outcome_reports_the_reused_family() {
    let service = service();
    let first = service.issue("user-1", None, None).await.expect("issue");

    let issued = match service
        .rotate_outcome(&first.token)
        .await
        .expect("rotate outcome")
    {
        RotateOutcome::Rotated(issued) => issued,
        other => panic!("expected Rotated, got {other:?}"),
    };
    assert_eq!(issued.family_id, first.family_id);

    match service
        .rotate_outcome(&first.token)
        .await
        .expect("reuse outcome")
    {
        RotateOutcome::Reused { family_id } => {
            assert_eq!(family_id, first.family_id, "the family must be reported");
        }
        other => panic!("expected Reused, got {other:?}"),
    }

    // The family is revoked by the reuse path, so even the legitimate
    // replacement token is now reported as Revoked (not as a silent success).
    assert_eq!(
        service
            .rotate_outcome(&issued.token)
            .await
            .expect("revoked outcome"),
        RotateOutcome::Revoked
    );

    // Malformed input is an expected failure value, not a backend error.
    assert_eq!(
        service
            .rotate_outcome("not-a-refresh-token")
            .await
            .expect("malformed outcome"),
        RotateOutcome::Invalid
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

/// Records every `revoke_by_refresh_family` call and counts each family once,
/// so a repeated call reports `0` like a real idempotent store would.
struct RecordingSessionStore {
    families: Mutex<Vec<String>>,
    sessions_per_family: u64,
}

impl RecordingSessionStore {
    fn new(sessions_per_family: u64) -> Self {
        Self {
            families: Mutex::new(Vec::new()),
            sessions_per_family,
        }
    }

    fn calls(&self) -> Vec<String> {
        self.families
            .lock()
            .expect("recording session store lock")
            .clone()
    }
}

#[async_trait]
impl ISessionStore for RecordingSessionStore {
    async fn revoke(&self, _jti: &str) -> Result<()> {
        Ok(())
    }

    async fn is_revoked(&self, _jti: &str) -> Result<bool> {
        Ok(false)
    }

    async fn revoke_by_refresh_family(&self, refresh_family_id: &str) -> Result<u64> {
        let mut families = self.families.lock().expect("recording session store lock");
        if families.iter().any(|seen| seen == refresh_family_id) {
            return Ok(0);
        }
        families.push(refresh_family_id.to_string());
        Ok(self.sessions_per_family)
    }

    fn supports_refresh_family_revocation(&self) -> bool {
        true
    }
}

/// Declares no refresh-family capability and keeps the trait's default
/// `revoke_by_refresh_family` (501).
struct UnsupportedSessionStore;

#[async_trait]
impl ISessionStore for UnsupportedSessionStore {
    async fn revoke(&self, _jti: &str) -> Result<()> {
        Ok(())
    }

    async fn is_revoked(&self, _jti: &str) -> Result<bool> {
        Ok(false)
    }
}

/// A store whose session backend is down (not "unsupported"): it declares the
/// capability, so its failure must propagate.
struct FailingSessionStore;

#[async_trait]
impl ISessionStore for FailingSessionStore {
    async fn revoke(&self, _jti: &str) -> Result<()> {
        Err(Error::internal("session backend down".to_string()))
    }

    async fn is_revoked(&self, _jti: &str) -> Result<bool> {
        Err(Error::internal("session backend down".to_string()))
    }

    async fn revoke_by_refresh_family(&self, _refresh_family_id: &str) -> Result<u64> {
        Err(Error::internal("session backend down".to_string()))
    }

    fn supports_refresh_family_revocation(&self) -> bool {
        true
    }
}

/// A store that **declares** refresh-family support but answers `501` (a real
/// backend fault, not a missing capability).
struct DeclaredButFiveOhOneSessionStore;

#[async_trait]
impl ISessionStore for DeclaredButFiveOhOneSessionStore {
    async fn revoke(&self, _jti: &str) -> Result<()> {
        Ok(())
    }

    async fn is_revoked(&self, _jti: &str) -> Result<bool> {
        Ok(false)
    }

    async fn revoke_by_refresh_family(&self, _refresh_family_id: &str) -> Result<u64> {
        Err(Error::status(
            501,
            "the session index query is not implemented by this backend build".to_string(),
        ))
    }

    fn supports_refresh_family_revocation(&self) -> bool {
        true
    }
}

fn paired_service() -> (Arc<dyn IRefreshTokenStore>, RefreshTokenService) {
    let store: Arc<dyn IRefreshTokenStore> = Arc::new(InMemoryRefreshTokenStore::new());
    let service = RefreshTokenService::new(
        store.clone(),
        Duration::from_secs(3600),
        Duration::from_secs(24 * 3600),
    );
    (store, service)
}

#[tokio::test]
async fn paired_revocation_revokes_family_and_sessions() {
    let (store, service) = paired_service();
    let recording = Arc::new(RecordingSessionStore::new(2));
    let sessions: Arc<dyn ISessionStore> = recording.clone();

    let first = service.issue("user-1", None, None).await.expect("issue");
    let second = service.rotate(&first.token).await.expect("rotate");
    assert_eq!(second.family_id, first.family_id);

    let outcome = revoke_session_and_family(&store, Some(&sessions), &first.family_id)
        .await
        .expect("paired revocation");
    assert_eq!(
        outcome.refresh_tokens_revoked, 2,
        "both the used token and the active sibling are revoked"
    );
    assert_eq!(outcome.sessions_revoked, Some(2));
    assert_eq!(recording.calls(), vec![first.family_id.clone()]);
    assert!(
        service.rotate(&second.token).await.is_err(),
        "no sibling of the family may rotate"
    );

    let retry = revoke_session_and_family(&store, Some(&sessions), &first.family_id)
        .await
        .expect("retry");
    assert_eq!(
        retry.refresh_tokens_revoked, 0,
        "family revocation is idempotent"
    );
    assert_eq!(retry.sessions_revoked, Some(0), "nothing left to revoke");
    assert_eq!(
        recording.calls(),
        vec![first.family_id.clone()],
        "the retry still asks the session store"
    );
}

#[tokio::test]
async fn unsupported_session_store_keeps_the_refresh_revocation() {
    // No capability declaration (the default `false`): the session half is
    // skipped explicitly, before any `revoke_by_refresh_family` call.
    let (store, service) = paired_service();
    let sessions: Arc<dyn ISessionStore> = Arc::new(UnsupportedSessionStore);

    let issued = service.issue("user-1", None, None).await.expect("issue");
    let outcome = revoke_session_and_family(&store, Some(&sessions), &issued.family_id)
        .await
        .expect("a declared capability gap is not an error");
    assert_eq!(outcome.refresh_tokens_revoked, 1);
    assert_eq!(
        outcome.sessions_revoked, None,
        "unsupported mapping is None, never a silent Some(0)"
    );
    assert!(
        service.rotate(&issued.token).await.is_err(),
        "the refresh family is revoked even without session mapping"
    );
}

/// Upstream feedback §20: a store that *declares* support and then fails must
/// not be downgraded to "unsupported" just because the failure is a 501.
#[tokio::test]
async fn a_real_session_store_501_is_not_mistaken_for_unsupported() {
    let (store, service) = paired_service();
    let sessions: Arc<dyn ISessionStore> = Arc::new(DeclaredButFiveOhOneSessionStore);

    let issued = service.issue("user-1", None, None).await.expect("issue");
    let error = revoke_session_and_family(&store, Some(&sessions), &issued.family_id)
        .await
        .expect_err("a declared-capable store's 501 is a failure, not a capability gap");
    assert_eq!(
        error.status_code(),
        501,
        "the backend error is returned as-is: {error:?}"
    );

    // The refresh family was revoked before the session half failed.
    assert_eq!(
        service
            .rotate_outcome(&issued.token)
            .await
            .expect("revoked outcome"),
        RotateOutcome::Revoked
    );
}

#[tokio::test]
async fn session_store_error_propagates() {
    let (store, service) = paired_service();
    let sessions: Arc<dyn ISessionStore> = Arc::new(FailingSessionStore);

    let issued = service.issue("user-1", None, None).await.expect("issue");
    let error = revoke_session_and_family(&store, Some(&sessions), &issued.family_id)
        .await
        .expect_err("a real store failure must not be swallowed");
    assert_eq!(error.status_code(), 500);
    assert_eq!(error.code(), "internal_error");

    // The refresh family was revoked before the session half failed.
    assert_eq!(
        service
            .rotate_outcome(&issued.token)
            .await
            .expect("revoked outcome"),
        RotateOutcome::Revoked
    );
}

#[tokio::test]
async fn blank_family_id_is_a_validation_error() {
    let (store, _service) = paired_service();
    let recording = Arc::new(RecordingSessionStore::new(1));
    let sessions: Arc<dyn ISessionStore> = recording.clone();

    for blank in ["", " ", " \t\n "] {
        let error = revoke_session_and_family(&store, Some(&sessions), blank)
            .await
            .expect_err("a blank family id identifies nothing");
        assert_eq!(error.code(), "validation_failed", "{error:?}");
        assert_eq!(error.status_code(), 400, "not a 500: {error:?}");
    }
    assert!(
        recording.calls().is_empty(),
        "validation happens before any I/O"
    );
}

/// Full reuse flow: `rotate_outcome` reports `Reused { family_id }`, the helper
/// revokes the paired session, and the replayed refresh token can no longer
/// rotate.
#[tokio::test]
async fn reuse_then_paired_revocation_blocks_the_old_refresh_token() {
    let (store, service) = paired_service();
    let sessions: Arc<dyn ISessionStore> = Arc::new(RecordingSessionStore::new(1));

    let first = service.issue("user-1", None, None).await.expect("issue");
    let rotated = service.rotate(&first.token).await.expect("rotate");

    let family_id = match service.rotate_outcome(&first.token).await.expect("replay") {
        RotateOutcome::Reused { family_id } => family_id,
        other => panic!("expected Reused, got {other:?}"),
    };
    assert_eq!(family_id, first.family_id);

    let outcome = revoke_session_and_family(&store, Some(&sessions), &family_id)
        .await
        .expect("paired revocation");
    // `rotate_outcome` already revoked the family on reuse.
    assert_eq!(outcome.refresh_tokens_revoked, 0);
    assert_eq!(
        outcome.sessions_revoked,
        Some(1),
        "the paired session is revoked even when the family was already dead"
    );

    match service.rotate(&first.token).await {
        Ok(_) => panic!("the replayed token must not rotate after the family is revoked"),
        Err(error) => assert_eq!(error.status_code(), 401),
    }
    assert_eq!(
        service
            .rotate_outcome(&first.token)
            .await
            .expect("still revoked"),
        RotateOutcome::Revoked
    );
    assert!(service.rotate(&rotated.token).await.is_err());
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}
