//! N-2 upgrade smoke application for the rust-webx 2.9.2 release.
//!
//! This file is compiled twice in CI: against the published `rust-webx 2.9.0`
//! (the N-2 release for 2.9.2) and against the current workspace path. It may
//! only use APIs that exist in both releases; `HostBuilder::configure`'s
//! options setter and `Error`'s constructors changed in the documented 2.9.1
//! breaking release, so this file deliberately avoids them.

use webx::*;

#[derive(Default, WebxRequestMeta)]
struct PingRequest;

#[derive(serde::Serialize)]
struct PingView {
    ok: bool,
}

#[get("/ping")]
impl IRequest<PingView> for PingRequest {}

#[derive(Default)]
struct PingHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<PingRequest, PingView> for PingHandler {
    async fn handle(&mut self, _req: PingRequest) -> ActionResult<PingView> {
        Ok(PingView { ok: true })
    }
}

fn build_host() -> Host {
    Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .build()
}

fn main() {
    let _build = build_host;
    let _problem = ProblemDetails::new(400, "Bad Request");
    let paging = PagedRequest {
        page: 1,
        page_size: 20,
    };
    let _offset = paging.offset();
    let limiter = RateLimiter::new(10.0, 20, 1_000);
    let _retry_after = limiter.retry_after_secs();
}
