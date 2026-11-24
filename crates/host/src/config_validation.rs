//! Startup-time aggregate validation of the merged [`AppOptions`].
//!
//! Historically [`HostBuilder::try_build`](crate::server::HostBuilder::try_build)
//! validated the configuration with a sequence of `?` calls, so an operator
//! with several bad keys saw only the first violation and had to redeploy once
//! per fix. [`validate_options`] and [`format_violations`] collect every
//! violation that can be decided from the merged configuration and report them
//! in one deterministic message.
//!
//! The rules are the existing startup guards — TLS client auth, the bounded
//! engine server (request/admission deadlines and the connection ceiling) and
//! the Production legacy-`RateLimit`-trust rejection. Checks that need builder
//! state rather than [`AppOptions`] alone (whether authentication is enabled,
//! an explicit `use_cors` / `use_turn_config` / `JwtValidation` override) are
//! collected by the host with the helpers this module exposes for them; the
//! assembly-time guards remain the enforcement point, so no rule is duplicated
//! in spirit and no new constraint is introduced.

use std::fmt;

use webx_core::config::AppOptions;
use webx_core::error::Error;
use webx_core::mode::AppMode;

/// One invalid configuration value found during startup validation.
///
/// `key` is the full configuration path an operator searches for (for example
/// `Server.RequestTimeoutSeconds`) and `message` is the human-readable reason
/// (including the accepted values or the fix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigViolation {
    /// Configuration section, for example `"Server"`.
    pub section: &'static str,
    /// Full configuration key path, for example `"Server.RequestTimeoutSeconds"`.
    pub key: String,
    /// Operator-facing reason and fix.
    pub message: String,
}

impl ConfigViolation {
    /// Build a violation from its section, key path and message.
    pub fn new(section: &'static str, key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            section,
            key: key.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.key, self.message)
    }
}

/// Collect every startup violation that is decidable from the merged
/// [`AppOptions`] alone.
///
/// The returned list is deterministically ordered by section, key and message.
/// Builder-state and filesystem-dependent validations (an explicit
/// `use_cors`/`use_turn_config` override, JWT only when authentication is
/// enabled, localization resources, `Jwt.SecretFile`) are enforced at their
/// original startup sites.
pub fn validate_options(options: &AppOptions, mode: AppMode) -> Vec<ConfigViolation> {
    let mut violations = Vec::new();
    collect_tls_client_auth(options, &mut violations);
    collect_engine_server(options, mode, &mut violations);
    collect_legacy_rate_limit_trust(options, mode, &mut violations);
    collect_rate_limit(options, &mut violations);
    collect_cors(options, &mut violations);
    sort_violations(&mut violations);
    violations
}

/// The `Cors` section may not combine `*` with credentials (any mode).
fn collect_cors(options: &AppOptions, violations: &mut Vec<ConfigViolation>) {
    if let Some(violation) =
        cors_credentials_wildcard_violation(&options.cors.origins, options.cors.allow_credentials)
    {
        violations.push(violation);
    }
}

/// An enabled rate limiter with a zero rate or burst limiter is a startup
/// error, not a silently-throttled host.
///
/// `BurstSize = 0` makes every request `429` with `Retry-After: 1` forever;
/// `RequestsPerSecond = 0` allows one burst and then nothing. Both look like a
/// healthy configuration in the log line, so refuse to start instead.
fn collect_rate_limit(options: &AppOptions, violations: &mut Vec<ConfigViolation>) {
    let limit = &options.rate_limit;
    if !limit.enabled {
        return;
    }
    if limit.burst_size < 1 {
        violations.push(ConfigViolation::new(
            "RateLimit",
            "RateLimit.BurstSize",
            "RateLimit is enabled but BurstSize is 0: every request would be rejected with 429. \
             Set BurstSize >= 1 (APP__RateLimit__BurstSize) or disable RateLimit.",
        ));
    }
    if !limit.requests_per_second.is_finite() || limit.requests_per_second <= 0.0 {
        violations.push(ConfigViolation::new(
            "RateLimit",
            "RateLimit.RequestsPerSecond",
            "RateLimit is enabled but RequestsPerSecond must be a finite value > 0. \
             Set RequestsPerSecond (APP__RateLimit__RequestsPerSecond) or disable RateLimit.",
        ));
    }
}

/// Format violations as the operator-facing multi-line startup error.
///
/// The list is sorted by section, key and message before rendering, so the same
/// broken configuration always produces the same message (stable in tests and
/// in log diffing).
pub fn format_violations(violations: &[ConfigViolation]) -> String {
    let mut sorted: Vec<&ConfigViolation> = violations.iter().collect();
    sorted.sort_by(|left, right| {
        (left.section, left.key.as_str(), left.message.as_str()).cmp(&(
            right.section,
            right.key.as_str(),
            right.message.as_str(),
        ))
    });

    let mut message = format!(
        "Invalid configuration: {} violation(s) found. Fix every key below before starting the host:",
        sorted.len()
    );
    for violation in sorted {
        message.push_str("\n  - ");
        message.push_str(&violation.to_string());
    }
    message
}

/// Production refuses a wildcard CORS origin (`*`).
///
/// Shared by the aggregate collector and the assembly-time guard so both report
/// the same text; `origins` must be the list the CORS middleware will actually
/// use (an explicit `use_cors` override supersedes `Cors.Origins`).
pub(crate) fn production_cors_violation(
    mode: AppMode,
    origins: &[String],
) -> Option<ConfigViolation> {
    (mode == AppMode::Production && origins.iter().any(|origin| origin == "*")).then(|| {
        ConfigViolation::new(
            "Cors",
            "Cors.Origins",
            "Production forbids CORS origin '*'. Set explicit origins via Cors.Origins or APP__Cors__Origins.",
        )
    })
}

/// A wildcard origin combined with credentials is refused in every mode.
///
/// Per the CORS spec `Access-Control-Allow-Origin: *` cannot be combined with
/// credentials, so echoing the request origin while `AllowCredentials` is on
/// lets any site read authenticated responses. ASP.NET Core and Spring reject
/// this combination at startup; so does rust-webx, regardless of `APP_ENV`.
pub(crate) fn cors_credentials_wildcard_violation(
    origins: &[String],
    allow_credentials: bool,
) -> Option<ConfigViolation> {
    (allow_credentials && origins.iter().any(|origin| origin == "*")).then(|| {
        ConfigViolation::new(
            "Cors",
            "Cors.Origins",
            "'*' cannot be combined with AllowCredentials = true (the framework would echo any \
             origin with credentials). List the allowed origins explicitly.",
        )
    })
}

/// Production must not sign HS256 tokens with a weak/missing secret.
///
/// `uses_hs256` and `weak_secret` are computed by the caller so the rule stays
/// identical in the aggregate collector and the assembly-time guard.
pub(crate) fn production_jwt_secret_violation(
    mode: AppMode,
    auth_enabled: bool,
    uses_hs256: bool,
    weak_secret: bool,
) -> Option<ConfigViolation> {
    (mode == AppMode::Production && auth_enabled && uses_hs256 && weak_secret).then(|| {
        ConfigViolation::new(
            "Jwt",
            "Jwt.Secret",
            "Production requires a strong JWT secret. Set APP__Jwt__Secret or JWT_SECRET env var (min 32 chars, not a placeholder).",
        )
    })
}

/// Production refuses the legacy `RateLimit.TrustProxy` /
/// `RateLimit.TrustedProxies` keys.
///
/// Ignoring them silently would key every client by the reverse-proxy address
/// (one global bucket an attacker can drain), so the operator must move the
/// trust configuration to `ClientInfo`.
pub(crate) fn legacy_rate_limit_trust_keys_violation(mode: AppMode) -> Option<ConfigViolation> {
    (mode == AppMode::Production).then(|| {
        ConfigViolation::new(
            "RateLimit",
            "RateLimit.TrustProxy",
            LEGACY_RATE_LIMIT_TRUST_MESSAGE,
        )
    })
}

/// Operator-facing text of the legacy `RateLimit` trust-key failure.
const LEGACY_RATE_LIMIT_TRUST_MESSAGE: &str =
    "RateLimit.TrustProxy / RateLimit.TrustedProxies are legacy and ignored since 2.8.0. Move the trust \
     configuration to ClientInfo.TrustForwardedHeaders / ClientInfo.TrustedProxies (see \
     docs/rust-webx/10-configuration/appsettings.md). Development/Test hosts keep starting with a warning.";

/// Turn the message of an existing validator into a structured violation.
///
/// The retained validators return only an [`Error`]; their messages begin with
/// the offending `Section.Key` (for example `Jwt.Algorithms ...` or
/// `Turn.Secret ...`). Lift that token into [`ConfigViolation::key`] and fall
/// back to `fallback_key` when the message does not start with a key path.
pub(crate) fn violation_from_error(
    default_section: &'static str,
    fallback_key: &'static str,
    error: &Error,
) -> ConfigViolation {
    let message = error_detail(error);
    let key = message
        .split_whitespace()
        .next()
        .filter(|token| {
            token.contains('.')
                && !token.contains('=')
                && token.chars().all(|character| {
                    character.is_ascii_alphanumeric() || character == '.' || character == '_'
                })
        })
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_key.to_owned());
    ConfigViolation::new(default_section, key, message)
}

/// Strip the error-variant prefix so violations read like configuration text.
///
/// `Error::Di("x")` renders as `"DI error: x"`; the aggregate report already
/// says this is a startup configuration failure, so the variant prefix is noise
/// for an operator (and would hide the `Jwt.`/`Turn.` key prefix).
fn error_detail(error: &Error) -> String {
    match error {
        Error::Di(message)
        | Error::Internal(message)
        | Error::Http(message)
        | Error::Message(message) => message.clone(),
        other => other.to_string(),
    }
}

fn collect_tls_client_auth(options: &AppOptions, violations: &mut Vec<ConfigViolation>) {
    // Retained validator: keep the exact rule set (and first-failure order) the
    // host has always applied, but report it as one violation of the aggregate.
    if let Err(error) = crate::server::validate_tls_client_auth(options) {
        violations.push(violation_from_error("Tls", "Tls.ClientAuth", &error));
    }
}

fn collect_engine_server(
    options: &AppOptions,
    mode: AppMode,
    violations: &mut Vec<ConfigViolation>,
) {
    if let Err(error) =
        webx_engine::config::validate_request_timeout(mode, options.server.request_timeout_seconds)
    {
        violations.push(violation_from_error(
            "Server",
            "Server.RequestTimeoutSeconds",
            &error,
        ));
    }
    if let Err(error) =
        webx_engine::config::validate_admission_timeout(mode, options.server.admission_timeout_ms)
    {
        violations.push(violation_from_error(
            "Server",
            "Server.AdmissionTimeoutMs",
            &error,
        ));
    }
    // The engine runs with the effective (clamped) value, so validate that one:
    // `0` is bumped to `1` and is not a new build-time failure.
    let effective_max_connections = options.app.max_connections.max(1);
    if let Err(error) = webx_engine::config::validate_max_connections(effective_max_connections) {
        violations.push(violation_from_error("App", "App.MaxConnections", &error));
    }
}

fn collect_legacy_rate_limit_trust(
    options: &AppOptions,
    mode: AppMode,
    violations: &mut Vec<ConfigViolation>,
) {
    let legacy_keys_configured =
        options.rate_limit.trust_proxy || !options.rate_limit.trusted_proxies.is_empty();
    if legacy_keys_configured {
        if let Some(violation) = legacy_rate_limit_trust_keys_violation(mode) {
            violations.push(violation);
        }
    }
}

fn sort_violations(violations: &mut [ConfigViolation]) {
    violations.sort_by(|left, right| {
        (left.section, left.key.as_str(), left.message.as_str()).cmp(&(
            right.section,
            right.key.as_str(),
            right.message.as_str(),
        ))
    });
}
