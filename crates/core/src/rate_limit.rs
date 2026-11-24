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
///
/// Every leaf subject falls back to the client IP when its principal/credential
/// is absent, so anonymous traffic is still limited (per address instead of per
/// principal). [`Self::Composite`] combines several leaves into one budget.
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
    /// One budget shared by the combination of its parts, e.g.
    /// `Composite(&[Tenant, User])` limits each user separately *within* each
    /// tenant (where `Tenant` alone would pool the whole tenant).
    ///
    /// Each part resolves on its own with the leaf fallback above (a missing
    /// principal falls back to the client IP, it is never skipped), and the
    /// resolved parts are joined into one key in declaration order. Nested
    /// composites are flattened, so `Composite(&[Composite(&[Tenant, User]), Ip])`
    /// budgets exactly like `Composite(&[Tenant, User, Ip])`. The slice is
    /// `'static` so the enum stays [`Copy`].
    Composite(&'static [RateLimitSubject]),
}

/// Which requests a [`RateLimitRule`] applies to.
///
/// Matching happens on the **raw request path**, because the policy middleware
/// runs before routing (see [`RateLimitRule`]): `Prefix("/api/im")` covers every
/// route under that area and `Pattern("/api/tenants/{id}/invitations")` covers
/// one route shape. The router's own matched pattern
/// ([`IHttpRequest::route_pattern`](crate::http::IHttpRequest::route_pattern)) is
/// therefore not available here — routes are matched in the pipeline terminal —
/// so [`Self::Pattern`] implements a documented **subset** of the router
/// (`matchit`) syntax rather than a second routing language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitRoute {
    /// Every request (lowest precedence).
    Any,
    /// One exact path.
    Exact(String),
    /// Segment-level template; see [`Self::pattern`].
    Pattern(String),
    /// Any path starting with the prefix (longer prefixes beat shorter ones).
    Prefix(String),
}

impl RateLimitRoute {
    /// Exact-path rule.
    pub fn exact(path: impl Into<String>) -> Self {
        Self::Exact(path.into())
    }

    /// Segment-level template rule, e.g. `"/api/tenants/{id}/invitations"`.
    ///
    /// A `{param}` segment matches exactly one non-empty path segment and every
    /// other segment is literal, so the template matches
    /// `"/api/tenants/42/invitations"` but not `"/api/tenants/42"` (segment
    /// count), `"/api/tenants//invitations"` (empty parameter) or
    /// `"/api/tenant/42/invitations"` (literal segment differs). The path is
    /// matched as received: query strings are not part of
    /// [`IHttpRequest::path`](crate::http::IHttpRequest::path), percent-encoded
    /// segments are not decoded.
    ///
    /// # Supported subset of the router syntax
    ///
    /// The router (`matchit`) supports parameters with prefixes/suffixes
    /// (`"/img-{id}.png"`), catch-alls (`"/{*rest}"`) and `{{`/`}}` escapes.
    /// Rate-limit patterns support **none** of them: a segment is a parameter
    /// only when it is exactly `{name}` (non-empty name without `{`, `}` or `*`);
    /// catch-alls are expressed as [`Self::prefix`] instead. This keeps the
    /// pattern language a strict subset of the router's semantics, not a second
    /// dialect.
    ///
    /// # Panics
    ///
    /// Panics when the template is malformed (unclosed brace, empty parameter
    /// name, `{*rest}`, within-segment parameter, brace escape). A policy is
    /// assembled during startup, so a typo fails fast instead of silently
    /// disabling the budget (which would leave the route unthrottled) in a
    /// release build. The `Pattern` variant is still public; constructing it
    /// directly with a malformed template is safe but never matches — use this
    /// constructor.
    pub fn pattern(template: impl Into<String>) -> Self {
        let template = template.into();
        assert!(
            valid_pattern(&template),
            "malformed rate-limit route pattern `{template}`: only full `{{name}}` \
             segments are supported (non-empty name, no `{{`, `}}` or `*`); use \
             RateLimitRoute::prefix for a catch-all"
        );
        Self::Pattern(template)
    }

    /// Path-prefix rule.
    pub fn prefix(prefix: impl Into<String>) -> Self {
        Self::Prefix(prefix.into())
    }

    /// Most specific match of this rule for `path`, if it applies.
    fn specificity(&self, path: &str) -> Option<Specificity> {
        match self {
            Self::Exact(exact) => (path == exact).then_some(Specificity::Exact),
            Self::Pattern(template) => pattern_specificity(template, path),
            Self::Prefix(prefix) => path
                .starts_with(prefix.as_str())
                .then_some(Specificity::Prefix(prefix.len())),
            Self::Any => Some(Specificity::Any),
        }
    }
}

/// Match specificity of one rule for one path.
///
/// The derived ordering encodes the documented precedence: `Exact` > `Pattern` >
/// `Prefix` > `Any`. Among prefixes a longer prefix wins; among patterns more
/// literal segments win, then more literal characters (see
/// [`pattern_specificity`]). Equal specificity keeps the first declared rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Specificity {
    Any,
    Prefix(usize),
    Pattern { literals: usize, chars: usize },
    Exact,
}

/// One template segment: `{name}` (matches one non-empty path segment) or a
/// literal compared whole.
enum Segment<'a> {
    Param,
    Literal(&'a str),
}

/// Classify a template segment; `None` means the template is malformed.
fn classify_segment(segment: &str) -> Option<Segment<'_>> {
    if segment.contains(['{', '}']) {
        let name = segment.strip_prefix('{')?.strip_suffix('}')?;
        (!name.is_empty() && !name.contains(['{', '}', '*'])).then_some(Segment::Param)
    } else {
        Some(Segment::Literal(segment))
    }
}

/// Whether every segment of `template` is a supported parameter or a literal.
fn valid_pattern(template: &str) -> bool {
    template
        .split('/')
        .all(|segment| classify_segment(segment).is_some())
}

/// Match `path` against a segment-level template, returning its specificity.
///
/// A malformed template never matches: a rule built from it (only possible by
/// constructing [`RateLimitRoute::Pattern`] directly) is inert instead of
/// matching something the router would not.
fn pattern_specificity(template: &str, path: &str) -> Option<Specificity> {
    let mut literals = 0usize;
    let mut chars = 0usize;
    let mut template_segments = template.split('/');
    let mut path_segments = path.split('/');
    loop {
        match (template_segments.next(), path_segments.next()) {
            (None, None) => return Some(Specificity::Pattern { literals, chars }),
            (Some(template_segment), Some(path_segment)) => {
                match classify_segment(template_segment)? {
                    // `{param}` needs exactly one non-empty segment.
                    Segment::Param if path_segment.is_empty() => return None,
                    Segment::Param => {}
                    Segment::Literal(literal) if literal != path_segment => return None,
                    Segment::Literal(literal) => {
                        literals += 1;
                        chars += literal.len();
                    }
                }
            }
            // Different segment counts only match if both ended together above.
            _ => return None,
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
/// Precedence: exact path > pattern > longest prefix > `Any`. Patterns are
/// ordered by literal content — more literal segments win, then more literal
/// characters — so `"/api/tenants/{id}/invitations"` beats `"/api/{area}/{id}"`
/// for the same path. Equal specificity keeps the first declared rule. The policy
/// owns one limiter per rule, so different areas can have different budgets
/// without a second middleware contract.
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
        let mut best: Option<(&RateLimitRule, Specificity)> = None;
        for rule in &self.rules {
            let Some(specificity) = rule.route.specificity(path) else {
                continue;
            };
            if best.is_none_or(|(_, current)| specificity > current) {
                best = Some((rule, specificity));
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

    #[test]
    fn pattern_matches_exactly_one_non_empty_segment_per_param() {
        let route = RateLimitRoute::pattern("/api/tenants/{id}/invitations");
        for path in [
            "/api/tenants/42/invitations",
            "/api/tenants/tenant-1/invitations",
        ] {
            assert!(
                route.specificity(path).is_some(),
                "{path} should match the template"
            );
        }
        for path in [
            // Segment counts must agree.
            "/api/tenants/42",
            "/api/tenants/42/invitations/",
            "/api/tenants/42/invitations/7",
            // A `{param}` needs exactly one non-empty segment.
            "/api/tenants//invitations",
            "/api/tenants/a/b/invitations",
            // Literal segments compare whole, not as prefixes.
            "/api/tenant/42/invitations",
            "/api/tenants/42/invitation",
            "/api/tenants/42/invitations2",
            "/API/tenants/42/invitations",
            "/api/tenants/42/invitations?page=2",
        ] {
            assert!(
                route.specificity(path).is_none(),
                "{path} must not match the template"
            );
        }
        // The constructor is equivalent to the documented variant.
        assert_eq!(
            RateLimitRoute::pattern("/api/tenants/{id}/invitations"),
            RateLimitRoute::Pattern("/api/tenants/{id}/invitations".to_string())
        );
    }

    #[test]
    fn precedence_is_exact_then_pattern_then_longest_prefix_then_any() {
        let exact = rule(
            RateLimitRoute::exact("/api/tenants/42/invitations"),
            RateLimitSubject::User,
        );
        let pattern = rule(
            RateLimitRoute::pattern("/api/tenants/{id}/invitations"),
            RateLimitSubject::Tenant,
        );
        let long_prefix = rule(
            RateLimitRoute::prefix("/api/tenants"),
            RateLimitSubject::ApiKey,
        );
        let short_prefix = rule(RateLimitRoute::prefix("/api"), RateLimitSubject::Ip);
        let any = rule(RateLimitRoute::Any, RateLimitSubject::Ip);

        let policy = RateLimitPolicy::new()
            .with_rule(any)
            .with_rule(short_prefix)
            .with_rule(long_prefix)
            .with_rule(pattern)
            .with_rule(exact);

        assert_eq!(
            policy
                .resolve("/api/tenants/42/invitations")
                .expect("exact")
                .subject,
            RateLimitSubject::User,
            "exact wins over the pattern that also matches"
        );
        assert_eq!(
            policy
                .resolve("/api/tenants/43/invitations")
                .expect("pattern")
                .subject,
            RateLimitSubject::Tenant,
            "pattern wins over a prefix that also matches"
        );
        assert_eq!(
            policy
                .resolve("/api/tenants/43/members")
                .expect("prefix")
                .subject,
            RateLimitSubject::ApiKey,
            "longest prefix wins when no pattern matches"
        );
        assert_eq!(
            policy.resolve("/api/orders").expect("short prefix").subject,
            RateLimitSubject::Ip
        );
        assert_eq!(
            policy.resolve("/health").expect("any").route,
            RateLimitRoute::Any
        );
    }

    #[test]
    fn patterns_are_ordered_by_literal_content_then_declaration_order() {
        // Same segment count and both match; the concrete tenant segment wins.
        let specific = rule(
            RateLimitRoute::pattern("/api/tenants/{id}/invitations"),
            RateLimitSubject::Tenant,
        );
        let generic = rule(
            RateLimitRoute::pattern("/api/{area}/{id}/invitations"),
            RateLimitSubject::Ip,
        );
        let policy = RateLimitPolicy::new()
            .with_rule(generic)
            .with_rule(specific);
        assert_eq!(
            policy
                .resolve("/api/tenants/42/invitations")
                .expect("specific")
                .subject,
            RateLimitSubject::Tenant,
            "the pattern with more literal segments wins"
        );

        // Equal literal weight: the first declared pattern wins.
        let first = rule(
            RateLimitRoute::pattern("/api/{area}/{id}/invitations"),
            RateLimitSubject::User,
        );
        let second = rule(
            RateLimitRoute::pattern("/api/{scope}/{key}/invitations"),
            RateLimitSubject::ApiKey,
        );
        let policy = RateLimitPolicy::new().with_rule(first).with_rule(second);
        assert_eq!(
            policy.resolve("/api/x/y/invitations").expect("tie").subject,
            RateLimitSubject::User,
            "equal specificity keeps the first declared rule"
        );
    }

    #[test]
    fn malformed_patterns_never_match() {
        // Direct `Pattern` construction bypasses the constructor check; a rule
        // built that way must be inert (no accidental matches) rather than
        // matching something the router would reject.
        for template in [
            "/api/{id/invitations",
            "/api/id}/invitations",
            "/api/{}/invitations",
            "/api/{*rest}/invitations",
            "/api/img-{id}.png",
            "/api/{{literal}}",
        ] {
            let route = RateLimitRoute::Pattern(template.to_string());
            let policy = RateLimitPolicy::new().with_rule(rule(route, RateLimitSubject::Ip));
            assert!(
                policy.resolve(template).is_none(),
                "{template} must never match, not even its own text"
            );
            assert!(
                policy.resolve("/api/42/invitations").is_none(),
                "{template} must never match a concrete path"
            );
        }
    }

    #[test]
    #[should_panic(expected = "malformed rate-limit route pattern")]
    fn pattern_constructor_rejects_malformed_templates() {
        let _ = RateLimitRoute::pattern("/api/{id/invitations");
    }

    #[test]
    #[should_panic(expected = "malformed rate-limit route pattern")]
    fn pattern_constructor_rejects_router_syntax_outside_the_subset() {
        // Catch-alls and within-segment parameters are router syntax, not
        // rate-limit pattern syntax: fail at startup instead of drifting.
        let _ = RateLimitRoute::pattern("/api/{*rest}");
    }
}
