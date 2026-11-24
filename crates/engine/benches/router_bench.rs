//! Benchmark: the **production** engine router (`webx_engine::Router`).
//!
//! The host crate's `webx_host::router::Router` is a 1.5-compatibility embedder
//! that the serving path no longer uses, so measuring it (as the old
//! `crates/host/benches/router_bench.rs` did) said nothing about a real request.
//! This measures the match a real request performs: one `matchit` lookup per
//! method table plus parameter extraction.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use webx_core::error::Result;
use webx_core::routing::HttpMethod;
use webx_engine::router::{RouteTarget, Router};
use webx_engine::EngineContext;

/// A target that does nothing: the benchmark measures routing, not the handler.
struct NoopTarget;

#[async_trait::async_trait]
impl RouteTarget for NoopTarget {
    async fn handle(&self, _ctx: &mut EngineContext) -> Result<()> {
        Ok(())
    }
}

fn build_router() -> Router {
    let mut router = Router::new();
    let target: Arc<dyn RouteTarget> = Arc::new(NoopTarget);
    // A realistic table shape for a mid-size API: many static routes, a handful
    // of parameterized ones, one deep hierarchy, one catch-all.
    for index in 0..40 {
        let path = Box::leak(format!("/api/v1/resources/{index}").into_boxed_str());
        router.insert(HttpMethod::Get, path, Arc::clone(&target));
    }
    router.insert(HttpMethod::Get, "/api/v1/users/{id}", Arc::clone(&target));
    router.insert(
        HttpMethod::Get,
        "/api/v1/users/{id}/posts/{post_id}",
        Arc::clone(&target),
    );
    router.insert(
        HttpMethod::Post,
        "/api/v1/orgs/{org}/teams/{team}/members/{member}",
        Arc::clone(&target),
    );
    router.insert(
        HttpMethod::Get,
        "/api/v1/files/{*rest}",
        Arc::clone(&target),
    );
    router
}

fn bench_router(c: &mut Criterion) {
    let router = build_router();

    c.bench_function("engine_router/static", |b| {
        b.iter(|| {
            let matched = router.match_route("GET", black_box("/api/v1/resources/21"));
            black_box(matched).expect("static route");
        })
    });

    c.bench_function("engine_router/param_1", |b| {
        b.iter(|| {
            let matched = router.match_route("GET", black_box("/api/v1/users/42"));
            black_box(matched).expect("one parameter");
        })
    });

    c.bench_function("engine_router/param_3", |b| {
        b.iter(|| {
            let matched = router.match_route(
                "POST",
                black_box("/api/v1/orgs/acme/teams/core/members/alice"),
            );
            black_box(matched).expect("three parameters");
        })
    });

    c.bench_function("engine_router/catch_all", |b| {
        b.iter(|| {
            let matched =
                router.match_route("GET", black_box("/api/v1/files/reports/2026/q3/report.pdf"));
            black_box(matched).expect("catch-all route");
        })
    });

    c.bench_function("engine_router/miss_405", |b| {
        b.iter(|| {
            // Path exists for GET only: the router scans the other method tables
            // to answer 405 instead of 404.
            black_box(router.match_route("POST", black_box("/api/v1/users/42")));
        })
    });
}

criterion_group!(benches, bench_router);
criterion_main!(benches);
