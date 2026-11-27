//! Refresh-token issuance and rotation.
//!
//! An opaque token is `token_id.secret`; the store only keeps
//! `SHA-256(secret)`. Rotation is single-use and atomic; presenting a consumed
//! token again is **reuse** and revokes the whole family (every rotation of that
//! login), so a stolen refresh token cannot keep a session alive after the
//! legitimate client rotates it.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use uuid::Uuid;
use webx_core::error::{Error, Result};
use webx_core::metrics::{Counter, IMeter};
use webx_core::security::{
    constant_time_eq, IRefreshTokenStore, RefreshRotation, RefreshTokenRecord, RefreshTokenStatus,
};

/// Refresh-token lifecycle counters (`auth.refresh.events`).
pub struct RefreshMetrics {
    issued: Arc<dyn Counter>,
    rotated: Arc<dyn Counter>,
    reused: Arc<dyn Counter>,
    expired: Arc<dyn Counter>,
    rejected: Arc<dyn Counter>,
    revoked: Arc<dyn Counter>,
}

impl RefreshMetrics {
    fn new(meter: &dyn IMeter) -> Self {
        let counter =
            |outcome: &'static str| meter.counter("auth.refresh.events", &[("outcome", outcome)]);
        Self {
            issued: counter("issued"),
            rotated: counter("rotated"),
            reused: counter("reuse"),
            expired: counter("expired"),
            rejected: counter("rejected"),
            revoked: counter("revoked_tokens"),
        }
    }
}

/// A freshly issued (or rotated) refresh token.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedRefreshToken {
    /// Opaque `token_id.secret`; return it to the client once.
    pub token: String,
    /// Public lookup id (also stored).
    pub token_id: String,
    /// Family id (one per login).
    pub family_id: String,
    /// Absolute expiry of this token (Unix seconds).
    pub expires_at_unix_secs: u64,
}

/// The raw token is a credential: never print it in a log line.
impl std::fmt::Debug for IssuedRefreshToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedRefreshToken")
            .field("token", &"<redacted>")
            .field("token_id", &self.token_id)
            .field("family_id", &self.family_id)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .finish()
    }
}

/// Outcome of [`RefreshTokenService::rotate_outcome`].
///
/// Expected failures are values, not errors: the caller decides how to answer
/// and — for [`RotateOutcome::Reused`] — can revoke the **paired session row**
/// because the outcome carries the family id. Without it an application knows
/// only "the refresh token was reused" and has to leave the leaked access token
/// valid until it expires (upstream feedback §20).
///
/// Backend failures still surface as `Err`: "the store is down" is not a
/// rotation outcome.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotateOutcome {
    /// The presented token was consumed and a replacement was issued.
    Rotated(IssuedRefreshToken),
    /// The presented token had already been rotated. The whole family has been
    /// revoked; `family_id` identifies the login whose session should also be
    /// revoked.
    Reused {
        /// Family id shared by every rotation of that login.
        family_id: String,
    },
    /// The token or its family passed its absolute expiry. The family is not
    /// revoked (a client may still hold a valid sibling).
    Expired,
    /// The token or its family was already revoked.
    Revoked,
    /// Unknown token, malformed token, or wrong secret. Nothing was revoked:
    /// an attacker who guesses a token id must not be able to log the user out.
    Invalid,
}

/// Result of [`revoke_session_and_family`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyRevocation {
    /// Refresh tokens revoked (changed to `Revoked`) in the family.
    pub refresh_tokens_revoked: u64,
    /// Sessions revoked by the paired session-store call.
    ///
    /// `None` = no session store was supplied, or the store declares no
    /// refresh-family capability
    /// ([`ISessionStore::supports_refresh_family_revocation`](webx_core::security::ISessionStore::supports_refresh_family_revocation)). `Some(0)` = the
    /// store supports the capability but nothing matched — the two are
    /// deliberately distinguishable.
    pub sessions_revoked: Option<u64>,
}

/// Issues and rotates refresh tokens over [`IRefreshTokenStore`].
pub struct RefreshTokenService {
    store: Arc<dyn IRefreshTokenStore>,
    ttl: Duration,
    family_ttl: Duration,
    metrics: Option<RefreshMetrics>,
}

impl RefreshTokenService {
    /// `ttl` bounds one token; `family_ttl` bounds the whole rotation chain
    /// (clamped to at least `ttl`).
    pub fn new(store: Arc<dyn IRefreshTokenStore>, ttl: Duration, family_ttl: Duration) -> Self {
        let ttl = ttl.max(Duration::from_secs(1));
        Self {
            store,
            ttl,
            family_ttl: family_ttl.max(ttl),
            metrics: None,
        }
    }

    /// Attach refresh-token metrics (`auth.refresh.events`).
    pub fn with_meter(mut self, meter: &dyn IMeter) -> Self {
        self.metrics = Some(RefreshMetrics::new(meter));
        self
    }

    fn record(&self, outcome: &'static str) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        match outcome {
            "issued" => metrics.issued.inc(),
            "rotated" => metrics.rotated.inc(),
            "reuse" => metrics.reused.inc(),
            "expired" => metrics.expired.inc(),
            "revoked" => metrics.revoked.inc(),
            _ => metrics.rejected.inc(),
        }
    }

    /// The store in use.
    pub fn store(&self) -> &Arc<dyn IRefreshTokenStore> {
        &self.store
    }

    /// Issue the first token of a new family.
    pub async fn issue(
        &self,
        subject: impl Into<String>,
        tenant_id: Option<String>,
        client_id: Option<String>,
    ) -> Result<IssuedRefreshToken> {
        let now = unix_now();
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(Error::validation(
                "refresh token subject must not be empty".to_string(),
            ));
        }
        let token_id = new_id();
        let secret = new_secret();
        let family_id = new_id();
        let record = RefreshTokenRecord {
            token_id: token_id.clone(),
            secret_hash: hash_secret(&secret),
            family_id: family_id.clone(),
            generation: 0,
            subject,
            tenant_id,
            client_id,
            issued_at_unix_secs: now,
            expires_at_unix_secs: now.saturating_add(self.ttl.as_secs()),
            used_at_unix_secs: None,
            status: RefreshTokenStatus::Active,
            family_created_at_unix_secs: now,
        };
        self.store.insert(record).await?;
        self.record("issued");
        Ok(IssuedRefreshToken {
            token: format!("{token_id}.{secret}"),
            token_id,
            family_id,
            expires_at_unix_secs: now.saturating_add(self.ttl.as_secs()),
        })
    }

    /// Rotate a presented refresh token, mapping expected failures to `401`.
    ///
    /// Use [`Self::rotate_outcome`] when the caller must know **which family**
    /// was reused so it can revoke the paired session row.
    ///
    /// A consumed token means reuse: the family is revoked and the caller gets
    /// `401`. A wrong secret does **not** revoke the family (otherwise an
    /// attacker who learns a token id could log the user out).
    pub async fn rotate(&self, presented: &str) -> Result<IssuedRefreshToken> {
        match self.rotate_outcome(presented).await? {
            RotateOutcome::Rotated(issued) => Ok(issued),
            RotateOutcome::Reused { .. } => Err(Error::unauthorized(
                "refresh token reuse detected; session revoked".to_string(),
            )),
            RotateOutcome::Expired => {
                Err(Error::unauthorized("refresh token has expired".to_string()))
            }
            RotateOutcome::Revoked => Err(Error::unauthorized(
                "refresh token has been revoked".to_string(),
            )),
            RotateOutcome::Invalid => Err(Error::unauthorized("invalid refresh token".to_string())),
        }
    }

    /// Rotate a presented refresh token without collapsing expected failures
    /// into `401`.
    ///
    /// On reuse the family is revoked **before** [`RotateOutcome::Reused`] is
    /// returned, and the outcome carries `family_id`: the caller can now revoke
    /// the matching session row instead of leaving the leaked access token
    /// valid until it expires. A wrong secret is [`RotateOutcome::Invalid`] and
    /// revokes nothing.
    pub async fn rotate_outcome(&self, presented: &str) -> Result<RotateOutcome> {
        let Ok((token_id, secret)) = split_token(presented) else {
            // Malformed input is the same rejection bucket as an unknown token
            // (metrics must not distinguish "how" the client failed).
            self.record("rejected");
            return Ok(RotateOutcome::Invalid);
        };
        let now = unix_now();
        let Some(record) = self.store.get(token_id).await? else {
            self.record("rejected");
            return Ok(RotateOutcome::Invalid);
        };

        if record.status == RefreshTokenStatus::Revoked {
            self.record("rejected");
            return Ok(RotateOutcome::Revoked);
        }
        if !constant_time_eq(
            hash_secret(secret).as_bytes(),
            record.secret_hash.as_bytes(),
        ) {
            self.record("rejected");
            return Ok(RotateOutcome::Invalid);
        }
        if record.status == RefreshTokenStatus::Used {
            // Reuse after a successful rotation: kill the family.
            self.store.revoke_family(&record.family_id).await?;
            self.record("reuse");
            return Ok(RotateOutcome::Reused {
                family_id: record.family_id,
            });
        }

        let family_expires = record
            .family_created_at_unix_secs
            .saturating_add(self.family_ttl.as_secs());
        if record.is_expired(now) || now >= family_expires {
            // Expiry is not revocation: do not kill a family that may still
            // contain a token the client is legitimately about to rotate.
            self.record("expired");
            return Ok(RotateOutcome::Expired);
        }

        let replacement_id = new_id();
        let replacement_secret = new_secret();
        let replacement = RefreshTokenRecord {
            token_id: replacement_id.clone(),
            secret_hash: hash_secret(&replacement_secret),
            family_id: record.family_id.clone(),
            generation: record.generation.saturating_add(1),
            subject: record.subject.clone(),
            tenant_id: record.tenant_id.clone(),
            client_id: record.client_id.clone(),
            issued_at_unix_secs: now,
            expires_at_unix_secs: now.saturating_add(self.ttl.as_secs()).min(family_expires),
            used_at_unix_secs: None,
            status: RefreshTokenStatus::Active,
            family_created_at_unix_secs: record.family_created_at_unix_secs,
        };

        match self.store.rotate(token_id, now, replacement).await? {
            RefreshRotation::Rotated => {
                self.record("rotated");
                Ok(RotateOutcome::Rotated(IssuedRefreshToken {
                    token: format!("{replacement_id}.{replacement_secret}"),
                    token_id: replacement_id,
                    family_id: record.family_id,
                    expires_at_unix_secs: now
                        .saturating_add(self.ttl.as_secs())
                        .min(family_expires),
                }))
            }
            RefreshRotation::Reused => {
                self.store.revoke_family(&record.family_id).await?;
                self.record("reuse");
                Ok(RotateOutcome::Reused {
                    family_id: record.family_id,
                })
            }
            RefreshRotation::Expired => {
                self.record("expired");
                Ok(RotateOutcome::Expired)
            }
            RefreshRotation::Revoked => {
                self.record("rejected");
                Ok(RotateOutcome::Revoked)
            }
            RefreshRotation::NotFound => {
                self.record("rejected");
                Ok(RotateOutcome::Invalid)
            }
        }
    }

    /// Revoke a presented refresh token (RFC 7009). Verifies the secret before
    /// revoking the whole family; unknown/malformed tokens return `false`.
    pub async fn revoke_token(&self, presented: &str) -> Result<bool> {
        let Ok((token_id, secret)) = split_token(presented) else {
            return Ok(false);
        };
        let Some(record) = self.store.get(token_id).await? else {
            return Ok(false);
        };
        if !constant_time_eq(
            hash_secret(secret).as_bytes(),
            record.secret_hash.as_bytes(),
        ) {
            return Ok(false);
        }
        let removed = self.revoke_family(&record.family_id).await?;
        Ok(removed > 0)
    }

    /// Revoke one family (logout of one device/session).
    pub async fn revoke_family(&self, family_id: &str) -> Result<u64> {
        let removed = self.store.revoke_family(family_id).await?;
        if let Some(metrics) = &self.metrics {
            metrics.revoked.add(removed);
        }
        Ok(removed)
    }

    /// Revoke every refresh token of `subject` ("sign out everywhere").
    pub async fn revoke_all_for_subject(&self, subject: &str) -> Result<u64> {
        let removed = self.store.revoke_all_for_subject(subject).await?;
        if let Some(metrics) = &self.metrics {
            metrics.revoked.add(removed);
        }
        Ok(removed)
    }
}

/// Revoke a refresh family **and** the sessions bound to it (paired revocation
/// after reuse detection).
///
/// Order is deliberate: [`IRefreshTokenStore::revoke_family`] runs first and
/// must succeed, because a leaked refresh token is the credential that keeps
/// minting access tokens. Only then is the session store asked to revoke the
/// sessions mapped to the same family.
///
/// Failure semantics:
///
/// * The session store's **capability declaration** decides the session half:
///   when [`ISessionStore::supports_refresh_family_revocation`](webx_core::security::ISessionStore::supports_refresh_family_revocation) is `false`, the
///   refresh family stays revoked, [`FamilyRevocation::sessions_revoked`] is
///   `None`, and a `tracing::warn!` records the gap. This is not an error —
///   the caller cannot do better.
/// * When the store declares the capability, its result is taken at face value:
///   **any** error — including a `501` — is returned as-is. Inferring
///   "unsupported" from `501` would turn a real backend failure into a silent
///   downgrade with a misleading success. The refresh family is already
///   revoked, so retrying is safe (family revocation is idempotent).
/// * A blank `family_id` is [`Error::validation`] before any I/O.
///
/// No lock is held across the two store calls, and only `family_id` (never a
/// token or secret) may appear in logs.
pub async fn revoke_session_and_family(
    refresh: &Arc<dyn IRefreshTokenStore>,
    sessions: Option<&Arc<dyn webx_core::security::ISessionStore>>,
    family_id: &str,
) -> Result<FamilyRevocation> {
    let family_id = family_id.trim();
    if family_id.is_empty() {
        return Err(Error::validation(
            "refresh family id must not be empty".to_string(),
        ));
    }

    let refresh_tokens_revoked = refresh.revoke_family(family_id).await?;

    let Some(sessions) = sessions else {
        return Ok(FamilyRevocation {
            refresh_tokens_revoked,
            sessions_revoked: None,
        });
    };

    // Declared capability, not an error code: a store that does not support the
    // family index is skipped explicitly, while every failure of a store that
    // does support it propagates.
    if !sessions.supports_refresh_family_revocation() {
        tracing::warn!(
            family_id,
            "ISessionStore does not support refresh-family revocation; the refresh family is revoked but its sessions are not"
        );
        return Ok(FamilyRevocation {
            refresh_tokens_revoked,
            sessions_revoked: None,
        });
    }

    let sessions_revoked = sessions.revoke_by_refresh_family(family_id).await?;
    Ok(FamilyRevocation {
        refresh_tokens_revoked,
        sessions_revoked: Some(sessions_revoked),
    })
}

fn split_token(presented: &str) -> Result<(&str, &str)> {
    let (token_id, secret) = presented
        .split_once('.')
        .ok_or_else(|| Error::unauthorized("invalid refresh token".to_string()))?;
    if token_id.is_empty() || secret.is_empty() {
        return Err(Error::unauthorized("invalid refresh token".to_string()));
    }
    Ok((token_id, secret))
}

fn new_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn new_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_ids_and_secrets_are_hex_and_the_hash_is_not_the_secret() {
        let secret = new_secret();
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
        let hash = hash_secret(&secret);
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, secret);
        assert_ne!(new_id(), new_id());
    }
}
