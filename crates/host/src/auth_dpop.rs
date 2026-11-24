//! DPoP (RFC 9449): sender-constrained access tokens.
//!
//! The access token is still a JWT (validated by [`JwtAuth`]); the client also
//! sends a `DPoP` proof JWT signed by a key it holds. The proof must:
//!
//! * use `typ: "dpop+jwt"`, an asymmetric `alg`, and embed its public `jwk`;
//! * match the request (`htm`/`htu`), be fresh (`iat`), carry a unique `jti`
//!   and the access-token hash (`ath`);
//! * be signed by the same key whose RFC 7638 thumbprint appears in the access
//!   token's `cnf.jkt` claim.
//!
//! The replay store rejects a `jti` seen within the freshness window.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use jsonwebtoken::{decode, Algorithm, Validation};
use serde::Deserialize;
use serde_json::Value;
use sha2::Digest as _;
use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::security::constant_time_eq;

use crate::auth_jwks::Jwk;
use crate::auth_jwt::JwtAuth;

/// Replay protection for DPoP proof ids.
#[async_trait]
pub trait IDpopReplayStore: Send + Sync {
    /// Record `jti` until `expires_at_unix_secs`; `false` means replay.
    async fn check_and_record(&self, jti: &str, expires_at_unix_secs: u64) -> Result<bool>;
}

/// Process-local DPoP replay store.
#[derive(Debug, Default)]
pub struct InMemoryDpopReplayStore {
    entries: Mutex<HashMap<String, u64>>,
    max_entries: usize,
}

impl InMemoryDpopReplayStore {
    /// Create a store with a 100,000-entry cap.
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max_entries: 100_000,
        }
    }

    /// Override the entry cap.
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }
}

#[async_trait]
impl IDpopReplayStore for InMemoryDpopReplayStore {
    async fn check_and_record(&self, jti: &str, expires_at_unix_secs: u64) -> Result<bool> {
        if jti.is_empty() {
            return Err(Error::validation("DPoP jti must not be empty".to_string()));
        }
        let now = unix_now();
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.len() >= self.max_entries {
            entries.retain(|_, expires_at| *expires_at > now);
            if entries.len() >= self.max_entries {
                entries.clear();
            }
        }
        if entries.get(jti).is_some_and(|expires_at| *expires_at > now) {
            return Ok(false);
        }
        entries.insert(jti.to_string(), expires_at_unix_secs);
        Ok(true)
    }
}

/// DPoP scheme configuration.
#[derive(Debug, Clone)]
pub struct DpopConfig {
    /// Public origin the proof's `htu` must match, e.g. `https://api.example.com`.
    pub expected_origin: String,
    /// Maximum proof age (1–600s).
    pub max_age: Duration,
    /// Accepted proof algorithms (asymmetric only).
    pub algorithms: Vec<Algorithm>,
}

impl DpopConfig {
    /// Configuration for `origin` with ES256/RS256 and a 5-minute window.
    pub fn new(origin: impl Into<String>) -> Result<Self> {
        let config = Self {
            expected_origin: origin.into(),
            max_age: Duration::from_secs(300),
            algorithms: vec![Algorithm::ES256, Algorithm::RS256],
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate fail-closed startup constraints.
    pub fn validate(&self) -> Result<()> {
        if !self.expected_origin.starts_with("https://") {
            return Err(Error::di(
                "DPoP expected_origin must be https://".to_string(),
            ));
        }
        if self.max_age.is_zero() || self.max_age > Duration::from_secs(600) {
            return Err(Error::di(
                "DPoP max_age must be between 1 and 600 seconds".to_string(),
            ));
        }
        if self.algorithms.is_empty()
            || self.algorithms.iter().any(|algorithm| {
                matches!(
                    algorithm,
                    Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
                )
            })
        {
            return Err(Error::di(
                "DPoP algorithms must be non-empty and asymmetric".to_string(),
            ));
        }
        Ok(())
    }
}

/// `Authorization: DPoP …` + `DPoP` proof scheme.
pub struct DpopScheme {
    jwt_auth: Arc<JwtAuth>,
    replay: Arc<dyn IDpopReplayStore>,
    config: DpopConfig,
}

impl DpopScheme {
    /// Build the scheme; validates [`DpopConfig`].
    pub fn new(
        jwt_auth: Arc<JwtAuth>,
        replay: Arc<dyn IDpopReplayStore>,
        config: DpopConfig,
    ) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            jwt_auth,
            replay,
            config,
        })
    }

    async fn validate_proof(
        &self,
        claims: &dyn IClaims,
        access_token: &str,
        proof: &str,
        method: &str,
        path: &str,
    ) -> Result<()> {
        let header_part = proof
            .split_once('.')
            .map(|(header, _)| header)
            .ok_or_else(|| Error::unauthorized("invalid DPoP proof".to_string()))?;
        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(header_part)
            .map_err(|_| Error::unauthorized("invalid DPoP proof header".to_string()))?;
        let header: Value = serde_json::from_slice(&header_bytes)
            .map_err(|_| Error::unauthorized("invalid DPoP proof header".to_string()))?;

        if header.get("typ").and_then(Value::as_str) != Some("dpop+jwt") {
            return Err(Error::unauthorized(
                "DPoP proof typ must be dpop+jwt".to_string(),
            ));
        }
        let algorithm = header
            .get("alg")
            .and_then(Value::as_str)
            .and_then(parse_algorithm)
            .ok_or_else(|| Error::unauthorized("unsupported DPoP proof algorithm".to_string()))?;
        if !self.config.algorithms.contains(&algorithm) {
            return Err(Error::unauthorized(
                "DPoP proof algorithm is not allowed".to_string(),
            ));
        }
        let jwk_value = header
            .get("jwk")
            .cloned()
            .ok_or_else(|| Error::unauthorized("DPoP proof is missing jwk".to_string()))?;
        let jwk: Jwk = serde_json::from_value(jwk_value)
            .map_err(|_| Error::unauthorized("invalid DPoP jwk".to_string()))?;
        let (jwk_algorithm, key) = jwk
            .decoding_key()
            .ok_or_else(|| Error::unauthorized("unsupported DPoP jwk".to_string()))?;
        if jwk_algorithm != algorithm {
            return Err(Error::unauthorized(
                "DPoP jwk does not match the proof algorithm".to_string(),
            ));
        }
        let thumbprint = jwk
            .thumbprint()
            .ok_or_else(|| Error::unauthorized("unsupported DPoP jwk".to_string()))?;

        let confirmation = claims
            .claims()
            .get("cnf")
            .ok_or_else(|| Error::unauthorized("access token is not DPoP-bound".to_string()))?;
        let confirmation: Value = serde_json::from_str(confirmation)
            .map_err(|_| Error::unauthorized("invalid cnf claim".to_string()))?;
        let expected_thumbprint = confirmation
            .get("jkt")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::unauthorized("cnf.jkt is missing".to_string()))?;
        if !constant_time_eq(expected_thumbprint.as_bytes(), thumbprint.as_bytes()) {
            return Err(Error::unauthorized(
                "DPoP key does not match the access token".to_string(),
            ));
        }

        let mut validation = Validation::new(algorithm);
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        let proof_claims = decode::<DpopClaims>(proof, &key, &validation)
            .map_err(|_| Error::unauthorized("invalid DPoP proof signature".to_string()))?
            .claims;

        if !proof_claims.htm.eq_ignore_ascii_case(method) {
            return Err(Error::unauthorized("DPoP htm mismatch".to_string()));
        }
        let expected_htu = format!(
            "{}{}",
            self.config.expected_origin.trim_end_matches('/'),
            path
        );
        if proof_claims.htu != expected_htu {
            return Err(Error::unauthorized("DPoP htu mismatch".to_string()));
        }
        let now = unix_now();
        if proof_claims.iat > now.saturating_add(60)
            || now.saturating_sub(proof_claims.iat) > self.config.max_age.as_secs()
        {
            return Err(Error::unauthorized("DPoP proof is stale".to_string()));
        }
        if proof_claims.jti.trim().is_empty() {
            return Err(Error::unauthorized("DPoP jti is missing".to_string()));
        }
        let expected_ath = access_token_hash(access_token);
        match proof_claims.ath.as_deref() {
            Some(ath) if constant_time_eq(ath.as_bytes(), expected_ath.as_bytes()) => {}
            _ => return Err(Error::unauthorized("DPoP ath mismatch".to_string())),
        }

        let expires_at = now.saturating_add(self.config.max_age.as_secs());
        if !self
            .replay
            .check_and_record(&proof_claims.jti, expires_at)
            .await?
        {
            return Err(Error::unauthorized(
                "DPoP proof has already been used".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl IAuthenticationScheme for DpopScheme {
    fn name(&self) -> &'static str {
        "dpop"
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        ctx.request()
            .header("authorization")
            .and_then(dpop_token)
            .is_some()
            && ctx.request().header("dpop").is_some()
    }

    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        let Some(token) = ctx.request().header("authorization").and_then(dpop_token) else {
            return Ok(None);
        };
        let Some(proof) = ctx.request().header("dpop") else {
            return Err(Error::unauthorized("missing DPoP proof".to_string()));
        };
        let Some(claims) = self.jwt_auth.validate_token(token).await? else {
            return Err(Error::unauthorized("invalid DPoP access token".to_string()));
        };
        self.validate_proof(
            &*claims,
            token,
            proof,
            ctx.request().method(),
            ctx.request().path(),
        )
        .await?;
        Ok(Some(claims))
    }
}

#[derive(Debug, Deserialize)]
struct DpopClaims {
    htm: String,
    htu: String,
    iat: u64,
    jti: String,
    #[serde(default)]
    ath: Option<String>,
}

fn dpop_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("DPoP") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

fn parse_algorithm(name: &str) -> Option<Algorithm> {
    match name {
        "RS256" => Some(Algorithm::RS256),
        "RS384" => Some(Algorithm::RS384),
        "RS512" => Some(Algorithm::RS512),
        "ES256" => Some(Algorithm::ES256),
        "ES384" => Some(Algorithm::ES384),
        "PS256" => Some(Algorithm::PS256),
        "PS384" => Some(Algorithm::PS384),
        "PS512" => Some(Algorithm::PS512),
        _ => None,
    }
}

fn access_token_hash(token: &str) -> String {
    let digest = sha2::Sha256::digest(token.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
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

    #[tokio::test]
    async fn replay_store_allows_once_within_the_window() {
        let store = InMemoryDpopReplayStore::new();
        let now = unix_now();
        assert!(store.check_and_record("j1", now + 60).await.unwrap());
        assert!(!store.check_and_record("j1", now + 60).await.unwrap());
        assert!(store.check_and_record("j2", now + 60).await.unwrap());
    }

    #[test]
    fn config_requires_https_and_asymmetric_algorithms() {
        assert!(DpopConfig::new("http://api.example.com").is_err());
        let mut config = DpopConfig::new("https://api.example.com").unwrap();
        config.algorithms = vec![Algorithm::HS256];
        assert!(config.validate().is_err());
    }
}
