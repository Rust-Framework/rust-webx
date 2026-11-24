//! Pluggable authentication schemes: JWT and API keys on one host.
//!
//! [`AuthenticationMiddleware`] runs a chain of [`IAuthenticationScheme`]s in
//! registration order. The first scheme whose `matches` returns `true` handles
//! the request; there is no per-route selector yet, which keeps the hot path a
//! cheap shape check:
//!
//! * [`JwtScheme`] recognizes `Authorization: Bearer a.b.c` (the compact JWT
//!   shape) and keeps the framework's "invalid token = anonymous" behaviour.
//! * [`ApiKeyScheme`] recognizes `Authorization: Bearer sk-…` or `x-api-key:
//!   sk-…` and delegates verification to an [`IApiKeyValidator`]. An explicitly
//!   presented but invalid key is a `401`, not an anonymous request.
//!
//! ```ignore
//! Host::builder()
//!     .add_authentication_scheme(Arc::new(ApiKeyScheme::new(validator)))
//!     .add_authentication() // JWT scheme appended after explicit schemes
//!     .build();
//! ```

use async_trait::async_trait;
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::Instrument;
use webx_core::auth::{IAuthenticationHandler, IAuthenticationScheme, IClaims};
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::metrics::{Counter, Histogram, IMeter};
use webx_core::middleware::IMiddleware;
use webx_core::security::ISessionStore;

/// Counter of authentication decisions.
pub const AUTH_REQUESTS_METRIC: &str = "auth.requests";
/// Latency histogram of one authentication decision, in milliseconds.
pub const AUTH_DURATION_METRIC: &str = "auth.duration";

/// Claims produced by an API-key validator.
///
/// The key id is the subject; tenant/roles/permissions are copied into the raw
/// claim map so `IdentityUser::from_claims` and `RequestUser` behave exactly as
/// they do for JWT claims.
#[derive(Debug, Clone, Default)]
pub struct ApiKeyClaims {
    subject: String,
    roles: Vec<String>,
    permissions: Vec<String>,
    raw: HashMap<String, String>,
}

impl ApiKeyClaims {
    /// Claims for the key id `subject`.
    pub fn new(subject: impl Into<String>) -> Self {
        let subject = subject.into();
        let mut raw = HashMap::new();
        raw.insert("sub".to_string(), subject.clone());
        Self {
            subject,
            roles: Vec::new(),
            permissions: Vec::new(),
            raw,
        }
    }

    /// Add the `tenant_id` claim (used by `require_tenant`).
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.raw.insert("tenant_id".to_string(), tenant.into());
        self
    }

    /// Set the roles claim.
    pub fn with_roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    /// Set the permissions claim.
    pub fn with_permissions<I, S>(mut self, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    /// Add an arbitrary string claim.
    pub fn with_claim(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.raw.insert(key.into(), value.into());
        self
    }
}

impl IClaims for ApiKeyClaims {
    fn subject(&self) -> &str {
        &self.subject
    }

    fn roles(&self) -> &[String] {
        &self.roles
    }

    fn permissions(&self) -> &[String] {
        &self.permissions
    }

    fn claims(&self) -> &HashMap<String, String> {
        &self.raw
    }

    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(self.clone())
    }
}

/// Verifies a presented API key.
///
/// Implementations look the key up in a store (cache + database), check status
/// and expiry, and return the key's claims. Returning `Ok(None)` means "unknown
/// or revoked"; the scheme turns that into a 401 without echoing the key.
#[async_trait]
pub trait IApiKeyValidator: Send + Sync {
    /// Validate `presented_key` and return its claims when it is active.
    async fn validate(&self, presented_key: &str) -> Result<Option<Box<dyn IClaims>>>;
}

/// Which header an [`ApiKeyScheme`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiKeyHeader {
    /// Accept `Authorization: Bearer sk-…` and `x-api-key: sk-…`.
    #[default]
    Both,
    /// Only `Authorization: Bearer sk-…`.
    AuthorizationBearer,
    /// Only `x-api-key: sk-…`.
    XApiKey,
}

/// Bounded fixed-window counter for failed API-key attempts.
///
/// A gateway exposes `sk-` keys to the internet; without a failure limit an
/// attacker can brute-force secrets at request speed. The key is
/// `{client_ip}|{key_prefix}`, so one bad key cannot lock out a different key
/// and one client cannot hammer many prefixes from one address unnoticed.
/// This is abuse protection, not authorization: a valid key still authenticates
/// while another prefix is throttled.
#[derive(Debug)]
pub struct AuthFailureLimiter {
    max_failures: u32,
    window: Duration,
    entries: Mutex<HashMap<String, FailureWindow>>,
}

#[derive(Debug)]
struct FailureWindow {
    failures: u32,
    started: Instant,
}

impl AuthFailureLimiter {
    /// Allow `max_failures` failures per `window` for one throttle key.
    pub fn new(max_failures: u32, window: Duration) -> Self {
        Self {
            max_failures: max_failures.max(1),
            window: window.max(Duration::from_millis(1)),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Whether another attempt for `key` may proceed.
    pub fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match entries.get(key) {
            None => true,
            Some(entry) if now.duration_since(entry.started) >= self.window => {
                entries.remove(key);
                true
            }
            Some(entry) => entry.failures < self.max_failures,
        }
    }

    /// Record a rejected attempt.
    pub fn record_failure(&self, key: &str) {
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Keep the map bounded on a public endpoint: drop windows that have
        // already expired instead of letting the key space grow forever.
        if entries.len() > 4096 {
            entries.retain(|_, entry| now.duration_since(entry.started) < self.window);
        }
        let entry = entries.entry(key.to_string()).or_insert(FailureWindow {
            failures: 0,
            started: now,
        });
        if now.duration_since(entry.started) >= self.window {
            entry.failures = 0;
            entry.started = now;
        }
        entry.failures = entry.failures.saturating_add(1);
    }

    /// Clear the window after a success (the caller proved the key).
    pub fn reset(&self, key: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(key);
    }
}

/// API-key authentication scheme.
///
/// The scheme is deliberately storage-agnostic: it extracts the credential,
/// checks the configured prefix, and delegates the actual verification to
/// [`IApiKeyValidator`]. HMAC storage, revocation and per-key quotas belong to
/// the validator/store implementation, not to the scheme.
pub struct ApiKeyScheme {
    validator: Arc<dyn IApiKeyValidator>,
    header: ApiKeyHeader,
    prefix: String,
    failure_limiter: Option<AuthFailureLimiter>,
}

impl ApiKeyScheme {
    /// A scheme reading both headers and accepting keys with the `sk-` prefix.
    pub fn new(validator: Arc<dyn IApiKeyValidator>) -> Self {
        Self {
            validator,
            header: ApiKeyHeader::Both,
            prefix: "sk-".to_string(),
            failure_limiter: None,
        }
    }

    /// Restrict which header is read.
    pub fn header(mut self, header: ApiKeyHeader) -> Self {
        self.header = header;
        self
    }

    /// Change the accepted key prefix (for example `"ak_"`).
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Throttle invalid presentations per client IP + key prefix.
    ///
    /// After `max_failures` rejected attempts inside `window`, further
    /// attempts with the same IP/prefix get `429` without reaching the store.
    /// A successful authentication clears that bucket. `max_failures == 0`
    /// disables the limiter (default).
    pub fn with_failure_limit(mut self, max_failures: u32, window: Duration) -> Self {
        self.failure_limiter =
            (max_failures > 0).then(|| AuthFailureLimiter::new(max_failures, window));
        self
    }

    /// Throttle bucket key: client address + a short key prefix.
    fn throttle_key(&self, ctx: &dyn IHttpContext) -> String {
        let client = ctx
            .request()
            .remote_addr()
            .map(|address| address.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let presented = self.credential(ctx).unwrap_or_default();
        let prefix: String = presented
            .chars()
            .take(self.prefix.chars().count().saturating_add(8))
            .collect();
        format!("{client}|{prefix}")
    }

    fn credential<'a>(&self, ctx: &'a dyn IHttpContext) -> Option<&'a str> {
        let bearer = if matches!(
            self.header,
            ApiKeyHeader::Both | ApiKeyHeader::AuthorizationBearer
        ) {
            ctx.request()
                .header("authorization")
                .and_then(bearer_token)
                .filter(|token| self.accepts(token))
        } else {
            None
        };
        if bearer.is_some() {
            return bearer;
        }

        if matches!(self.header, ApiKeyHeader::Both | ApiKeyHeader::XApiKey) {
            return ctx
                .request()
                .header("x-api-key")
                .map(str::trim)
                .filter(|token| self.accepts(token));
        }
        None
    }

    fn accepts(&self, credential: &str) -> bool {
        credential.len() > self.prefix.len() && credential.starts_with(&self.prefix)
    }
}

#[async_trait]
impl IAuthenticationScheme for ApiKeyScheme {
    fn name(&self) -> &'static str {
        "api-key"
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        self.credential(ctx).is_some()
    }

    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        // Own the presented key: the throttle key also needs it, and the 429
        // branch mutates the response (which would conflict with a ctx borrow).
        let Some(key) = self.credential(ctx).map(str::to_string) else {
            return Ok(None);
        };
        let throttle = self
            .failure_limiter
            .as_ref()
            .map(|limiter| (limiter, self.throttle_key(ctx)));
        if let Some((limiter, bucket)) = &throttle {
            if !limiter.allow(bucket) {
                ctx.response_mut().set_header("retry-after", "60");
                return Err(Error::status(
                    429,
                    "too many invalid API key attempts".to_string(),
                ));
            }
        }
        match self.validator.validate(&key).await? {
            Some(claims) => {
                if let Some((limiter, bucket)) = &throttle {
                    limiter.reset(bucket);
                }
                Ok(Some(claims))
            }
            // A presented key is an explicit credential: reject it instead of
            // silently continuing anonymously, so the client gets a clear 401.
            None => {
                if let Some((limiter, bucket)) = &throttle {
                    limiter.record_failure(bucket);
                }
                Err(Error::unauthorized("invalid API key".to_string()))
            }
        }
    }
}

/// JWT bearer scheme.
///
/// Wraps the existing [`JwtAuth`](crate::auth_jwt::JwtAuth) handler and the
/// optional session store; the "invalid token = anonymous" behaviour is
/// unchanged. `matches` only recognizes the compact JWT shape (`a.b.c`), so an
/// `sk-` key is left for [`ApiKeyScheme`].
pub struct JwtScheme {
    handler: Arc<dyn IAuthenticationHandler>,
    session_store: Option<Arc<dyn ISessionStore>>,
}

impl JwtScheme {
    /// Build a scheme over an authentication handler.
    pub fn new(
        handler: Arc<dyn IAuthenticationHandler>,
        session_store: Option<Arc<dyn ISessionStore>>,
    ) -> Self {
        Self {
            handler,
            session_store,
        }
    }
}

#[async_trait]
impl IAuthenticationScheme for JwtScheme {
    fn name(&self) -> &'static str {
        "jwt"
    }

    fn matches(&self, ctx: &dyn IHttpContext) -> bool {
        ctx.request()
            .header("authorization")
            .and_then(bearer_token)
            .map(|token| token.matches('.').count() == 2)
            .unwrap_or(false)
    }

    async fn authenticate(&self, ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        crate::auth_jwt::authenticate_with_revocation(
            &*self.handler,
            self.session_store.as_ref(),
            ctx,
        )
        .await
    }
}

/// Outcome of one authentication decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthOutcome {
    /// A credential was presented and verified; claims were set.
    Authenticated,
    /// No credential matched, or a matched scheme returned `Ok(None)`.
    Anonymous,
    /// A presented credential was rejected (`401`).
    Unauthorized,
    /// Authenticated but not allowed (`403`, normally from authorization).
    Forbidden,
    /// Throttled (`429`).
    RateLimited,
    /// A backend/scheme failure (`5xx`); fail-closed.
    Error,
}

impl AuthOutcome {
    /// Stable metric/log label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authenticated => "ok",
            Self::Anonymous => "anonymous",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::RateLimited => "rate_limited",
            Self::Error => "error",
        }
    }
}

/// Per-scheme auth metrics, pre-bound so the hot path only looks up a handle.
pub struct AuthMetrics {
    counters: HashMap<(&'static str, AuthOutcome), Arc<dyn Counter>>,
    durations: HashMap<&'static str, Arc<dyn Histogram>>,
    no_scheme_counter: Arc<dyn Counter>,
    no_scheme_duration: Arc<dyn Histogram>,
}

impl AuthMetrics {
    /// Bind one counter per (scheme, outcome) and one histogram per scheme.
    pub fn new(meter: &dyn IMeter, schemes: &[Arc<dyn IAuthenticationScheme>]) -> Self {
        let mut counters = HashMap::new();
        let mut durations = HashMap::new();
        for scheme in schemes {
            let name = scheme.name();
            durations.insert(
                name,
                meter.histogram(AUTH_DURATION_METRIC, &[("scheme", name)]),
            );
            for outcome in [
                AuthOutcome::Authenticated,
                AuthOutcome::Anonymous,
                AuthOutcome::Unauthorized,
                AuthOutcome::Forbidden,
                AuthOutcome::RateLimited,
                AuthOutcome::Error,
            ] {
                counters.insert(
                    (name, outcome),
                    meter.counter(
                        AUTH_REQUESTS_METRIC,
                        &[("scheme", name), ("outcome", outcome.as_str())],
                    ),
                );
            }
        }
        let no_scheme_counter = meter.counter(
            AUTH_REQUESTS_METRIC,
            &[
                ("scheme", "none"),
                ("outcome", AuthOutcome::Anonymous.as_str()),
            ],
        );
        let no_scheme_duration = meter.histogram(AUTH_DURATION_METRIC, &[("scheme", "none")]);
        Self {
            counters,
            durations,
            no_scheme_counter,
            no_scheme_duration,
        }
    }

    /// Record one scheme decision.
    pub fn record(&self, scheme: &'static str, outcome: AuthOutcome, elapsed: Duration) {
        if let Some(counter) = self.counters.get(&(scheme, outcome)) {
            counter.inc();
        }
        if let Some(histogram) = self.durations.get(scheme) {
            histogram.record(elapsed.as_secs_f64() * 1000.0);
        }
    }

    /// Record a request that no scheme recognized.
    pub fn record_no_scheme(&self, elapsed: Duration) {
        self.no_scheme_counter.inc();
        self.no_scheme_duration
            .record(elapsed.as_secs_f64() * 1000.0);
    }
}

/// Composes authentication schemes into one middleware.
///
/// The first scheme that matches handles the request. If no scheme matches, the
/// request continues anonymously (public endpoints and login stay reachable,
/// and protected endpoints are rejected later by `#[authorize]` or identity
/// binding).
pub struct AuthenticationMiddleware {
    schemes: Vec<Arc<dyn IAuthenticationScheme>>,
    metrics: Option<AuthMetrics>,
    route_policies: Vec<(String, Vec<&'static str>)>,
}

impl AuthenticationMiddleware {
    /// Build a middleware from an ordered scheme list.
    pub fn new(schemes: Vec<Arc<dyn IAuthenticationScheme>>) -> Self {
        Self {
            schemes,
            metrics: None,
            route_policies: Vec::new(),
        }
    }

    /// Restrict which schemes may authenticate a path prefix.
    ///
    /// The first matching prefix wins; a path with no policy keeps the full
    /// chain. A policy never turns an unauthenticated request into a 401 by
    /// itself — protected routes still enforce `#[authorize]`/identity.
    pub fn with_route_policies(mut self, policies: Vec<(String, Vec<&'static str>)>) -> Self {
        self.route_policies = policies;
        self
    }

    fn allowed_schemes(&self, path: &str) -> Option<&[&'static str]> {
        self.route_policies
            .iter()
            .find(|(prefix, _)| {
                let prefix = prefix.trim_end_matches('/');
                path == prefix
                    || path
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|(_, names)| names.as_slice())
    }

    /// Attach auth metrics (`auth.requests` / `auth.duration`).
    pub fn with_metrics(mut self, meter: &dyn IMeter) -> Self {
        self.metrics = Some(AuthMetrics::new(meter, &self.schemes));
        self
    }

    /// Scheme names in registration order, for startup diagnostics.
    pub fn names(&self) -> Vec<&'static str> {
        self.schemes.iter().map(|scheme| scheme.name()).collect()
    }

    fn record_error(
        &self,
        scheme: &'static str,
        error: &Error,
        elapsed: Duration,
        trace_id: Option<&str>,
    ) -> AuthOutcome {
        let outcome = match error.status_code() {
            401 => AuthOutcome::Unauthorized,
            403 => AuthOutcome::Forbidden,
            429 => AuthOutcome::RateLimited,
            _ => AuthOutcome::Error,
        };
        if let Some(metrics) = &self.metrics {
            metrics.record(scheme, outcome, elapsed);
        }
        match outcome {
            AuthOutcome::Error => tracing::warn!(
                scheme,
                error = %error,
                trace_id = ?trace_id,
                "authentication backend failed"
            ),
            _ => tracing::debug!(
                scheme,
                outcome = outcome.as_str(),
                trace_id = ?trace_id,
                "authentication rejected"
            ),
        }
        outcome
    }
}

#[async_trait]
impl IMiddleware for AuthenticationMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        // One `auth.authenticate` span per request, even for anonymous ones:
        // the `outcome` field (ok/anonymous/unauthorized/forbidden/rate_limited/
        // error) is the stable contract. Scheme metrics and logs stay as they
        // are; the span is the trace-side view of the same decision.
        let span = tracing::debug_span!(
            target: "webx.http",
            "auth.authenticate",
            scheme = tracing::field::Empty,
            outcome = tracing::field::Empty,
        );
        async {
            let started = Instant::now();
            let allowed = self.allowed_schemes(ctx.request().path());
            for scheme in &self.schemes {
                if allowed.is_some_and(|allowed| !allowed.contains(&scheme.name())) {
                    continue;
                }
                if !scheme.matches(ctx) {
                    continue;
                }
                let scheme_started = Instant::now();
                match scheme.authenticate(ctx).await {
                    Ok(Some(claims)) => {
                        ctx.set_claims(claims);
                        if let Some(metrics) = &self.metrics {
                            metrics.record(
                                scheme.name(),
                                AuthOutcome::Authenticated,
                                scheme_started.elapsed(),
                            );
                        }
                        tracing::debug!(
                            scheme = scheme.name(),
                            outcome = "ok",
                            trace_id = ?ctx.trace_id(),
                            "authentication"
                        );
                        let span = tracing::Span::current();
                        span.record("scheme", scheme.name());
                        span.record("outcome", AuthOutcome::Authenticated.as_str());
                    }
                    Ok(None) => {
                        if let Some(metrics) = &self.metrics {
                            metrics.record(
                                scheme.name(),
                                AuthOutcome::Anonymous,
                                scheme_started.elapsed(),
                            );
                        }
                        tracing::debug!(
                            scheme = scheme.name(),
                            outcome = "anonymous",
                            trace_id = ?ctx.trace_id(),
                            "authentication"
                        );
                        let span = tracing::Span::current();
                        span.record("scheme", scheme.name());
                        span.record("outcome", AuthOutcome::Anonymous.as_str());
                    }
                    Err(error) => {
                        let outcome = self.record_error(
                            scheme.name(),
                            &error,
                            scheme_started.elapsed(),
                            ctx.trace_id(),
                        );
                        let span = tracing::Span::current();
                        span.record("scheme", scheme.name());
                        span.record("outcome", outcome.as_str());
                        return Err(error);
                    }
                }
                return Ok(ControlFlow::Continue(()));
            }
            if let Some(metrics) = &self.metrics {
                metrics.record_no_scheme(started.elapsed());
            }
            let span = tracing::Span::current();
            span.record("scheme", "none");
            span.record("outcome", AuthOutcome::Anonymous.as_str());
            Ok(ControlFlow::Continue(()))
        }
        .instrument(span)
        .await
    }
}

/// Extract the token from an RFC 6750 `Bearer` header.
fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_is_case_insensitive_and_trims() {
        assert_eq!(bearer_token("Bearer abc.def.ghi"), Some("abc.def.ghi"));
        assert_eq!(bearer_token("bearer   sk-123  "), Some("sk-123"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer   "), None);
        assert_eq!(bearer_token("Bearer"), None);
    }

    #[test]
    fn api_key_claims_expose_subject_tenant_and_raw_claims() {
        let claims = ApiKeyClaims::new("key-1")
            .with_tenant("tenant-1")
            .with_roles(["admin"])
            .with_permissions(["model.invoke"])
            .with_claim("key_prefix", "sk-abc");

        assert_eq!(claims.subject(), "key-1");
        assert_eq!(claims.get_tenantid(), Some("tenant-1"));
        assert_eq!(claims.roles(), ["admin"]);
        assert_eq!(claims.permissions(), ["model.invoke"]);
        assert_eq!(claims.claims().get("key_prefix").unwrap(), "sk-abc");
        assert_eq!(claims.claims().get("sub").unwrap(), "key-1");

        let cloned: Box<dyn IClaims> = claims.clone_box();
        assert_eq!(cloned.subject(), "key-1");
    }

    #[test]
    fn failure_limiter_blocks_after_the_threshold_and_recovers_after_the_window() {
        let limiter = AuthFailureLimiter::new(2, Duration::from_millis(20));
        assert!(limiter.allow("ip|sk-a"));
        limiter.record_failure("ip|sk-a");
        limiter.record_failure("ip|sk-a");
        assert!(!limiter.allow("ip|sk-a"), "threshold reached");
        assert!(
            limiter.allow("ip|sk-b"),
            "a different key prefix is not throttled"
        );

        std::thread::sleep(Duration::from_millis(30));
        assert!(limiter.allow("ip|sk-a"), "the window expires");
    }

    #[test]
    fn failure_limiter_reset_clears_the_bucket() {
        let limiter = AuthFailureLimiter::new(1, Duration::from_secs(60));
        limiter.record_failure("ip|sk-a");
        assert!(!limiter.allow("ip|sk-a"));
        limiter.reset("ip|sk-a");
        assert!(limiter.allow("ip|sk-a"));
    }
}
