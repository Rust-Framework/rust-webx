//! JWKS-backed JWT verification keys (RS256/ES256/PS256) and `kid` rotation.
//!
//! The verifier resolves a token header's `kid` through [`IJwtKeyResolver`].
//! [`JwksResolver`] caches a remote JWKS for a bounded TTL, refreshes it once
//! for an unknown `kid` (single-flight), and fails closed with `503` when the
//! document cannot be fetched or contains no usable signing keys.
//!
//! Security properties:
//!
//! * HTTPS only; the fetch uses [`webx_client::HttpClient`], which keeps
//!   the default SSRF guard (private/link-local/metadata addresses blocked).
//! * the document size is bounded before parsing; the key count is capped;
//!   unknown `kid` is an authentication failure, not a fetch loop.
//! * keys whose `use` is not `sig`, whose `alg` does not match their `kty`, or
//!   whose curve is not P-256 are skipped instead of weakening verification.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use jsonwebtoken::{Algorithm, DecodingKey};
use serde::Deserialize;
use sha2::Digest as _;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};
use webx_core::config::JwtJwksSection;
use webx_core::error::{Error, Result};

use crate::health::{HealthStatus, IHealthCheck};

/// Maximum accepted JWKS document size.
pub const JWKS_MAX_DOCUMENT_BYTES: usize = 512 * 1024;

/// Port for resolving a token's signing key.
#[async_trait]
pub trait IJwtKeyResolver: Send + Sync {
    /// Resolve `kid` for `alg`.
    ///
    /// `401` means "unknown/invalid key" (the credential is not authentic);
    /// `503` means the key backend is unavailable (fail closed, do not treat
    /// the request as anonymous).
    async fn resolve(&self, kid: &str, alg: Algorithm) -> Result<Arc<DecodingKey>>;
}

/// A JWK set document (RFC 7517).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct JwkSet {
    #[serde(default)]
    pub keys: Vec<Jwk>,
}

/// One JSON Web Key. Fields not needed for signature verification are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    /// `RSA` or `EC` (other key types are ignored).
    pub kty: String,
    /// Key id referenced by the token header.
    #[serde(default)]
    pub kid: Option<String>,
    /// Optional algorithm restriction.
    #[serde(default)]
    pub alg: Option<String>,
    /// `use` must be absent or `sig`.
    #[serde(default, rename = "use")]
    pub use_: Option<String>,
    /// RSA modulus (base64url).
    #[serde(default)]
    pub n: Option<String>,
    /// RSA exponent (base64url).
    #[serde(default)]
    pub e: Option<String>,
    /// EC x coordinate (base64url).
    #[serde(default)]
    pub x: Option<String>,
    /// EC y coordinate (base64url).
    #[serde(default)]
    pub y: Option<String>,
    /// EC curve; only `P-256` is accepted.
    #[serde(default)]
    pub crv: Option<String>,
}

/// A parsed verification key.
#[derive(Clone)]
pub struct ResolvedJwk {
    /// Key id (empty when the JWK has no `kid`).
    pub kid: String,
    /// Algorithm bound to the key.
    pub alg: Algorithm,
    /// Verification key.
    pub key: Arc<DecodingKey>,
}

/// Parse a JWK set into usable signature keys, skipping unusable entries.
pub fn parse_jwk_set(set: &JwkSet, max_keys: usize) -> Result<Vec<ResolvedJwk>> {
    let mut keys = Vec::new();
    for jwk in set.keys.iter().take(max_keys) {
        if jwk.use_.as_deref().is_some_and(|value| value != "sig") {
            continue;
        }
        // A multi-key set must identify keys: an empty-kid entry would be
        // ambiguous and could shadow the intended key.
        if set.keys.len() > 1 && jwk.kid.is_none() {
            continue;
        }
        let Some((alg, key)) = jwk.to_decoding_key() else {
            continue;
        };
        keys.push(ResolvedJwk {
            kid: jwk.kid.clone().unwrap_or_default(),
            alg,
            key: Arc::new(key),
        });
    }
    if keys.is_empty() {
        return Err(Error::status(
            503,
            "JWKS contains no usable signing keys".to_string(),
        ));
    }
    Ok(keys)
}

impl Jwk {
    /// Build the verification key and its algorithm, when usable.
    ///
    /// Public so sender-constrained schemes (DPoP proofs) can verify the
    /// embedded `jwk` header with the same rules as JWKS keys.
    pub fn decoding_key(&self) -> Option<(Algorithm, DecodingKey)> {
        self.to_decoding_key()
    }

    /// RFC 7638 JWK thumbprint (base64url SHA-256).
    ///
    /// Only `RSA` and `EC P-256` keys are supported; other key types return
    /// `None`.
    pub fn thumbprint(&self) -> Option<String> {
        let canonical = match self.kty.as_str() {
            "RSA" => format!(
                "{{\"e\":\"{}\",\"kty\":\"RSA\",\"n\":\"{}\"}}",
                self.e.as_deref()?,
                self.n.as_deref()?
            ),
            "EC" if self.crv.as_deref() == Some("P-256") => format!(
                "{{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"{}\",\"y\":\"{}\"}}",
                self.x.as_deref()?,
                self.y.as_deref()?
            ),
            _ => return None,
        };
        let digest = sha2::Sha256::digest(canonical.as_bytes());
        Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest))
    }

    fn to_decoding_key(&self) -> Option<(Algorithm, DecodingKey)> {
        let algorithm = self.algorithm()?;
        match self.kty.as_str() {
            "RSA" => {
                let n = self.n.as_deref()?;
                let e = self.e.as_deref()?;
                let key = DecodingKey::from_rsa_components(n, e).ok()?;
                Some((algorithm, key))
            }
            "EC" => {
                if self.crv.as_deref() != Some("P-256") {
                    return None;
                }
                let x = self.x.as_deref()?;
                let y = self.y.as_deref()?;
                let key = DecodingKey::from_ec_components(x, y).ok()?;
                Some((algorithm, key))
            }
            _ => None,
        }
    }

    fn algorithm(&self) -> Option<Algorithm> {
        let declared = self.alg.as_deref();
        let inferred = match self.kty.as_str() {
            "RSA" => Algorithm::RS256,
            "EC" => Algorithm::ES256,
            _ => return None,
        };
        let algorithm = match declared {
            None => inferred,
            Some("RS256") => Algorithm::RS256,
            Some("RS384") => Algorithm::RS384,
            Some("RS512") => Algorithm::RS512,
            Some("ES256") => Algorithm::ES256,
            Some("ES384") => Algorithm::ES384,
            Some("PS256") => Algorithm::PS256,
            Some("PS384") => Algorithm::PS384,
            Some("PS512") => Algorithm::PS512,
            Some(_) => return None,
        };
        // Never let a declared algorithm contradict the key type.
        let compatible = match self.kty.as_str() {
            "RSA" => matches!(
                algorithm,
                Algorithm::RS256
                    | Algorithm::RS384
                    | Algorithm::RS512
                    | Algorithm::PS256
                    | Algorithm::PS384
                    | Algorithm::PS512
            ),
            "EC" => matches!(algorithm, Algorithm::ES256 | Algorithm::ES384),
            _ => false,
        };
        compatible.then_some(algorithm)
    }
}

/// Fetch a JWKS document.
#[async_trait]
pub trait IJwksFetcher: Send + Sync {
    /// Fetch and parse the current document.
    async fn fetch(&self) -> Result<JwkSet>;
}

/// HTTPS JWKS fetcher with bounded size/timeouts and the client's SSRF guard.
pub struct HttpJwksFetcher {
    client: HttpClient,
}

impl HttpJwksFetcher {
    /// Build a fetcher for an absolute `https://` URL.
    pub fn new(url: &str, timeout: Duration, allow_private_networks: bool) -> Result<Self> {
        if !url.starts_with("https://") {
            return Err(Error::di(format!(
                "Jwt.Jwks.Url must be https:// (got '{url}')"
            )));
        }
        let mut options = HttpClientOptions::new(url);
        options.connect_timeout = timeout.min(Duration::from_secs(3));
        options.request_timeout = Some(timeout);
        options.read_timeout = Some(timeout);
        options.pool_max_idle_per_host = 1;
        options.allow_private_networks = allow_private_networks;
        let client = HttpClient::new("jwks", options)
            .map_err(|error| Error::di(format!("invalid Jwt.Jwks.Url: {error}")))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl IJwksFetcher for HttpJwksFetcher {
    async fn fetch(&self) -> Result<JwkSet> {
        let response = self
            .client
            .execute(OutboundRequest::get(""))
            .await
            .map_err(|error| Error::status(503, format!("JWKS fetch failed: {error}")))?;
        if !response.is_success() {
            return Err(Error::status(
                503,
                format!("JWKS endpoint returned HTTP {}", response.status),
            ));
        }
        let body = response
            .body
            .collect(JWKS_MAX_DOCUMENT_BYTES)
            .await
            .map_err(|error| Error::status(503, format!("JWKS body failed: {error}")))?;
        serde_json::from_slice(&body)
            .map_err(|error| Error::status(503, format!("invalid JWKS document: {error}")))
    }
}

struct CachedJwks {
    keys: Vec<ResolvedJwk>,
    expires_at: Instant,
}

/// Caching, single-flight JWKS resolver.
pub struct JwksResolver {
    fetcher: Arc<dyn IJwksFetcher>,
    ttl: Duration,
    max_keys: usize,
    cache: RwLock<Option<CachedJwks>>,
    refresh: tokio::sync::Mutex<()>,
    /// When the last unknown-`kid` refresh was attempted.
    ///
    /// An unknown `kid` is unauthenticated input: without this gate every such
    /// request would trigger an upstream fetch and the single-flight mutex would
    /// serialize authentication behind it (identity-provider DoS amplification).
    last_unknown_kid_refresh: std::sync::Mutex<Option<Instant>>,
    /// Minimum spacing between two unknown-`kid` refreshes.
    min_refresh_interval: Duration,
}

/// Default spacing between two unknown-`kid` JWKS refreshes.
pub const DEFAULT_JWKS_MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

impl JwksResolver {
    /// Build from a fetcher; `ttl` is clamped to at least one second.
    pub fn new(fetcher: Arc<dyn IJwksFetcher>, ttl: Duration, max_keys: usize) -> Self {
        Self {
            fetcher,
            ttl: ttl.max(Duration::from_secs(1)),
            max_keys: max_keys.clamp(1, 256),
            cache: RwLock::new(None),
            refresh: tokio::sync::Mutex::new(()),
            last_unknown_kid_refresh: std::sync::Mutex::new(None),
            min_refresh_interval: DEFAULT_JWKS_MIN_REFRESH_INTERVAL,
        }
    }

    /// Set the minimum spacing between unknown-`kid` refreshes (`0` disables).
    pub fn with_min_refresh_interval(mut self, interval: Duration) -> Self {
        self.min_refresh_interval = interval;
        self
    }

    /// Build from `Jwt.Jwks` configuration.
    pub fn from_section(section: &JwtJwksSection) -> Result<Self> {
        let timeout = Duration::from_millis(section.timeout_ms);
        let fetcher = Arc::new(HttpJwksFetcher::new(
            &section.url,
            timeout,
            section.allow_private_networks,
        )?);
        Ok(Self::new(
            fetcher,
            Duration::from_secs(section.cache_seconds),
            section.max_keys,
        ))
    }

    fn lookup(&self, kid: &str, alg: Algorithm) -> Option<Arc<DecodingKey>> {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        let cached = cache.as_ref()?;
        if Instant::now() >= cached.expires_at {
            return None;
        }
        if kid.is_empty() {
            // RFC 7515 `kid` is optional. With exactly one key for this
            // algorithm we can safely infer it; multiple candidates are
            // ambiguous and must fail closed rather than guess.
            let mut candidates = cached.keys.iter().filter(|key| key.alg == alg);
            let first = candidates.next()?;
            if candidates.next().is_some() {
                return None;
            }
            return Some(Arc::clone(&first.key));
        }
        cached
            .keys
            .iter()
            .find(|key| key.kid == kid && key.alg == alg)
            .map(|key| Arc::clone(&key.key))
    }

    /// Whether the cached document is still within its TTL.
    pub fn cache_is_fresh(&self) -> bool {
        let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
        cache
            .as_ref()
            .is_some_and(|cached| Instant::now() < cached.expires_at)
    }

    /// Refresh the JWKS if the cache is stale; used by readiness probes.
    pub async fn ensure_fresh(&self) -> Result<()> {
        if self.cache_is_fresh() {
            return Ok(());
        }
        let _guard = self.refresh.lock().await;
        if self.cache_is_fresh() {
            return Ok(());
        }
        self.refresh().await
    }

    async fn refresh(&self) -> Result<()> {
        let set = self.fetcher.fetch().await?;
        let keys = parse_jwk_set(&set, self.max_keys)?;
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        *cache = Some(CachedJwks {
            keys,
            expires_at: Instant::now() + self.ttl,
        });
        Ok(())
    }
}

#[async_trait]
impl IJwtKeyResolver for JwksResolver {
    async fn resolve(&self, kid: &str, alg: Algorithm) -> Result<Arc<DecodingKey>> {
        if let Some(key) = self.lookup(kid, alg) {
            return Ok(key);
        }
        // Single-flight: concurrent requests for an unknown kid share one
        // fetch instead of stampeding the identity provider.
        let _guard = self.refresh.lock().await;
        if let Some(key) = self.lookup(kid, alg) {
            return Ok(key);
        }
        // Unknown `kid` is attacker-controlled input: bound how often it can
        // reach the identity provider (see `last_unknown_kid_refresh`). The gate
        // only applies once a document exists — a cold resolver must fetch.
        let has_document = self
            .cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some();
        if has_document && self.min_refresh_interval > Duration::ZERO {
            let mut last = self
                .last_unknown_kid_refresh
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let now = Instant::now();
            if let Some(previous) = *last {
                if now.duration_since(previous) < self.min_refresh_interval {
                    return Err(Error::unauthorized(format!(
                        "unknown JWKS key id '{kid}' for {alg:?}"
                    )));
                }
            }
            *last = Some(now);
        }
        self.refresh().await?;
        self.lookup(kid, alg)
            .ok_or_else(|| Error::unauthorized(format!("unknown JWKS key id '{kid}' for {alg:?}")))
    }
}

/// Readiness probe for a [`JwksResolver`].
///
/// Refreshes a stale cache (the check itself is the probe) and reports
/// `fail` when the document cannot be fetched, so `/health/ready` reflects an
/// unreachable identity provider.
pub struct JwksHealthCheck {
    resolver: Arc<JwksResolver>,
}

impl JwksHealthCheck {
    /// Wrap a resolver.
    pub fn new(resolver: Arc<JwksResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl IHealthCheck for JwksHealthCheck {
    fn name(&self) -> &str {
        "jwks"
    }

    async fn check(&self) -> HealthStatus {
        match self.resolver.ensure_fresh().await {
            Ok(()) => HealthStatus::pass(),
            Err(error) => HealthStatus::fail(format!("JWKS unavailable: {error}")),
        }
    }
}

/// Static resolver for tests and pinned-key deployments.
#[derive(Default)]
pub struct StaticKeyResolver {
    keys: Vec<ResolvedJwk>,
}

impl StaticKeyResolver {
    /// Empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a raw PEM public key for `kid`/`alg`.
    pub fn with_pem(mut self, kid: impl Into<String>, alg: Algorithm, pem: &[u8]) -> Result<Self> {
        let key = match alg {
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 => {
                DecodingKey::from_rsa_pem(pem)
                    .map_err(|error| Error::di(format!("invalid RSA public key: {error}")))?
            }
            Algorithm::ES256 | Algorithm::ES384 => DecodingKey::from_ec_pem(pem)
                .map_err(|error| Error::di(format!("invalid EC public key: {error}")))?,
            other => {
                return Err(Error::di(format!(
                    "{other:?} is not supported by StaticKeyResolver"
                )))
            }
        };
        self.keys.push(ResolvedJwk {
            kid: kid.into(),
            alg,
            key: Arc::new(key),
        });
        Ok(self)
    }
}

#[async_trait]
impl IJwtKeyResolver for StaticKeyResolver {
    async fn resolve(&self, kid: &str, alg: Algorithm) -> Result<Arc<DecodingKey>> {
        self.keys
            .iter()
            .find(|key| key.kid == kid && key.alg == alg)
            .map(|key| Arc::clone(&key.key))
            .ok_or_else(|| Error::unauthorized(format!("unknown key id '{kid}'")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_algorithm_and_curve_are_skipped() {
        let document = serde_json::json!({
            "keys": [
                // RSA key declaring an EC algorithm: never trust it.
                { "kty": "RSA", "kid": "rsa-1", "alg": "ES256", "n": "AQAB", "e": "AQAB" },
                // EC key on an unsupported curve.
                { "kty": "EC", "kid": "ec-1", "crv": "P-384", "x": "eA", "y": "eA" },
                // Symmetric keys are not accepted by the JWKS verifier.
                { "kty": "oct", "kid": "hmac-1", "k": "c2VjcmV0" }
            ]
        });
        let set: JwkSet = serde_json::from_value(document).expect("parse JWKS");
        let error = parse_jwk_set(&set, 32).err().expect("no usable keys");
        assert_eq!(error.status_code(), 503, "got {error:?}");
    }
}
