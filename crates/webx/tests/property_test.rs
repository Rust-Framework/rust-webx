//! Property tests (fuzz corpus) for the parser/matcher/encoding surfaces that
//! accept untrusted input.
//!
//! These run in the normal PR test job (proptest shrinks failures
//! automatically); the nightly soak/chaos suites live in
//! `stability_*_test.rs`.

use std::sync::Arc;

use proptest::prelude::*;
use webx::*;

struct NoopTarget;

#[async_trait::async_trait]
impl webx_engine::RouteTarget for NoopTarget {
    async fn handle(&self, _ctx: &mut webx_engine::EngineContext) -> Result<()> {
        Ok(())
    }
}

fn fixture_router() -> webx_engine::Router {
    let mut router = webx_engine::Router::new();
    router.insert(HttpMethod::Get, "/health", Arc::new(NoopTarget));
    router.insert(HttpMethod::Get, "/api/users/{id}", Arc::new(NoopTarget));
    router.insert(HttpMethod::Get, "/assets/{*path}", Arc::new(NoopTarget));
    router
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        // This file lives under `tests/`, where proptest cannot infer a source
        // file for its regression side-car. Failing seeds are printed to the
        // log instead; CI logs keep them for reproduction.
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// Arbitrary request targets must never panic the matcher.
    #[test]
    fn router_never_panics_on_arbitrary_paths(
        path in ".*",
        method in ".*",
    ) {
        let router = fixture_router();
        let _ = router.match_route(&method, &path);
        let _ = router.path_exists(&path);
        let _ = router.allowed_methods(&path);
    }

    /// A static route matches exactly and never allocates params.
    #[test]
    fn static_route_is_exact(method in "(GET|POST|PUT|DELETE)") {
        let router = fixture_router();
        let hit = router.match_route(&method, "/health");
        if method == "GET" {
            let hit = hit.expect("GET /health must match");
            prop_assert_eq!(hit.pattern.as_ref(), "/health");
            prop_assert!(hit.params.is_empty());
        } else {
            // HTTP methods are case-sensitive and only GET is registered here.
            prop_assert!(hit.is_none());
        }
    }

    /// `{id}` captures exactly the segment that was requested.
    #[test]
    fn dynamic_segment_is_captured(id in "[A-Za-z0-9_-]{1,32}") {
        let router = fixture_router();
        let path = format!("/api/users/{id}");
        let hit = router.match_route("GET", &path).expect("dynamic route");
        prop_assert_eq!(hit.pattern.as_ref(), "/api/users/{id}");
        prop_assert_eq!(hit.params.len(), 1);
        prop_assert_eq!(hit.params[0].0, "id");
        prop_assert_eq!(&hit.params[0].1, &id);
    }

    /// Pagination offset never overflows, even for extreme inputs.
    #[test]
    fn pagination_offset_is_u64_and_bounded(
        page in any::<u32>(),
        page_size in any::<u32>(),
    ) {
        let paging = PagedRequest { page, page_size };
        prop_assert_eq!(
            paging.offset(),
            u64::from(page.saturating_sub(1)) * u64::from(page_size)
        );
        prop_assert_eq!(paging.limit(), page_size);
    }

    /// RFC 9457 members survive a serialize/parse round trip, including
    /// arbitrary titles/details.
    #[test]
    fn problem_details_round_trips(
        status in 400u16..600,
        title in ".{0,64}",
        detail in proptest::option::of(".{0,128}"),
    ) {
        let mut problem = ProblemDetails::new(status, title.clone());
        if let Some(detail) = detail.clone() {
            problem = problem.with_detail(detail);
        }
        problem.complete(Some("/path"), Some("trace-1"));

        let json = serde_json::to_vec(&problem).expect("serialize");
        let value: serde_json::Value = serde_json::from_slice(&json).expect("parse value");
        prop_assert_eq!(value["status"].as_u64(), Some(u64::from(status)));
        prop_assert!(value["code"].is_string());
        prop_assert!(value["retryable"].is_boolean());
        prop_assert_eq!(value["instance"].as_str(), Some("/path"));
        prop_assert_eq!(value["traceId"].as_str(), Some("trace-1"));

        let back: ProblemDetails = serde_json::from_slice(&json).expect("parse back");
        prop_assert_eq!(back.status, status);
        prop_assert_eq!(back.title, title);
        prop_assert_eq!(back.detail, detail);
    }
}
