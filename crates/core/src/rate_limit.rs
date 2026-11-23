//! Rate-limiting port.
//!
//! The framework owns the middleware contract (`429` + `Retry-After` +
//! `problem+json`); the counter store is a port so a distributed deployment can
//! back it with Redis or another shared store without changing handlers.
//!
//! [`IRateLimiter`] is called once per request with an opaque key — the client
//! IP by default. Implementations must be cheap and bounded: the hot path may
//! not block on I/O outside their own store.

use std::sync::Arc;

/// Outcome of one rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDecision {
    /// Whether the request may proceed.
    pub allowed: bool,
    /// Whole seconds the client should wait before retrying (RFC 6585).
    ///
    /// Always at least `1` when `allowed` is `false`; reporting `0` would invite
    /// an immediate retry storm.
    pub retry_after_secs: u64,
}

impl RateLimitDecision {
    /// Allow the request.
    pub const fn allow() -> Self {
        Self {
            allowed: true,
            retry_after_secs: 1,
        }
    }

    /// Deny the request, telling the client when to retry.
    pub const fn deny(retry_after_secs: u64) -> Self {
        Self {
            allowed: false,
            retry_after_secs: if retry_after_secs == 0 {
                1
            } else {
                retry_after_secs
            },
        }
    }
}

/// Distributed-capable rate-limit store.
///
/// The default implementation is an in-process, sharded token bucket
/// (`webx_host::rate_limit::InMemoryRateLimiter`). A multi-instance deployment
/// that needs a cluster-wide limit implements this port over a shared store.
#[async_trait::async_trait]
pub trait IRateLimiter: Send + Sync {
    /// Consume one token for `key` and report whether the request may proceed.
    async fn check(&self, key: &str) -> RateLimitDecision;
}

/// What identifies one rate-limit budget.
///
/// `Ip` protects unauthenticated traffic (the pre-auth middleware position);
/// `User`/`Tenant`/`ApiKey` need the authenticated principal or a credential
/// header, so policy-based limiting runs after authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitSubject {
    /// Client IP resolved by the `ClientInfo` middleware: the peer address, or
    /// the trusted forwarded client when forwarding is configured. Consumers
    /// read it through [`IHttpContext::client_info`](crate::http::IHttpContext::client_info).
    Ip,
    /// Authenticated subject (`sub` claim).
    User,
    /// Tenant claim (`tenant_id`); requests without one fall back to the IP.
    Tenant,
    /// Presented API key / bearer credential, hashed before it becomes a key.
    ApiKey,
}

/// Which requests a [`RateLimitRule`] applies to.
///
/// Matching happens on the **request path** (the policy middleware runs before
/// routing), so `Prefix("/api/im")` covers every route under that area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitRoute {
    /// Every request (lowest precedence).
    Any,
    /// One exact path.
    Exact(String),
    /// Any path starting with the prefix (longer prefixes beat shorter ones).
    Prefix(String),
}

impl RateLimitRoute {
    /// Exact-path rule.
    pub fn exact(path: impl Into<String>) -> Self {
        Self::Exact(path.into())
    }

    /// Path-prefix rule.
    pub fn prefix(prefix: impl Into<String>) -> Self {
        Self::Prefix(prefix.into())
    }

    /// Match specificity: exact wins, then longer prefixes, then `Any`.
    fn weight(&self, path: &str) -> Option<usize> {
        match self {
            Self::Exact(exact) if path == exact => Some(usize::MAX),
            Self::Prefix(prefix) if path.starts_with(prefix.as_str()) => Some(prefix.len()),
            Self::Prefix(_) | Self::Exact(_) => None,
            Self::Any => Some(0),
        }
    }
}

/// One route + subject + budget.
pub struct RateLimitRule {
    /// Which requests this rule applies to.
    pub route: RateLimitRoute,
    /// Which request attribute is the budget owner.
    pub subject: RateLimitSubject,
    /// Store that holds this rule's budget (its rate/burst is fixed at construction).
    pub limiter: Arc<dyn IRateLimiter>,
}

impl RateLimitRule {
    /// A rule for `route` keyed by `subject` over `limiter`.
    pub fn new(
        route: RateLimitRoute,
        subject: RateLimitSubject,
        limiter: Arc<dyn IRateLimiter>,
    ) -> Self {
        Self {
            route,
            subject,
            limiter,
        }
    }
}

/// Ordered set of rules; resolution picks the most specific path match.
///
/// Precedence: exact path > longest prefix > `Any`. Equal specificity keeps the
/// first declared rule. The policy owns one limiter per rule, so different areas
/// can have different budgets without a second middleware contract.
pub struct RateLimitPolicy {
    rules: Vec<RateLimitRule>,
}

impl Default for RateLimitPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitPolicy {
    /// An empty policy (matches nothing).
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Append a rule (declaration order breaks specificity ties).
    pub fn with_rule(mut self, rule: RateLimitRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Whether the policy has no rules (the middleware then does nothing).
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Number of rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Resolve the rule for one request path.
    pub fn resolve(&self, path: &str) -> Option<&RateLimitRule> {
        let mut best: Option<(&RateLimitRule, usize)> = None;
        for rule in &self.rules {
            let Some(weight) = rule.route.weight(path) else {
                continue;
            };
            if best.is_none_or(|(_, current)| weight > current) {
                best = Some((rule, weight));
            }
        }
        best.map(|(rule, _)| rule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct NoopLimiter;

    #[async_trait::async_trait]
    impl IRateLimiter for NoopLimiter {
        async fn check(&self, _key: &str) -> RateLimitDecision {
            RateLimitDecision::allow()
        }
    }

    fn rule(route: RateLimitRoute, subject: RateLimitSubject) -> RateLimitRule {
        RateLimitRule::new(route, subject, Arc::new(NoopLimiter))
    }

    #[test]
    fn resolution_prefers_exact_then_longest_prefix_then_any() {
        let exact = RateLimitRule::new(
            RateLimitRoute::exact("/api/im/send"),
            RateLimitSubject::User,
            Arc::new(NoopLimiter),
        );
        let long_prefix = RateLimitRule::new(
            RateLimitRoute::prefix("/api/im"),
            RateLimitSubject::ApiKey,
            Arc::new(NoopLimiter),
        );
        let short_prefix = RateLimitRule::new(
            RateLimitRoute::prefix("/api"),
            RateLimitSubject::Ip,
            Arc::new(NoopLimiter),
        );
        let any = RateLimitRule::new(
            RateLimitRoute::Any,
            RateLimitSubject::Ip,
            Arc::new(NoopLimiter),
        );

        let policy = RateLimitPolicy::new()
            .with_rule(short_prefix)
            .with_rule(any)
            .with_rule(long_prefix)
            .with_rule(exact);

        assert_eq!(
            policy.resolve("/api/im/send").expect("exact").subject,
            RateLimitSubject::User
        );
        assert_eq!(
            policy.resolve("/api/im/history").expect("prefix").subject,
            RateLimitSubject::ApiKey
        );
        assert_eq!(
            policy.resolve("/api/other").expect("short").subject,
            RateLimitSubject::Ip
        );
        assert_eq!(
            policy.resolve("/public").expect("any").route,
            RateLimitRoute::Any
        );
    }

    #[test]
    fn empty_policy_and_equal_specificity() {
        let policy = RateLimitPolicy::new();
        assert!(policy.is_empty());
        assert!(policy.resolve("/x").is_none());

        let first = rule(RateLimitRoute::prefix("/a"), RateLimitSubject::Ip);
        let second = rule(RateLimitRoute::prefix("/a"), RateLimitSubject::User);
        let policy = RateLimitPolicy::new().with_rule(first).with_rule(second);
        assert_eq!(policy.rule_count(), 2);
        assert_eq!(
            policy.resolve("/a/b").expect("match").subject,
            RateLimitSubject::Ip,
            "the first declared rule wins a tie"
        );
    }
}
