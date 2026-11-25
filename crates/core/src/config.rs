//! Built-in configuration support: appsettings.json + AppOptions pattern.
//!
//! The framework automatically loads `appsettings.json` (merged with
//! `appsettings.Development.json` in dev mode) and binds it to the
//! built-in `AppOptions` struct.  Users customize options via
//! `HostBuilder::configure(|app| app.options(|o| { ... }))`.

use crate::mode::AppMode;
use serde::Deserialize;
use std::path::Path;

// ---------------------------------------------------------------------------
// IAppOptions trait (for user-defined option types)
// ---------------------------------------------------------------------------

/// Application options — binds to a section of appsettings.json.
///
/// Users define their own structs implementing this trait,
/// then call `AppConfig::bind()` to bind values.
pub trait IAppOptions: for<'de> Deserialize<'de> + Default + Send + Sync + 'static {}

impl<T> IAppOptions for T where T: for<'de> Deserialize<'de> + Default + Send + Sync + 'static {}

// ---------------------------------------------------------------------------
// Built-in option types (matching standard appsettings.json layout)
// ---------------------------------------------------------------------------

/// Top-level application section.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct AppSection {
    /// Application display name.
    #[serde(default, rename = "Name")]
    pub name: String,
    /// Listen addresses as full URLs (e.g., "http://0.0.0.0:5000").
    /// ASP.NET Core compatible. Default: ["http://0.0.0.0:5000"].
    #[serde(default = "default_urls", rename = "Urls")]
    pub urls: Vec<String>,
    /// Maximum request body size in bytes. Default: 10 MB.
    #[serde(default = "default_max_body_size", rename = "MaxBodySize")]
    pub max_body_size: usize,
    /// Maximum concurrent connections. Default: 10000.
    #[serde(default = "default_max_connections", rename = "MaxConnections")]
    pub max_connections: usize,
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            name: String::new(),
            urls: default_urls(),
            max_body_size: default_max_body_size(),
            max_connections: default_max_connections(),
        }
    }
}

fn default_urls() -> Vec<String> {
    vec!["http://0.0.0.0:5000".to_string()]
}

fn default_max_body_size() -> usize {
    10 * 1024 * 1024 // 10 MB
}

fn default_max_connections() -> usize {
    10_000
}

/// JWT authentication section.
///
/// appsettings keys are PascalCase: `Jwt.Secret` and `Jwt.ExpirationMinutes`
/// (or `APP__Jwt__ExpirationMinutes`). `ExpirationMinutes` drives the lifetime
/// of tokens minted by the default `ITokenIssuer` that
/// `HostBuilder::use_authentication()` registers.
///
/// # Where the signing secret comes from (XR-2)
///
/// First non-empty source wins, evaluated at host startup:
///
/// 1. `APP__Jwt__Secret` / `JWT_SECRET` env vars — applied over the JSON during
///    `load_appsettings`, so they arrive here as `Secret`;
/// 2. `Jwt.Secret` in `appsettings.json` (or a code-level
///    `configure(|app| app.options(..))` override);
/// 3. `Jwt.SecretFile` — a path to a file holding the secret, read by
///    `rust_webx_host::auth_jwt::apply_jwt_secret_file`. The file must be at
///    least 32 bytes of non-whitespace content and, on Unix, must not grant
///    group/other permissions (`chmod 600`); a missing, unreadable, too short
///    or over-permissive file fails host startup instead of running with a
///    broken key.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct JwtSection {
    /// HMAC secret for signing/verifying JWT tokens.
    #[serde(default, rename = "Secret")]
    pub secret: String,

    /// Optional path to a file holding the HMAC secret.
    ///
    /// Only used when `Secret` is empty, so an explicit secret (env or JSON)
    /// always wins and existing appsettings keep working unchanged. The
    /// framework does not create the file: provisioning the secret is an
    /// operator/installer concern. DPAPI-wrapped secrets are not supported by
    /// this release.
    #[serde(default, rename = "SecretFile")]
    pub secret_file: Option<String>,

    /// Lifetime of freshly issued tokens, in minutes. Default: 60.
    ///
    /// `0` falls back to the default: a zero lifetime would only mint tokens
    /// that are already expired.
    #[serde(
        default = "default_jwt_expiration_minutes",
        rename = "ExpirationMinutes"
    )]
    pub expiration_minutes: u64,

    /// Expected `iss` claim. When set (non-empty), a token without a matching
    /// `iss` is rejected.
    ///
    /// Additive: unset means no issuer check.
    #[serde(default, rename = "Issuer")]
    pub issuer: Option<String>,

    /// Expected `aud` claim. When set (non-empty), a token without a matching
    /// `aud` is rejected.
    ///
    /// Additive: unset means no audience check.
    #[serde(default, rename = "Audience")]
    pub audience: Option<String>,

    /// Accepted signature algorithms. Default: `["HS256"]`.
    ///
    /// `HS256` uses the shared secret; the asymmetric algorithms require JWKS
    /// metadata so the host can resolve the signing keys. A configured value
    /// that the host cannot verify is a startup error (fail-closed), never a
    /// silent downgrade.
    #[serde(default = "default_jwt_algorithms", rename = "Algorithms")]
    pub algorithms: Vec<JwtAlgorithm>,

    /// Clock-skew allowance for `exp`/`nbf`/`iat`, in seconds. Default: 30.
    ///
    /// The host rejects values above [`MAX_JWT_CLOCK_SKEW_SECONDS`] at startup.
    #[serde(
        default = "default_jwt_clock_skew_seconds",
        rename = "ClockSkewSeconds"
    )]
    pub clock_skew_seconds: u64,

    /// What an explicitly presented but invalid token means.
    ///
    /// `Anonymous` (default) keeps the framework's long-standing semantics: a
    /// bad token only means "not authenticated", so public endpoints (login,
    /// health) stay reachable. `Reject` answers `401` immediately, which is the
    /// OAuth 2.0 resource-server behaviour.
    #[serde(default, rename = "InvalidTokenBehavior")]
    pub invalid_token_behavior: InvalidTokenBehavior,

    /// Maximum accepted token size, in bytes. Default: 8192.
    ///
    /// Tokens above the limit are rejected before signature verification, so a
    /// hostile header cannot force a large allocation.
    #[serde(default = "default_jwt_max_token_bytes", rename = "MaxTokenBytes")]
    pub max_token_bytes: usize,

    /// Remote JWKS source for asymmetric verification (RFC 7517).
    ///
    /// Required when `Algorithms` contains RSA/EC/PS algorithms. The host
    /// fetches and caches the set, refreshes it for an unknown `kid`, and
    /// rejects non-HTTPS or SSRF-guarded destinations at startup.
    #[serde(default, rename = "Jwks")]
    pub jwks: Option<JwtJwksSection>,
}

/// Remote JSON Web Key Set (JWKS) configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct JwtJwksSection {
    /// Absolute `https://` URL of the JWKS document.
    #[serde(default, rename = "Url")]
    pub url: String,
    /// Cache lifetime of a fetched set, in seconds. Default: 300.
    #[serde(default = "default_jwks_cache_seconds", rename = "CacheSeconds")]
    pub cache_seconds: u64,
    /// Per-fetch timeout, in milliseconds. Default: 2000 (bounded 100..=10000).
    #[serde(default = "default_jwks_timeout_ms", rename = "TimeoutMs")]
    pub timeout_ms: u64,
    /// Maximum keys kept from one document (default 32, bounded 1..=256).
    #[serde(default = "default_jwks_max_keys", rename = "MaxKeys")]
    pub max_keys: usize,
    /// Allow a JWKS on a private/loopback network (intranet IdP).
    ///
    /// Off by default: the SSRF guard stays enabled. Turn it on only when the
    /// JWKS host is a trusted internal service.
    #[serde(default, rename = "AllowPrivateNetworks")]
    pub allow_private_networks: bool,
}

impl Default for JwtJwksSection {
    fn default() -> Self {
        Self {
            url: String::new(),
            cache_seconds: default_jwks_cache_seconds(),
            timeout_ms: default_jwks_timeout_ms(),
            max_keys: default_jwks_max_keys(),
            allow_private_networks: false,
        }
    }
}

fn default_jwks_cache_seconds() -> u64 {
    300
}

fn default_jwks_timeout_ms() -> u64 {
    2000
}

fn default_jwks_max_keys() -> usize {
    32
}

/// JWT signature algorithm accepted by the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum JwtAlgorithm {
    /// HMAC-SHA256 (shared secret).
    #[default]
    #[serde(rename = "HS256")]
    Hs256,
    /// RSASSA-PKCS1-v1_5 with SHA-256 (requires JWKS keys).
    #[serde(rename = "RS256")]
    Rs256,
    /// ECDSA with P-256 and SHA-256 (requires JWKS keys).
    #[serde(rename = "ES256")]
    Es256,
    /// RSASSA-PSS with SHA-256 (requires JWKS keys).
    #[serde(rename = "PS256")]
    Ps256,
}

impl JwtAlgorithm {
    /// Stable configuration/CLI spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hs256 => "HS256",
            Self::Rs256 => "RS256",
            Self::Es256 => "ES256",
            Self::Ps256 => "PS256",
        }
    }
}

/// What to do when a request explicitly presents an invalid token.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum InvalidTokenBehavior {
    /// Bad token = anonymous (public endpoints stay reachable). Default.
    #[default]
    #[serde(rename = "Anonymous")]
    Anonymous,
    /// Bad token = immediate `401` (OAuth 2.0 resource-server semantics).
    #[serde(rename = "Reject")]
    Reject,
}

fn default_jwt_algorithms() -> Vec<JwtAlgorithm> {
    vec![JwtAlgorithm::Hs256]
}

/// Upper bound for `Jwt.ClockSkewSeconds`.
///
/// A large leeway silently extends every token's lifetime; the host rejects
/// configurations above this bound instead of accepting a five-minute grace
/// period that is really a ten-minute one.
pub const MAX_JWT_CLOCK_SKEW_SECONDS: u64 = 300;

/// Upper bound for `Jwt.MaxTokenBytes` (1 MiB).
pub const MAX_JWT_TOKEN_BYTES: usize = 1024 * 1024;

fn default_jwt_clock_skew_seconds() -> u64 {
    30
}

fn default_jwt_max_token_bytes() -> usize {
    8 * 1024
}

fn default_jwt_expiration_minutes() -> u64 {
    60
}

impl Default for JwtSection {
    fn default() -> Self {
        Self {
            secret: String::new(),
            secret_file: None,
            expiration_minutes: default_jwt_expiration_minutes(),
            issuer: None,
            audience: None,
            algorithms: default_jwt_algorithms(),
            clock_skew_seconds: default_jwt_clock_skew_seconds(),
            invalid_token_behavior: InvalidTokenBehavior::default(),
            max_token_bytes: default_jwt_max_token_bytes(),
            jwks: None,
        }
    }
}

/// CORS (Cross-Origin Resource Sharing) section.
///
/// JSON keys use PascalCase (`Origins`, `Methods`, …) to match appsettings
/// and ASP.NET Core conventions — same as `App` / `Jwt` / `RateLimit`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[non_exhaustive]
pub struct CorsSection {
    /// Allowed origins. Default: ["*"].
    #[serde(default = "default_origins")]
    pub origins: Vec<String>,
    /// Allowed methods. Default: GET, POST, PUT, DELETE, PATCH, OPTIONS.
    #[serde(default = "default_cors_methods")]
    pub methods: Vec<String>,
    /// Allowed headers. Default: Content-Type, Authorization.
    #[serde(default = "default_cors_headers")]
    pub headers: Vec<String>,
    /// Response headers exposed to the client. Default: empty.
    #[serde(default)]
    pub expose_headers: Vec<String>,
    /// Allow credentials. Default: false.
    #[serde(default)]
    pub allow_credentials: bool,
    /// Preflight cache max-age in seconds. Default: 86400.
    #[serde(default = "default_max_age")]
    pub max_age: u32,
}

impl Default for CorsSection {
    fn default() -> Self {
        Self {
            origins: default_origins(),
            methods: default_cors_methods(),
            headers: default_cors_headers(),
            expose_headers: Vec::new(),
            allow_credentials: false,
            max_age: default_max_age(),
        }
    }
}

fn default_origins() -> Vec<String> {
    // No CORS by default. The old `"*"` default made the framework's own
    // Production guard refuse to start an unconfigured host, so the README
    // "Hello World" could never run. An application that wants cross-origin
    // access opts in explicitly (`Cors.Origins` or `use_cors`).
    Vec::new()
}

fn default_cors_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "POST".to_string(),
        "PUT".to_string(),
        "DELETE".to_string(),
        "PATCH".to_string(),
        "OPTIONS".to_string(),
    ]
}

fn default_cors_headers() -> Vec<String> {
    vec!["Content-Type".to_string(), "Authorization".to_string()]
}

fn default_max_age() -> u32 {
    86400
}

/// Culture / localization section.
///
/// The default UI culture is a host-wide setting: endpoint names, OpenAPI, and
/// permission synchronization use it unless a caller explicitly passes another
/// culture. Request-header negotiation is deliberately not part of this:
/// administrative output must not change with the caller's browser language.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct CultureSection {
    /// Default BCP-47 culture, e.g. `"zh-CN"` or `"en"`. Default: `"en"`.
    #[serde(default = "default_culture", rename = "Default")]
    pub default: String,
}

impl Default for CultureSection {
    fn default() -> Self {
        Self {
            default: default_culture(),
        }
    }
}

fn default_culture() -> String {
    "en".to_string()
}

/// Per-IP rate limiting (token bucket).
///
/// The limiter keys by the request's resolved
/// [`ClientInfo`](crate::client_info::ClientInfo) — the peer address, or the
/// trusted forwarded client when the `ClientInfo` middleware ran. Forwarded
/// header trust is configured only in [`ClientInfoSection`].
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct RateLimitSection {
    /// Enable rate-limit middleware. Default: false (enable in Production appsettings).
    #[serde(default, rename = "Enabled")]
    pub enabled: bool,
    /// Sustained requests per second per client IP.
    #[serde(default = "default_rate_limit_rps", rename = "RequestsPerSecond")]
    pub requests_per_second: f64,
    /// Burst capacity before throttling.
    #[serde(default = "default_rate_limit_burst", rename = "BurstSize")]
    pub burst_size: u32,
    /// Maximum distinct client IPs tracked (LRU eviction when exceeded).
    #[serde(default = "default_rate_limit_max_ips", rename = "MaxTrackedIps")]
    pub max_tracked_ips: usize,
}

impl Default for RateLimitSection {
    fn default() -> Self {
        Self {
            enabled: false,
            requests_per_second: default_rate_limit_rps(),
            burst_size: default_rate_limit_burst(),
            max_tracked_ips: default_rate_limit_max_ips(),
        }
    }
}

fn default_rate_limit_rps() -> f64 {
    100.0
}

fn default_rate_limit_burst() -> u32 {
    200
}

fn default_rate_limit_max_ips() -> usize {
    10_000
}

/// HTTP request metrics (`GET /metrics` Prometheus text when enabled).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct MetricsSection {
    #[serde(default, rename = "Enabled")]
    pub enabled: bool,
    /// Cardinality cap for the default in-memory `IMeter` bridge.
    ///
    /// `/metrics` renders the application meter; the cap bounds its series
    /// (labels included) so a label explosion cannot exhaust memory. Ignored when
    /// the application registers its own meter.
    #[serde(default = "default_metrics_max_series", rename = "MaxSeries")]
    pub max_series: usize,
}

impl Default for MetricsSection {
    fn default() -> Self {
        Self {
            enabled: false,
            max_series: default_metrics_max_series(),
        }
    }
}

fn default_metrics_max_series() -> usize {
    10_000
}

/// Trusted client information (`X-Forwarded-For` resolution).
///
/// This is the framework's **single** forwarded-header configuration: the host
/// installs the `ClientInfo` middleware from it, and every consumer (rate
/// limiting included) reads the resolved value instead of parsing the headers
/// again.
///
/// Forwarded headers are only honoured when both `TrustForwardedHeaders` is on
/// **and** the direct peer belongs to `TrustedProxies`; a directly reachable
/// client could otherwise rotate `X-Forwarded-For` to spoof its address.
#[derive(Debug, Clone, Deserialize, Default)]
#[non_exhaustive]
pub struct ClientInfoSection {
    /// Resolve the client address from forwarded headers behind trusted proxies.
    #[serde(default, rename = "TrustForwardedHeaders")]
    pub trust_forwarded_headers: bool,
    /// CIDR ranges of the trusted reverse proxies (for example `["10.0.0.0/8"]`).
    #[serde(default, rename = "TrustedProxies")]
    pub trusted_proxies: Vec<String>,
}

/// Request-level idempotency (`Idempotency-Key`).
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct IdempotencySection {
    /// Install the idempotency middleware. Default: false.
    #[serde(default, rename = "Enabled")]
    pub enabled: bool,
    /// How long a stored response (or in-flight reservation) is retained.
    #[serde(default = "default_idempotency_ttl_seconds", rename = "TtlSeconds")]
    pub ttl_seconds: u64,
    /// How long an in-flight reservation blocks concurrent retries before a
    /// retry may reclaim the key (`Idempotency.LeaseSeconds`, default 60s).
    ///
    /// This is the reservation lease, not the replay retention: a crashed
    /// handler becomes retryable after the lease, while a completed response
    /// replays for [`Self::ttl_seconds`]. The middleware caps it at
    /// `TtlSeconds`, so a lease longer than the retention never reaches the
    /// store. Keep it above the handler's worst-case duration.
    #[serde(default = "default_idempotency_lease_seconds", rename = "LeaseSeconds")]
    pub lease_seconds: u64,
    /// Maximum retained keys for the default in-memory store.
    #[serde(default = "default_idempotency_max_keys", rename = "MaxTrackedKeys")]
    pub max_tracked_keys: usize,
    /// Responses larger than this are not cached (the key is released instead).
    #[serde(
        default = "default_idempotency_max_body_bytes",
        rename = "MaxBodyBytes"
    )]
    pub max_body_bytes: usize,
    /// Permit idempotency for anonymous requests. Default: true (a framework
    /// default that keeps existing deployments working).
    ///
    /// Anonymous requests are scoped by the resolved client IP. Requests whose
    /// client key is deterministically derived from the body (a common pattern)
    /// can then replay another user's response from the same NAT/proxy address,
    /// so set this to `false` unless every anonymous state-changing endpoint is
    /// safe to replay across clients. Authenticated requests are unaffected.
    #[serde(
        default = "default_idempotency_allow_anonymous",
        rename = "AllowAnonymous"
    )]
    pub allow_anonymous: bool,
    /// Only requests whose path starts with one of these prefixes are
    /// idempotent. Empty = every path (the pre-2.8 behavior).
    ///
    /// Entries must start with `/` and match on path-segment boundaries:
    /// `/api/orders` covers `/api/orders` and `/api/orders/123` but not
    /// `/api/orders-archive`; a trailing slash is ignored. Requests outside the
    /// list pass through without reading the key header or touching the store.
    #[serde(default, rename = "Paths")]
    pub paths: Vec<String>,
    /// Requests whose path starts with one of these prefixes are never
    /// idempotent. Takes precedence over `Paths`; same matching rules.
    /// Default: empty.
    #[serde(default, rename = "SkipPaths")]
    pub skip_paths: Vec<String>,
}

impl Default for IdempotencySection {
    fn default() -> Self {
        Self {
            enabled: false,
            ttl_seconds: default_idempotency_ttl_seconds(),
            lease_seconds: default_idempotency_lease_seconds(),
            max_tracked_keys: default_idempotency_max_keys(),
            max_body_bytes: default_idempotency_max_body_bytes(),
            allow_anonymous: default_idempotency_allow_anonymous(),
            paths: Vec::new(),
            skip_paths: Vec::new(),
        }
    }
}

fn default_idempotency_ttl_seconds() -> u64 {
    24 * 60 * 60
}

fn default_idempotency_lease_seconds() -> u64 {
    60
}

fn default_idempotency_max_keys() -> usize {
    10_000
}

fn default_idempotency_max_body_bytes() -> usize {
    1024 * 1024
}

fn default_idempotency_allow_anonymous() -> bool {
    true
}

/// Client-certificate policy for the TLS listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum TlsClientAuth {
    /// No client certificate is requested (TLS server only).
    #[default]
    Off,
    /// Request a certificate but allow clients without one.
    Optional,
    /// Require a certificate signed by `Tls.ClientCaPath`.
    Required,
}

/// TLS (Transport Layer Security) section.
///
/// TLS is activated automatically when the `App.Urls` array
/// contains one or more `https://` entries. The certificate
/// and key paths are read from this section.
#[derive(Debug, Clone, Deserialize, Default)]
#[non_exhaustive]
pub struct TlsSection {
    /// Path to TLS certificate PEM file.
    #[serde(default, rename = "CertPath")]
    pub cert_path: String,
    /// Path to TLS private key PEM file.
    #[serde(default, rename = "KeyPath")]
    pub key_path: String,
    /// CA bundle used to verify client certificates (PEM).
    #[serde(default, rename = "ClientCaPath")]
    pub client_ca_path: String,
    /// Client-certificate policy (`Off`/`Optional`/`Required`).
    #[serde(default, rename = "ClientAuth")]
    pub client_auth: TlsClientAuth,
}

/// `multipart/form-data` upload settings.
///
/// These are deliberately separate from `App.MaxBodySize`: a JSON endpoint
/// should stay tightly bounded while an upload endpoint legitimately accepts
/// far more. A request is measured against `MaxRequestSize` when it declares
/// `Content-Type: multipart/form-data`, and against `App.MaxBodySize`
/// otherwise.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct FormSection {
    /// Largest accepted multipart request body, in bytes. Default: 256 MiB.
    #[serde(default = "default_max_request_size", rename = "MaxRequestSize")]
    pub max_request_size: usize,
    /// Largest accepted individual file part, in bytes. Default: 128 MiB.
    #[serde(default = "default_max_file_size", rename = "MaxFileSize")]
    pub max_file_size: u64,
    /// Largest accepted individual text field, in bytes. Default: 1 MiB.
    #[serde(default = "default_max_field_size", rename = "MaxFieldSize")]
    pub max_field_size: usize,
    /// Bytes buffered in memory per file before spooling to disk. Default: 1 MiB.
    ///
    /// Set to `0` to spool every upload, or very high to keep everything in
    /// memory.
    #[serde(default = "default_memory_threshold", rename = "MemoryThreshold")]
    pub memory_threshold: usize,
    /// Directory for spooled uploads. Default: `webx-uploads` under the system
    /// temporary directory.
    #[serde(default, rename = "TempDir")]
    pub temp_dir: Option<String>,
}

impl Default for FormSection {
    fn default() -> Self {
        Self {
            max_request_size: default_max_request_size(),
            max_file_size: default_max_file_size(),
            max_field_size: default_max_field_size(),
            memory_threshold: default_memory_threshold(),
            temp_dir: None,
        }
    }
}

fn default_max_request_size() -> usize {
    256 * 1024 * 1024
}

fn default_max_file_size() -> u64 {
    128 * 1024 * 1024
}

fn default_max_field_size() -> usize {
    1024 * 1024
}

fn default_memory_threshold() -> usize {
    1024 * 1024
}

fn default_first_request_timeout_seconds() -> u64 {
    10
}

/// Default admission queue deadline: a saturated server answers `503` instead
/// of hiding the queue in the kernel backlog.
fn default_admission_timeout_ms() -> u64 {
    1_000
}

/// Default inbound deadline: a bounded request is the fail-closed default.
fn default_request_timeout_seconds() -> u64 {
    30
}

fn default_drain_timeout_seconds() -> u64 {
    30
}

fn default_stream_drain_timeout_seconds() -> u64 {
    300
}

/// Server lifecycle settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[non_exhaustive]
pub struct ServerSection {
    /// How long normal (non-streaming) connections may drain during shutdown.
    #[serde(default = "default_drain_timeout_seconds")]
    pub drain_timeout_seconds: u64,
    /// How long connections with an SSE response may drain during shutdown.
    #[serde(default = "default_stream_drain_timeout_seconds")]
    pub stream_drain_timeout_seconds: u64,
    /// Deadline for one inbound request, in seconds. Default: 30.
    ///
    /// The budget starts after routing and covers binding plus handler
    /// execution. A handler that exceeds it is cancelled and the client gets a
    /// `504`. It deliberately does **not** bound the response body of a streamed
    /// response: an `SseResult` handler returns quickly and the stream then runs
    /// under its own `max_lifetime` / drain budget.
    ///
    /// `0` disables the deadline. Production rejects it (fail-closed); raise the
    /// budget for endpoints that legitimately upload large bodies over slow
    /// links instead of disabling it.
    #[serde(default = "default_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
    /// Number of engine worker threads. `0` means auto (available parallelism,
    /// clamped to 1..=64). Explicit values are clamped to the same range.
    #[serde(default)]
    pub workers: usize,
    /// Deadline, in seconds, for an accepted connection to begin its first
    /// request. A connection that sends no bytes (or a partial HTTP/2 preface)
    /// is closed, releasing its connection slot. Default: 10.
    ///
    /// Zero is treated as the default rather than "disabled": an unbounded
    /// pre-request connection is a denial-of-service vector.
    #[serde(default = "default_first_request_timeout_seconds")]
    pub first_request_timeout_seconds: u64,

    /// How long a newly accepted connection may wait for an admission slot
    /// before the engine answers `503` and closes it. Default: 1000 ms.
    ///
    /// `0` waits forever (the pre-2.2 behaviour) and is rejected in
    /// Production: a saturated server must shed load instead of letting the
    /// kernel backlog hide the queue.
    #[serde(default = "default_admission_timeout_ms")]
    pub admission_timeout_ms: u64,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            drain_timeout_seconds: default_drain_timeout_seconds(),
            stream_drain_timeout_seconds: default_stream_drain_timeout_seconds(),
            request_timeout_seconds: default_request_timeout_seconds(),
            workers: 0,
            first_request_timeout_seconds: default_first_request_timeout_seconds(),
            admission_timeout_ms: default_admission_timeout_ms(),
        }
    }
}

/// TURN / STUN configuration (coturn REST API credentials).
///
/// JSON keys are PascalCase (`Turn.Secret`, `Turn.Urls`, `Turn.TtlSeconds`,
/// `Turn.Realm`); environment overrides use `APP__Turn__Secret`,
/// `APP__Turn__Urls__0`, ... (the generic config binder).
///
/// When `Secret` **and** `Urls` are empty the section counts as unconfigured:
/// the host then does not register a [`TurnRestConfig`] (TURN endpoints fail
/// with a clear DI error instead of serving unusable credentials).
///
/// [`TurnRestConfig`]: crate::realtime::TurnRestConfig
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[non_exhaustive]
pub struct TurnSection {
    /// coturn `static-auth-secret` shared secret.
    #[serde(default)]
    pub secret: String,
    /// TURN/STUN URLs handed to clients (`turn:host:3478`, `stun:host:3478`).
    #[serde(default)]
    pub urls: Vec<String>,
    /// Ephemeral credential lifetime in seconds (default 1h, max 24h).
    #[serde(default = "default_turn_ttl_seconds")]
    pub ttl_seconds: u64,
    /// Optional TURN realm echoed to clients.
    #[serde(default)]
    pub realm: Option<String>,
}

impl Default for TurnSection {
    fn default() -> Self {
        Self {
            secret: String::new(),
            urls: Vec::new(),
            ttl_seconds: default_turn_ttl_seconds(),
            realm: None,
        }
    }
}

impl TurnSection {
    /// Whether the application configured TURN at all.
    ///
    /// Any non-empty field counts as an intent to configure, so a partially
    /// filled section fails validation at startup instead of being ignored.
    pub fn is_configured(&self) -> bool {
        !self.secret.trim().is_empty()
            || self.urls.iter().any(|url| !url.trim().is_empty())
            || self.realm.is_some()
    }
}

fn default_turn_ttl_seconds() -> u64 {
    // Single source of truth for the default lifetime.
    crate::realtime::DEFAULT_TURN_TTL_SECONDS
}

/// Standard application options loaded from appsettings.json.
///
/// Bound automatically by the framework.  Access via `host.options()`
/// or customize via `configure(|app| app.options(..))`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppOptions {
    /// Application settings.
    #[serde(default, rename = "App")]
    pub app: AppSection,
    /// JWT authentication settings.
    #[serde(default, rename = "Jwt")]
    pub jwt: JwtSection,
    /// CORS settings.
    #[serde(default, rename = "Cors")]
    pub cors: CorsSection,
    /// TLS settings.
    #[serde(default, rename = "Tls")]
    pub tls: TlsSection,
    /// Server lifecycle settings.
    #[serde(default, rename = "Server")]
    pub server: ServerSection,
    /// Rate limiting settings.
    #[serde(default, rename = "RateLimit")]
    pub rate_limit: RateLimitSection,
    /// Prometheus-style metrics endpoint settings.
    #[serde(default, rename = "Metrics")]
    pub metrics: MetricsSection,
    /// Request-level idempotency settings.
    #[serde(default, rename = "Idempotency")]
    pub idempotency: IdempotencySection,
    /// Trusted client information (`X-Forwarded-For`) settings.
    #[serde(default, rename = "ClientInfo")]
    pub client_info: ClientInfoSection,
    /// TURN/STUN credential settings (coturn REST API).
    #[serde(default, rename = "Turn")]
    pub turn: TurnSection,
    /// `multipart/form-data` upload settings.
    #[serde(default, rename = "Form")]
    pub form: FormSection,
    /// Default culture for endpoint text and OpenAPI.
    #[serde(default, rename = "Culture")]
    pub culture: CultureSection,
}

// ---------------------------------------------------------------------------
// Config loading helpers
// ---------------------------------------------------------------------------

/// Configuration loading or binding failure.
///
/// Returned by the strict loaders and binders. The historical
/// [`load_appsettings`], [`bind_config`] and [`bind_root`] keep their lossy
/// behaviour for compatibility; the host itself uses the strict variants.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A config file exists but cannot be read.
    #[error("config file `{path}` cannot be read: {source}")]
    Read {
        /// Path that was attempted.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A config file exists but is not valid JSON.
    #[error("config file `{path}` is not valid JSON: {source}")]
    Parse {
        /// Path that was attempted.
        path: String,
        /// Underlying parse error.
        #[source]
        source: serde_json::Error,
    },
    /// A section does not match its schema (wrong type, invalid enum, ...).
    #[error("config section `{section}` does not match its schema: {source}")]
    Bind {
        /// Section name (`<root>` for the whole document).
        section: String,
        /// Underlying deserialization error.
        #[source]
        source: serde_json::Error,
    },
    /// A framework section contains keys the schema does not know.
    ///
    /// This is a startup error on purpose: the historical behaviour ignored the
    /// key and kept the default, which silently disabled settings such as rate
    /// limiting after a single typo.
    #[error(
        "unknown key(s) in config section `{section}`: {keys} \
         (fix the spelling; the framework no longer silently ignores them)"
    )]
    UnknownKeys {
        /// Section(s) that contained unknown keys.
        section: String,
        /// Ignored dotted paths, for example `RateLimit.RequestsPerSecondd`.
        keys: String,
    },
}

/// Framework-owned config sections.
///
/// Unknown keys inside these sections are startup errors; unknown keys in
/// application sections are left to the application.
const FRAMEWORK_SECTIONS: &[&str] = &[
    "App",
    "Jwt",
    "Cors",
    "Tls",
    "Server",
    "RateLimit",
    "Metrics",
    "Form",
    "Culture",
];

/// Where the effective configuration came from.
///
/// Values are never recorded — only file names and environment variable
/// **names** — so the report can be logged safely.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConfigSources {
    /// Files read, in precedence order (base first, overlay second).
    pub files: Vec<String>,
    /// `APP__*` environment keys applied.
    pub env_keys: Vec<String>,
    /// Conventional environment keys applied (`JWT_SECRET`, ...).
    pub well_known_env_keys: Vec<String>,
}

impl ConfigSources {
    /// Whether nothing but built-in defaults was used.
    pub fn is_default_only(&self) -> bool {
        self.files.is_empty() && self.env_keys.is_empty() && self.well_known_env_keys.is_empty()
    }

    /// One-line operator report with values redacted.
    pub fn summary(&self) -> String {
        if self.is_default_only() {
            return "built-in defaults only".to_string();
        }
        let mut parts = Vec::new();
        if !self.files.is_empty() {
            parts.push(format!("files=[{}]", self.files.join(", ")));
        }
        if !self.well_known_env_keys.is_empty() {
            parts.push(format!("env=[{}]", self.well_known_env_keys.join(", ")));
        }
        if !self.env_keys.is_empty() {
            parts.push(format!("APP__=[{}]", self.env_keys.join(", ")));
        }
        parts.join("; ")
    }
}

/// Strict counterpart of [`load_appsettings`].
///
/// Missing files are not an error (env-only deployments are supported); a file
/// that exists but cannot be read or parsed **is** an error.
pub fn try_load_appsettings(
    mode: AppMode,
) -> Result<(Option<serde_json::Value>, ConfigSources), ConfigError> {
    let mut sources = ConfigSources::default();

    let mut merged = match read_json_file_strict("appsettings.json")? {
        Some((value, path)) => {
            sources.files.push(path);
            value
        }
        None => serde_json::json!({}),
    };

    if let Some((overlay, path)) = read_json_file_strict(mode.overlay_filename())? {
        sources.files.push(path);
        merge_json(&mut merged, overlay);
    }

    // Well-known env vars (JWT_SECRET) — applied before APP__ so APP__Jwt__Secret wins.
    apply_well_known_env_overrides(&mut merged, &mut sources);

    // Apply environment variable overrides (APP__Section__Key pattern). These
    // apply even when no appsettings.json exists (env-only deployment).
    apply_env_overrides(&mut merged, &mut sources);

    sources.env_keys.sort();
    sources.well_known_env_keys.sort();

    let has_source = !sources.is_default_only();
    Ok((if has_source { Some(merged) } else { None }, sources))
}

/// Load the merged appsettings JSON (base + environment overlay + env overrides).
///
/// 按运行模式自动合并对应的 `appsettings.{Mode}.json` overlay：
/// - `Development` → `appsettings.Development.json`
/// - `Production`  → `appsettings.Production.json`
///
/// Environment variables prefixed with `APP__` override the corresponding JSON values.
/// For example, `APP__App__Address=0.0.0.0:8080` overrides `{"App": {"Address": "..."}}`.
///
/// Lossy compatibility wrapper around [`try_load_appsettings`]: a broken file
/// logs a warning and falls back to defaults. The host uses the strict variant.
pub fn load_appsettings(mode: AppMode) -> Option<serde_json::Value> {
    match try_load_appsettings(mode) {
        Ok((value, _)) => value,
        Err(error) => {
            tracing::warn!("[Config] {error}; falling back to defaults");
            None
        }
    }
}

/// Apply conventional environment variables that map to config sections.
///
/// `JWT_SECRET` → `Jwt.Secret` (overridden by `APP__Jwt__Secret` if set).
fn apply_well_known_env_overrides(config: &mut serde_json::Value, sources: &mut ConfigSources) {
    if let Ok(secret) = std::env::var("JWT_SECRET") {
        if !secret.is_empty() {
            set_json_value(config, &["Jwt", "Secret"], &secret);
            sources.well_known_env_keys.push("JWT_SECRET".to_string());
        }
    }
}

/// Apply environment variable overrides following the `APP__Section__Key` pattern.
fn apply_env_overrides(config: &mut serde_json::Value, sources: &mut ConfigSources) {
    for (key, value) in std::env::vars() {
        if let Some(path) = key.strip_prefix("APP__") {
            // Split by double underscore to get path segments
            let segments: Vec<&str> = path.split("__").collect();
            if segments.is_empty() {
                continue;
            }
            set_json_value(config, &segments, &value);
            sources.env_keys.push(key);
        }
    }
}

/// Set a value in a JSON object following the given path segments.
fn set_json_value(obj: &mut serde_json::Value, segments: &[&str], value: &str) {
    if segments.is_empty() {
        return;
    }

    let key = segments[0];

    if let serde_json::Value::Object(map) = obj {
        if segments.len() == 1 {
            // Leaf: set the value, attempting to parse as JSON first
            let parsed =
                serde_json::from_str(value).unwrap_or(serde_json::Value::String(value.to_string()));
            map.insert(key.to_string(), parsed);
        } else if let Some(child) = map.get_mut(key) {
            // Recurse into child
            set_json_value(child, &segments[1..], value);
        } else {
            // Create intermediate objects as needed
            let mut child = serde_json::json!({});
            set_json_value(&mut child, &segments[1..], value);
            map.insert(key.to_string(), child);
        }
    }
}

/// Bind a section of the config JSON to a deserializable type.
pub fn bind_config<T: for<'de> Deserialize<'de> + Default>(
    config: &serde_json::Value,
    section: &str,
) -> T {
    if section.is_empty() || section == "." {
        serde_json::from_value(config.clone()).unwrap_or_else(|e| {
            tracing::warn!(
                "[Config] Failed to bind root config to {}: {}, using defaults",
                std::any::type_name::<T>(),
                e
            );
            T::default()
        })
    } else {
        config
            .get(section)
            .map(|v| {
                serde_json::from_value(v.clone()).unwrap_or_else(|e| {
                    tracing::warn!(
                        "[Config] Failed to bind section '{}' to {}: {}, using defaults",
                        section,
                        std::any::type_name::<T>(),
                        e
                    );
                    T::default()
                })
            })
            .unwrap_or_default()
    }
}

/// Bind the entire config JSON to a type (for root-level deserialization).
pub fn bind_root<T: for<'de> Deserialize<'de> + Default>(config: &serde_json::Value) -> T {
    serde_json::from_value(config.clone()).unwrap_or_else(|e| {
        tracing::warn!(
            "[Config] Failed to bind root config to {}: {}, using defaults",
            std::any::type_name::<T>(),
            e
        );
        T::default()
    })
}

/// Strict counterpart of [`bind_root`]: a schema mismatch is an error, and
/// unknown keys inside framework-owned sections are rejected.
///
/// Application sections (`"MyFeature"`) are not the framework's business, so
/// unknown top-level sections are ignored as before.
pub fn try_bind_root<T>(config: &serde_json::Value) -> Result<T, ConfigError>
where
    T: for<'de> Deserialize<'de> + Default,
{
    let mut ignored = Vec::new();
    let bound = serde_ignored::deserialize(config.clone(), |path| ignored.push(path.to_string()))
        .map_err(|source| ConfigError::Bind {
        section: "<root>".to_string(),
        source,
    })?;
    reject_unknown_framework_keys(&ignored)?;
    Ok(bound)
}

/// Strict counterpart of [`bind_config`]: a wrong type is an error, and every
/// ignored key is reported (the caller opted into this section, so an unknown
/// key is almost always a typo).
pub fn try_bind_config<T>(config: &serde_json::Value, section: &str) -> Result<T, ConfigError>
where
    T: for<'de> Deserialize<'de> + Default,
{
    let value = if section.is_empty() || section == "." {
        config.clone()
    } else {
        match config.get(section) {
            Some(value) => value.clone(),
            None => return Ok(T::default()),
        }
    };
    let mut ignored = Vec::new();
    let bound = serde_ignored::deserialize(value, |path| ignored.push(path.to_string())).map_err(
        |source| ConfigError::Bind {
            section: section.to_string(),
            source,
        },
    )?;
    if !ignored.is_empty() {
        return Err(ConfigError::UnknownKeys {
            section: section.to_string(),
            keys: ignored.join(", "),
        });
    }
    Ok(bound)
}

fn reject_unknown_framework_keys(ignored: &[String]) -> Result<(), ConfigError> {
    let mut sections: Vec<&str> = Vec::new();
    let mut paths: Vec<&str> = Vec::new();
    for path in ignored {
        let head = path.split('.').next().unwrap_or(path.as_str());
        if FRAMEWORK_SECTIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(head))
        {
            paths.push(path.as_str());
            if !sections.contains(&head) {
                sections.push(head);
            }
        }
    }
    if paths.is_empty() {
        return Ok(());
    }
    Err(ConfigError::UnknownKeys {
        section: sections.join(", "),
        keys: paths.join(", "),
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read `path` from the app base directory or the working directory, strictly.
///
/// Returns `Ok(None)` when no candidate exists (env-only deployments), an error
/// when a candidate exists but cannot be read or parsed.
fn read_json_file_strict(
    path: impl AsRef<Path>,
) -> Result<Option<(serde_json::Value, String)>, ConfigError> {
    let path = path.as_ref();
    let candidates = [crate::paths::app_base().join(path), path.to_path_buf()];
    for candidate in candidates {
        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => {
                let display = candidate.display().to_string();
                let content =
                    std::fs::read_to_string(&candidate).map_err(|source| ConfigError::Read {
                        path: display.clone(),
                        source,
                    })?;
                let value =
                    serde_json::from_str(&content).map_err(|source| ConfigError::Parse {
                        path: display.clone(),
                        source,
                    })?;
                return Ok(Some((value, display)));
            }
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(ConfigError::Read {
                    path: candidate.display().to_string(),
                    source,
                })
            }
        }
    }
    Ok(None)
}

fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge_json(a.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (a, b) => *a = b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_drain_defaults_and_overrides() {
        let default = ServerSection::default();
        assert_eq!(default.drain_timeout_seconds, 30);
        assert_eq!(default.stream_drain_timeout_seconds, 300);

        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "Server": {
                "DrainTimeoutSeconds": 7,
                "StreamDrainTimeoutSeconds": 99
            }
        }))
        .expect("options parse");
        assert_eq!(options.server.drain_timeout_seconds, 7);
        assert_eq!(options.server.stream_drain_timeout_seconds, 99);
    }

    #[test]
    fn server_request_timeout_defaults_to_30s_and_parses_seconds() {
        assert_eq!(ServerSection::default().request_timeout_seconds, 30);

        // The serde default (no `Server` section at all) is the same 30s.
        let empty: AppOptions = bind_root(&serde_json::json!({}));
        assert_eq!(empty.server.request_timeout_seconds, 30);

        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "Server": { "RequestTimeoutSeconds": 5 }
        }))
        .expect("options parse");
        assert_eq!(options.server.request_timeout_seconds, 5);

        // Explicit zero still parses; the host decides whether it is legal.
        let disabled: AppOptions = serde_json::from_value(serde_json::json!({
            "Server": { "RequestTimeoutSeconds": 0 }
        }))
        .expect("options parse");
        assert_eq!(disabled.server.request_timeout_seconds, 0);
    }

    #[test]
    fn turn_section_defaults_are_unconfigured_and_bind_from_json() {
        let empty: AppOptions = bind_root(&serde_json::json!({}));
        assert!(!empty.turn.is_configured());
        assert_eq!(
            empty.turn.ttl_seconds,
            crate::realtime::DEFAULT_TURN_TTL_SECONDS
        );

        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "Turn": {
                "Secret": "turn-secret",
                "Urls": ["turn:turn.example:3478", "stun:turn.example:3478"],
                "TtlSeconds": 120,
                "Realm": "example"
            }
        }))
        .expect("turn config parse");
        assert!(options.turn.is_configured());
        assert_eq!(options.turn.secret, "turn-secret");
        assert_eq!(options.turn.urls.len(), 2);
        assert_eq!(options.turn.ttl_seconds, 120);
        assert_eq!(options.turn.realm.as_deref(), Some("example"));

        // A realm alone is "configured" so validation fails fast instead of
        // silently ignoring a half-filled section.
        let partial: AppOptions = serde_json::from_value(serde_json::json!({
            "Turn": { "Realm": "example" }
        }))
        .expect("partial turn config parse");
        assert!(partial.turn.is_configured());
    }

    #[test]
    fn idempotency_section_defaults_and_parses_lease_seconds() {
        let defaults = IdempotencySection::default();
        assert_eq!(defaults.ttl_seconds, 24 * 60 * 60);
        assert_eq!(defaults.lease_seconds, 60, "reservation lease default");
        assert!(defaults.allow_anonymous);
        assert!(defaults.paths.is_empty());
        assert!(defaults.skip_paths.is_empty());

        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "Idempotency": {
                "Enabled": true,
                "TtlSeconds": 3600,
                "LeaseSeconds": 30,
                "AllowAnonymous": false,
                "Paths": ["/api/orders"],
                "SkipPaths": ["/api/orders/internal"]
            }
        }))
        .expect("idempotency config parse");
        assert!(options.idempotency.enabled);
        assert_eq!(options.idempotency.ttl_seconds, 3600);
        assert_eq!(options.idempotency.lease_seconds, 30);
        assert!(!options.idempotency.allow_anonymous);
        assert_eq!(options.idempotency.paths, vec!["/api/orders".to_string()]);
        assert_eq!(
            options.idempotency.skip_paths,
            vec!["/api/orders/internal".to_string()]
        );

        // The lease default is independent of the retention: setting only
        // `TtlSeconds` keeps `LeaseSeconds` at 60.
        let options: AppOptions = serde_json::from_value(serde_json::json!({
            "Idempotency": { "TtlSeconds": 7200 }
        }))
        .expect("idempotency parse");
        assert_eq!(options.idempotency.ttl_seconds, 7_200);
        assert_eq!(options.idempotency.lease_seconds, 60);
    }

    #[test]
    fn config_env_jwt_and_app_override_precedence() {
        std::env::set_var("JWT_SECRET", "env-jwt-secret-that-is-long-enough-32");
        let mut config = serde_json::json!({ "Jwt": { "Secret": "from-file" } });
        let mut sources = ConfigSources::default();
        apply_well_known_env_overrides(&mut config, &mut sources);
        assert_eq!(
            config["Jwt"]["Secret"],
            "env-jwt-secret-that-is-long-enough-32"
        );

        std::env::set_var(
            "APP__Jwt__Secret",
            "from-app-override-secret-32chars-minimum",
        );
        apply_env_overrides(&mut config, &mut sources);
        std::env::remove_var("JWT_SECRET");
        std::env::remove_var("APP__Jwt__Secret");
        assert_eq!(
            config["Jwt"]["Secret"],
            "from-app-override-secret-32chars-minimum"
        );
        assert!(sources
            .well_known_env_keys
            .iter()
            .any(|key| key == "JWT_SECRET"));
        assert!(sources.env_keys.iter().any(|key| key == "APP__Jwt__Secret"));
    }

    #[test]
    fn strict_bind_root_rejects_unknown_framework_keys() {
        let error = try_bind_root::<AppOptions>(&serde_json::json!({
            "RateLimit": { "Enabled": true, "RequestsPerSecondd": 10.0 }
        }))
        .expect_err("a typo in a framework section must fail the startup bind");

        match error {
            ConfigError::UnknownKeys { section, keys } => {
                assert_eq!(section, "RateLimit");
                assert!(
                    keys.contains("RateLimit.RequestsPerSecondd"),
                    "the report must name the offending path, got: {keys}"
                );
            }
            other => panic!("expected UnknownKeys, got: {other}"),
        }
    }

    #[test]
    fn strict_bind_root_leaves_application_sections_alone() {
        let options = try_bind_root::<AppOptions>(&serde_json::json!({
            "App": { "Name": "svc" },
            "MyFeature": { "Anything": 1 }
        }))
        .expect("application sections are not the framework's business");
        assert_eq!(options.app.name, "svc");
    }

    #[test]
    fn strict_bind_config_reports_type_errors() {
        let error = try_bind_config::<ServerSection>(
            &serde_json::json!({ "Server": { "DrainTimeoutSeconds": "soon" } }),
            "Server",
        )
        .expect_err("a wrong type must not fall back to defaults");
        assert!(matches!(error, ConfigError::Bind { .. }));
    }

    #[test]
    fn strict_bind_config_reports_unknown_keys() {
        let error = try_bind_config::<ServerSection>(
            &serde_json::json!({ "Server": { "DrainTimeoutSecond": 5 } }),
            "Server",
        )
        .expect_err("an unknown key in an opted-in section must be reported");
        match error {
            ConfigError::UnknownKeys { section, keys } => {
                assert_eq!(section, "Server");
                assert!(keys.contains("DrainTimeoutSecond"));
            }
            other => panic!("expected UnknownKeys, got: {other}"),
        }
    }

    #[test]
    fn strict_file_reader_reports_malformed_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("broken.json");
        std::fs::write(&path, "{ not json").expect("write");

        let error = read_json_file_strict(&path).expect_err("malformed JSON must be an error");
        match error {
            ConfigError::Parse { path: reported, .. } => {
                assert!(reported.ends_with("broken.json"), "got: {reported}");
            }
            other => panic!("expected Parse, got: {other}"),
        }

        let valid = dir.path().join("ok.json");
        std::fs::write(&valid, r#"{"App":{"Name":"ok"}}"#).expect("write");
        let (value, reported) = read_json_file_strict(&valid)
            .expect("valid file")
            .expect("file exists");
        assert_eq!(value["App"]["Name"], "ok");
        assert!(reported.ends_with("ok.json"));
    }

    #[test]
    fn cors_section_binds_pascal_case_from_appsettings() {
        // Regression: without rename_all=PascalCase, Origins fell back to ["*"]
        // and Production fail-fast panicked even when Production overlay set explicit origins.
        let json = serde_json::json!({
            "Cors": {
                "Origins": ["https://www.lusida.net", "https://lusida.net"],
                "Methods": ["GET", "POST"],
                "Headers": ["Content-Type", "Authorization"],
                "AllowCredentials": true,
                "MaxAge": 3600
            }
        });
        let opts: AppOptions = bind_root(&json);
        assert_eq!(
            opts.cors.origins,
            vec![
                "https://www.lusida.net".to_string(),
                "https://lusida.net".to_string()
            ]
        );
        assert_eq!(
            opts.cors.methods,
            vec!["GET".to_string(), "POST".to_string()]
        );
        assert!(opts.cors.allow_credentials);
        assert_eq!(opts.cors.max_age, 3600);
        assert!(!opts.cors.origins.iter().any(|o| o == "*"));
    }

    #[test]
    fn culture_section_binds_default_and_falls_back_to_en() {
        let json = serde_json::json!({ "Culture": { "Default": "zh-CN" } });
        let opts: AppOptions = bind_root(&json);
        assert_eq!(opts.culture.default, "zh-CN");

        let opts: AppOptions = bind_root(&serde_json::json!({}));
        assert_eq!(opts.culture.default, "en");
    }

    #[test]
    fn jwt_section_binds_expiration_minutes_with_60_minute_default() {
        let opts: AppOptions = bind_root(&serde_json::json!({}));
        assert_eq!(opts.jwt.expiration_minutes, 60);
        assert!(opts.jwt.secret.is_empty());

        let opts: AppOptions = bind_root(&serde_json::json!({
            "Jwt": { "Secret": "s", "ExpirationMinutes": 15 }
        }));
        assert_eq!(opts.jwt.expiration_minutes, 15);
    }

    #[test]
    fn jwt_section_binds_secret_file_and_defaults_to_none() {
        // XR-2: the new key must be optional so existing appsettings bind
        // exactly as before.
        let opts: AppOptions = bind_root(&serde_json::json!({}));
        assert_eq!(opts.jwt.secret_file, None);
        assert_eq!(JwtSection::default().secret_file, None);

        let opts: AppOptions = bind_root(&serde_json::json!({
            "Jwt": { "SecretFile": "data/jwt.key" }
        }));
        assert_eq!(opts.jwt.secret_file.as_deref(), Some("data/jwt.key"));
        assert!(
            opts.jwt.secret.is_empty(),
            "binding records the path; the host reads the file"
        );

        // Both keys may coexist: resolution picks `Secret` first.
        let opts: AppOptions = bind_root(&serde_json::json!({
            "Jwt": { "Secret": "explicit-secret", "SecretFile": "data/jwt.key" }
        }));
        assert_eq!(opts.jwt.secret, "explicit-secret");
        assert_eq!(opts.jwt.secret_file.as_deref(), Some("data/jwt.key"));
    }

    #[test]
    fn jwt_section_binds_hardening_fields_with_safe_defaults() {
        let opts: AppOptions = bind_root(&serde_json::json!({}));
        assert_eq!(opts.jwt.issuer, None);
        assert_eq!(opts.jwt.audience, None);
        assert_eq!(opts.jwt.algorithms, vec![JwtAlgorithm::Hs256]);
        assert_eq!(opts.jwt.clock_skew_seconds, 30);
        assert_eq!(
            opts.jwt.invalid_token_behavior,
            InvalidTokenBehavior::Anonymous
        );
        assert_eq!(opts.jwt.max_token_bytes, 8192);

        let opts: AppOptions = bind_root(&serde_json::json!({
            "Jwt": {
                "Issuer": "https://id.example.com",
                "Audience": "travel-api",
                "Algorithms": ["HS256"],
                "ClockSkewSeconds": 10,
                "InvalidTokenBehavior": "Reject",
                "MaxTokenBytes": 4096
            }
        }));
        assert_eq!(opts.jwt.issuer.as_deref(), Some("https://id.example.com"));
        assert_eq!(opts.jwt.audience.as_deref(), Some("travel-api"));
        assert_eq!(opts.jwt.clock_skew_seconds, 10);
        assert_eq!(
            opts.jwt.invalid_token_behavior,
            InvalidTokenBehavior::Reject
        );
        assert_eq!(opts.jwt.max_token_bytes, 4096);
    }

    #[test]
    fn jwt_section_binds_jwks_with_bounded_defaults() {
        let opts: AppOptions = bind_root(&serde_json::json!({}));
        assert!(opts.jwt.jwks.is_none());

        let opts: AppOptions = bind_root(&serde_json::json!({
            "Jwt": {
                "Algorithms": ["RS256"],
                "Jwks": { "Url": "https://id.example.com/.well-known/jwks.json" }
            }
        }));
        let jwks = opts.jwt.jwks.expect("jwks section");
        assert_eq!(jwks.url, "https://id.example.com/.well-known/jwks.json");
        assert_eq!(jwks.cache_seconds, 300);
        assert_eq!(jwks.timeout_ms, 2000);
        assert_eq!(jwks.max_keys, 32);
    }

    #[test]
    fn production_overlay_merge_keeps_explicit_cors_origins() {
        let mut base = serde_json::json!({
            "App": { "Urls": ["http://localhost:5000"] },
            "Cors": { "Origins": ["*"] },
            "Jwt": { "Secret": "docbit-dev-secret-change-in-production" }
        });
        let overlay = serde_json::json!({
            "App": { "Urls": ["http://0.0.0.0:8100"] },
            "Cors": {
                "Origins": [
                    "https://www.lusida.net",
                    "https://lusida.net"
                ]
            },
            "Jwt": { "Secret": "" }
        });
        merge_json(&mut base, overlay);
        let opts: AppOptions = bind_root(&base);
        assert_eq!(opts.app.urls, vec!["http://0.0.0.0:8100".to_string()]);
        assert_eq!(
            opts.cors.origins,
            vec![
                "https://www.lusida.net".to_string(),
                "https://lusida.net".to_string()
            ]
        );
        assert!(opts.jwt.secret.is_empty());
    }
}
