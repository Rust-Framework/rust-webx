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

use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use jsonwebtoken::{Algorithm, DecodingKey};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use webx_client::{HttpClient, HttpClientOptions, IHttpClient, OutboundRequest};
use webx_core::config::{JwtAlgorithm, JwtJwksSection, JwtJwksSourceSection};
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

    /// Resolve the key **and** the issuer that publishes it, atomically.
    ///
    /// [`JwtAuth`](crate::auth_jwt::JwtAuth) uses this entry point: a token's
    /// `kid` is unauthenticated routing data, so the verified key has to be
    /// bound back to exactly the source that produced it. Returning the binding
    /// together with the key means no shared mutable state can invalidate it
    /// between resolution and verification.
    ///
    /// The default answers [`Self::resolve`] with no provenance, which is
    /// correct for a resolver that cannot attribute keys (a single shared JWKS,
    /// a static test key set); such tokens keep the coarse
    /// "`iss` must be in `Jwt.Issuers`" check.
    async fn resolve_bound(
        &self,
        kid: &str,
        alg: Algorithm,
    ) -> Result<(Arc<DecodingKey>, Option<Arc<str>>)> {
        Ok((self.resolve(kid, alg).await?, None))
    }
}

/// A JWK set document (RFC 7517).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct JwkSet {
    #[serde(default)]
    pub keys: Vec<Jwk>,
}

impl JwkSet {
    /// A set holding exactly one key.
    pub fn single(jwk: Jwk) -> Self {
        Self { keys: vec![jwk] }
    }

    /// Serialize into the JWKS document (`{"keys":[...]}`) served at
    /// `/.well-known/jwks.json`.
    ///
    /// Only public parameters are emitted; see [`Jwk`]'s `Serialize`.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|error| Error::di(format!("JWKS serialization failed: {error}")))
    }
}

/// One JSON Web Key. Fields not needed for signature verification are ignored.
///
/// `Serialize` emits only public key material: RSA `n`/`e` or EC `x`/`y` plus
/// `kty`/`kid`/`alg`/`crv`/`use`. A key parsed from a JWKS keeps whatever it
/// carried; a key built by [`Jwk::from_public_key_pem`] always carries
/// `use: "sig"`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Jwk {
    /// `RSA` or `EC` (other key types are ignored).
    pub kty: String,
    /// Key id referenced by the token header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    /// Optional algorithm restriction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alg: Option<String>,
    /// `use` must be absent or `sig`.
    #[serde(default, rename = "use", skip_serializing_if = "Option::is_none")]
    pub use_: Option<String>,
    /// RSA modulus (base64url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
    /// RSA exponent (base64url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e: Option<String>,
    /// EC x coordinate (base64url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// EC y coordinate (base64url).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    /// EC curve; only `P-256` is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
///
/// A `(kid, alg)` pair must identify exactly one key: duplicates (and an empty
/// `kid` in a multi-key set) are rejected rather than silently shadowed, because
/// "first match wins" would make which key verifies a token depend on document
/// order.
pub fn parse_jwk_set(set: &JwkSet, max_keys: usize) -> Result<Vec<ResolvedJwk>> {
    let mut keys = Vec::new();
    let mut seen: HashSet<(String, Algorithm)> = HashSet::new();
    for jwk in set.keys.iter().take(max_keys) {
        if jwk.use_.as_deref().is_some_and(|value| value != "sig") {
            continue;
        }
        // A multi-key set must identify keys: an empty-kid entry would be
        // ambiguous and could shadow the intended key.
        if set.keys.len() > 1 && jwk.kid.as_deref().is_none_or(str::is_empty) {
            continue;
        }
        let Some((alg, key)) = jwk.to_decoding_key() else {
            continue;
        };
        let kid = jwk.kid.clone().unwrap_or_default();
        if !seen.insert((kid.clone(), alg)) {
            return Err(Error::status(
                503,
                format!("JWKS publishes duplicate key id '{kid}' for {alg:?}"),
            ));
        }
        keys.push(ResolvedJwk {
            kid,
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
    /// Build a public JWK (`use: "sig"`) from a PEM public key.
    ///
    /// `alg` must be one of `RS256`/`ES256`/`PS256`: `HS256` has no public
    /// parameters and keeps using `Jwt.Secret`. RSA accepts SPKI
    /// (`BEGIN PUBLIC KEY`) and PKCS#1 (`BEGIN RSA PUBLIC KEY`); EC accepts a
    /// P-256 SPKI key. Private parameters are never present in the result.
    pub fn from_public_key_pem(alg: JwtAlgorithm, kid: &str, pem: &[u8]) -> Result<Jwk> {
        let kid = kid.trim();
        if kid.is_empty() {
            return Err(Error::di("Jwt.Signing.KeyId must not be empty".to_string()));
        }
        let pem = std::str::from_utf8(pem).map_err(|_| {
            Error::di("Jwt.Signing public key file is not valid UTF-8 PEM".to_string())
        })?;
        match alg {
            JwtAlgorithm::Rs256 | JwtAlgorithm::Ps256 => {
                let key = rsa_public_key_from_pem(pem)?;
                ensure_rsa_public_key_strength(&key)?;
                rsa_public_jwk(alg, kid, &key)
            }
            JwtAlgorithm::Es256 => {
                let key = p256_public_key_from_pem(pem)?;
                p256_public_jwk(kid, &key)
            }
            JwtAlgorithm::Hs256 => Err(Error::di(
                "Jwt.Signing.Algorithm must be RS256, ES256 or PS256; HS256 keeps using Jwt.Secret"
                    .to_string(),
            )),
        }
    }

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

/// Parse an RSA public key PEM (SPKI first, then PKCS#1).
fn rsa_public_key_from_pem(pem: &str) -> Result<rsa::PublicKey> {
    use rsa::pkcs8::DecodePublicKey as _;

    if let Ok(key) = rsa::PublicKey::from_public_key_pem(pem) {
        return Ok(key);
    }
    use rsa::pkcs1::DecodeRsaPublicKey as _;
    rsa::PublicKey::from_pkcs1_pem(pem).map_err(|error| {
        Error::di(format!(
            "Jwt.Signing public key is not a valid RSA public key PEM: {error}"
        ))
    })
}

/// Reject a retired/exported RSA public key weaker than the signing minimum.
///
/// The active private key is checked at load time; without the same floor here a
/// rotator could keep a 1024-bit retired key verifying tokens after it becomes
/// factorable.
fn ensure_rsa_public_key_strength(key: &rsa::PublicKey) -> Result<()> {
    use rsa::traits::PublicKeyParts as _;

    let modulus = key.n().to_be_bytes();
    let first_significant = modulus
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(modulus.len());
    let significant = &modulus[first_significant..];
    let bits = significant
        .first()
        .map(|first| significant.len() * 8 - first.leading_zeros() as usize)
        .unwrap_or(0);
    if bits < crate::auth_jwt::MIN_RSA_SIGNING_KEY_BITS {
        return Err(Error::di(format!(
            "public key has a {bits}-bit RSA modulus; at least {} bits are required",
            crate::auth_jwt::MIN_RSA_SIGNING_KEY_BITS
        )));
    }
    Ok(())
}

/// Parse a P-256 public key from its SPKI PEM.
fn p256_public_key_from_pem(pem: &str) -> Result<p256::PublicKey> {
    use p256::pkcs8::DecodePublicKey as _;

    p256::PublicKey::from_public_key_pem(pem).map_err(|error| {
        Error::di(format!(
            "Jwt.Signing public key is not a valid P-256 SPKI public key PEM: {error}"
        ))
    })
}

/// Public RSA JWK parameters (`n`, `e`) for `alg`.
pub(crate) fn rsa_public_jwk(alg: JwtAlgorithm, kid: &str, key: &rsa::PublicKey) -> Result<Jwk> {
    use rsa::traits::PublicKeyParts as _;

    Ok(Jwk {
        kty: "RSA".to_string(),
        kid: Some(kid.to_string()),
        alg: Some(alg.as_str().to_string()),
        use_: Some("sig".to_string()),
        n: Some(b64url_uint(&key.n().to_be_bytes())),
        e: Some(b64url_uint(&key.e().to_be_bytes())),
        x: None,
        y: None,
        crv: None,
    })
}

/// Public P-256 JWK parameters (`crv`, `x`, `y`).
pub(crate) fn p256_public_jwk(kid: &str, key: &p256::PublicKey) -> Result<Jwk> {
    use p256::elliptic_curve::sec1::ToSec1Point as _;

    // Uncompressed SEC1 point: 0x04 || x || y.
    let point = key.to_sec1_point(false);
    let bytes = point.as_bytes();
    if bytes.len() != 65 || bytes[0] != 0x04 {
        return Err(Error::di(
            "Jwt.Signing EC public key is not an uncompressed P-256 point".to_string(),
        ));
    }
    Ok(Jwk {
        kty: "EC".to_string(),
        kid: Some(kid.to_string()),
        alg: Some(JwtAlgorithm::Es256.as_str().to_string()),
        use_: Some("sig".to_string()),
        n: None,
        e: None,
        x: Some(b64url(&bytes[1..33])),
        y: Some(b64url(&bytes[33..65])),
        crv: Some("P-256".to_string()),
    })
}

/// base64url without padding (RFC 7515 §2).
fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url of a big-endian integer with its leading zero bytes removed, so
/// the JWK parameter is the minimal-length encoding RFC 7518 §6.3.1 expects.
fn b64url_uint(bytes: &[u8]) -> String {
    let first_significant = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len().saturating_sub(1));
    b64url(&bytes[first_significant..])
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

/// One [`JwksResolver`] per trusted issuer (`Jwt.JwksSources`).
///
/// A token's `kid` is the only routing information available **before** the
/// signature is verified; `iss` becomes trustworthy only afterwards. Resolution
/// therefore never guesses:
///
/// * a `kid` found in exactly one source resolves to that key;
/// * a `kid` found in no source is `401` (unknown credential);
/// * a `kid` published by **more than one** source is `503 ambiguous key id`
///   (fail-closed: silently picking a source would let one identity provider
///   sign tokens the other one authenticates);
/// * a broken/unreachable source is `503` for the whole request — an upstream
///   outage never degrades into "anonymous".
///
/// When sources are bound to their `Issuer` (the `Jwt.JwksSources` path),
/// [`IJwtKeyResolver::resolve_bound`] returns that issuer together with the key,
/// so [`JwtAuth`](crate::auth_jwt::JwtAuth) can reject a token whose `iss` does
/// not match the source that actually published the verifying key.
///
/// Each inner resolver keeps its own cache, single-flight refresh and
/// unknown-`kid` rate limit; sources are queried in configuration order.
pub struct MultiIssuerKeyResolver {
    sources: Vec<KeySource>,
}

struct KeySource {
    /// `Jwt.JwksSources[i].Issuer`; `None` for an unbound resolver (tests, or a
    /// single shared key set).
    issuer: Option<Arc<str>>,
    resolver: Arc<dyn IJwtKeyResolver>,
}

impl MultiIssuerKeyResolver {
    /// Wrap one resolver per source (one resolver is the ordinary case).
    ///
    /// Empty is rejected: a resolver that can never produce a key would answer
    /// `401` for every asymmetric token instead of failing at startup. Sources
    /// built here are **unbound**; call [`Self::bind_issuers`] to enable
    /// issuer-to-key binding.
    pub fn from_resolvers(resolvers: Vec<Arc<dyn IJwtKeyResolver>>) -> Result<Self> {
        if resolvers.is_empty() {
            return Err(Error::di(
                "Jwt.JwksSources must contain at least one source".to_string(),
            ));
        }
        Ok(Self {
            sources: resolvers
                .into_iter()
                .map(|resolver| KeySource {
                    issuer: None,
                    resolver,
                })
                .collect(),
        })
    }

    /// Bind each resolver, in declaration order, to the issuer that publishes
    /// its keys (`Jwt.JwksSources[i].Issuer`).
    ///
    /// The list must have exactly one non-blank entry per resolver.
    pub fn bind_issuers(mut self, issuers: Vec<String>) -> Result<Self> {
        if issuers.len() != self.sources.len() {
            return Err(Error::di(format!(
                "Jwt.JwksSources has {} source(s) but {} issuer binding(s)",
                self.sources.len(),
                issuers.len()
            )));
        }
        for (source, issuer) in self.sources.iter_mut().zip(issuers) {
            let issuer = issuer.trim();
            if issuer.is_empty() {
                return Err(Error::di(
                    "Jwt.JwksSources.Issuer must not be empty".to_string(),
                ));
            }
            source.issuer = Some(Arc::from(issuer));
        }
        Ok(self)
    }

    /// Build one [`JwksResolver`] per `Jwt.JwksSources` entry, bound to its
    /// `Issuer`.
    ///
    /// Every endpoint rule (HTTPS, cache/TTL bounds, key cap, SSRF guard) is
    /// enforced by [`JwksResolver::from_section`] before any request is served.
    pub fn from_sections(sources: &[JwtJwksSourceSection]) -> Result<Self> {
        let mut resolvers: Vec<Arc<dyn IJwtKeyResolver>> = Vec::with_capacity(sources.len());
        for source in sources {
            resolvers.push(Arc::new(JwksResolver::from_section(
                &source.jwks_section(),
            )?));
        }
        let issuers = sources.iter().map(|source| source.issuer.clone()).collect();
        Self::from_resolvers(resolvers)?.bind_issuers(issuers)
    }

    /// Number of configured sources.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether no source is configured (a built resolver always has one).
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

#[async_trait]
impl IJwtKeyResolver for MultiIssuerKeyResolver {
    async fn resolve(&self, kid: &str, alg: Algorithm) -> Result<Arc<DecodingKey>> {
        Ok(self.resolve_bound(kid, alg).await?.0)
    }

    async fn resolve_bound(
        &self,
        kid: &str,
        alg: Algorithm,
    ) -> Result<(Arc<DecodingKey>, Option<Arc<str>>)> {
        let mut hit: Option<(&KeySource, Arc<DecodingKey>)> = None;
        // Sequential on purpose: the inner resolvers already serialize their own
        // refreshes, and a fixed order keeps the reported failure deterministic.
        for source in &self.sources {
            match source.resolver.resolve(kid, alg).await {
                Ok(key) => {
                    if hit.is_some() {
                        return Err(ambiguous_key_id(kid));
                    }
                    hit = Some((source, key));
                }
                // "Not in this source" says nothing about the others, so every
                // source must be consulted before answering.
                Err(error) if error.status_code() == 401 => continue,
                // A key-backend failure is not "no key": fail closed with 503
                // instead of letting the request fall through to anonymous.
                Err(error) => {
                    tracing::warn!(
                        kid,
                        %error,
                        "[Auth] a JWT key source is unavailable; failing closed"
                    );
                    return Err(Error::unavailable(format!(
                        "JWT key source unavailable: {error}"
                    )));
                }
            }
        }
        let Some((source, key)) = hit else {
            return Err(Error::unauthorized(format!("unknown key id '{kid}'")));
        };
        // The binding travels with the key: no shared state can lose it between
        // this resolution and the post-verification `iss` check.
        Ok((key, source.issuer.as_ref().map(Arc::clone)))
    }
}

fn ambiguous_key_id(kid: &str) -> Error {
    Error::unavailable(format!(
        "ambiguous key id '{kid}': more than one Jwt.JwksSources entry publishes this key id, \
         and the issuer cannot be determined before verification"
    ))
}

/// Key resolution where the **local** signing key set is authoritative for its
/// own `kid`s, and only unknown kids fall through to the remote sources.
///
/// This is what makes `Jwt.Signing` + `Jwt.Jwks`/`Jwt.JwksSources` a coherent
/// configuration: the host signs with its active key, so it must always be able
/// to verify that key even when a partner IdP (a remote source) is down. A
/// remote source is consulted only for a `kid` the local set does not publish,
/// which keeps the multi-source fail-closed semantics for foreign tokens.
pub struct LocalFirstKeyResolver {
    local: Arc<dyn IJwtKeyResolver>,
    remote: Arc<dyn IJwtKeyResolver>,
}

impl LocalFirstKeyResolver {
    /// Chain `local` in front of `remote`.
    pub fn new(local: Arc<dyn IJwtKeyResolver>, remote: Arc<dyn IJwtKeyResolver>) -> Self {
        Self { local, remote }
    }
}

#[async_trait]
impl IJwtKeyResolver for LocalFirstKeyResolver {
    async fn resolve(&self, kid: &str, alg: Algorithm) -> Result<Arc<DecodingKey>> {
        Ok(self.resolve_bound(kid, alg).await?.0)
    }

    async fn resolve_bound(
        &self,
        kid: &str,
        alg: Algorithm,
    ) -> Result<(Arc<DecodingKey>, Option<Arc<str>>)> {
        match self.local.resolve(kid, alg).await {
            // The local key set signs this host's own tokens, so a local hit has
            // no external issuer to bind to.
            Ok(key) => Ok((key, None)),
            // Not published locally: the remote sources own this `kid`, and they
            // return their own binding.
            Err(error) if error.status_code() == 401 => self.remote.resolve_bound(kid, alg).await,
            // A local resolver is in-memory (static keys); anything else is a
            // real failure and must not fall through to "try the network".
            Err(error) => Err(error),
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
    ///
    /// `RS*` and `PS*` both use an RSA key; `ES*` uses an EC key. Any other
    /// algorithm is rejected: a resolver must never silently accept a
    /// different key family than the token header asks for.
    pub fn with_pem(mut self, kid: impl Into<String>, alg: Algorithm, pem: &[u8]) -> Result<Self> {
        let key = match alg {
            Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512 => DecodingKey::from_rsa_pem(pem)
                .map_err(|error| Error::di(format!("invalid RSA public key: {error}")))?,
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

    /// Add a public JWK (for example one published in a JWKS document).
    ///
    /// The key's `kid`/`alg` are taken from the JWK itself; a key without a
    /// `kid` is indexed under the empty id, which only an unambiguous
    /// single-key set can resolve.
    pub fn with_jwk(mut self, jwk: &Jwk) -> Result<Self> {
        self.push_jwk(jwk)?;
        Ok(self)
    }

    /// Add a public JWK in place.
    pub(crate) fn push_jwk(&mut self, jwk: &Jwk) -> Result<()> {
        if jwk.use_.as_deref().is_some_and(|value| value != "sig") {
            return Err(Error::di(format!(
                "JWK '{}' is not a signing key (use != sig)",
                jwk.kid.as_deref().unwrap_or("")
            )));
        }
        let Some((alg, key)) = jwk.decoding_key() else {
            return Err(Error::di(format!(
                "JWK '{}' is not a usable RSA/P-256 signing key",
                jwk.kid.as_deref().unwrap_or("")
            )));
        };
        self.keys.push(ResolvedJwk {
            kid: jwk.kid.clone().unwrap_or_default(),
            alg,
            key: Arc::new(key),
        });
        Ok(())
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

    #[test]
    fn duplicate_key_ids_and_empty_kids_are_rejected() {
        let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("EC key");
        let jwk =
            Jwk::from_public_key_pem(JwtAlgorithm::Es256, "dup", pair.public_key_pem().as_bytes())
                .expect("public JWK");

        // Two entries publishing the same (kid, alg): "first match wins" would
        // make verification depend on document order.
        let set = JwkSet {
            keys: vec![jwk.clone(), jwk.clone()],
        };
        let error = parse_jwk_set(&set, 32)
            .err()
            .expect("a duplicate key id must reject the document");
        assert_eq!(error.status_code(), 503, "got {error:?}");

        // An empty kid is ambiguous in a multi-key set: skip it, keep the named
        // key.
        let mut empty = jwk.clone();
        empty.kid = Some(String::new());
        let set = JwkSet {
            keys: vec![empty, jwk],
        };
        let keys = parse_jwk_set(&set, 32).expect("one named key remains");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kid, "dup");
    }
}
