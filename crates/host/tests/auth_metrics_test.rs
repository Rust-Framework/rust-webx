//! 2.5 M1 — authentication metrics (`auth.requests` / `auth.duration`).

mod test_utils;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use webx_core::auth::{IAuthenticationScheme, IClaims};
use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::metrics::{IMeter, MemoryMeter};
use webx_core::middleware::IMiddleware;
use webx_core::mode::AppMode;
use webx_host::auth::{
    AuthMetrics, AuthOutcome, AuthenticationMiddleware, AUTH_DURATION_METRIC, AUTH_REQUESTS_METRIC,
};
use webx_host::server::Host;

#[derive(Clone, Copy)]
enum Behavior {
    Claims,
    Anonymous,
    Unauthorized,
    BackendError,
}

struct FakeScheme {
    behavior: Behavior,
    matches: bool,
}

struct FakeClaims;

impl IClaims for FakeClaims {
    fn subject(&self) -> &str {
        "user-1"
    }

    fn roles(&self) -> &[String] {
        &[]
    }

    fn permissions(&self) -> &[String] {
        &[]
    }

    fn claims(&self) -> &HashMap<String, String> {
        static EMPTY: OnceLock<HashMap<String, String>> = OnceLock::new();
        EMPTY.get_or_init(HashMap::new)
    }

    fn clone_box(&self) -> Box<dyn IClaims> {
        Box::new(Self)
    }
}

#[async_trait::async_trait]
impl IAuthenticationScheme for FakeScheme {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn matches(&self, _ctx: &dyn IHttpContext) -> bool {
        self.matches
    }

    async fn authenticate(&self, _ctx: &mut dyn IHttpContext) -> Result<Option<Box<dyn IClaims>>> {
        match self.behavior {
            Behavior::Claims => Ok(Some(Box::new(FakeClaims))),
            Behavior::Anonymous => Ok(None),
            Behavior::Unauthorized => Err(Error::Unauthorized("bad token".to_string())),
            Behavior::BackendError => Err(Error::Internal("store down".to_string())),
        }
    }
}

fn middleware(behavior: Behavior, matches: bool, meter: &MemoryMeter) -> AuthenticationMiddleware {
    AuthenticationMiddleware::new(vec![Arc::new(FakeScheme { behavior, matches })])
        .with_metrics(meter)
}

#[tokio::test]
async fn authenticated_scheme_records_ok_and_latency() {
    let meter = MemoryMeter::new();
    let mw = middleware(Behavior::Claims, true, &meter);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/api/me");
    let _ = mw.invoke(&mut ctx).await.expect("authenticate");

    assert_eq!(
        meter.counter_value(
            AUTH_REQUESTS_METRIC,
            &[("scheme", "fake"), ("outcome", "ok")],
        ),
        1
    );
    assert_eq!(
        meter
            .histogram_values(AUTH_DURATION_METRIC, &[("scheme", "fake")])
            .len(),
        1
    );
}

#[tokio::test]
async fn anonymous_scheme_records_anonymous() {
    let meter = MemoryMeter::new();
    let mw = middleware(Behavior::Anonymous, true, &meter);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/public");
    let _ = mw.invoke(&mut ctx).await.expect("authenticate");

    assert_eq!(
        meter.counter_value(
            AUTH_REQUESTS_METRIC,
            &[("scheme", "fake"), ("outcome", "anonymous")],
        ),
        1
    );
}

#[tokio::test]
async fn rejected_credentials_record_unauthorized() {
    let meter = MemoryMeter::new();
    let mw = middleware(Behavior::Unauthorized, true, &meter);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/protected");
    let error = mw.invoke(&mut ctx).await.expect_err("must reject");
    assert!(matches!(error, Error::Unauthorized(_)));
    assert_eq!(
        meter.counter_value(
            AUTH_REQUESTS_METRIC,
            &[("scheme", "fake"), ("outcome", "unauthorized")],
        ),
        1
    );
}

#[tokio::test]
async fn backend_failure_records_error() {
    let meter = MemoryMeter::new();
    let mw = middleware(Behavior::BackendError, true, &meter);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/protected");
    let _ = mw.invoke(&mut ctx).await.expect_err("must fail");
    assert_eq!(
        meter.counter_value(
            AUTH_REQUESTS_METRIC,
            &[("scheme", "fake"), ("outcome", "error")],
        ),
        1
    );
}

#[tokio::test]
async fn no_matching_scheme_records_none() {
    let meter = MemoryMeter::new();
    let mw = middleware(Behavior::Claims, false, &meter);
    let mut ctx = test_utils::TestHttpContext::new("GET", "/public");
    let _ = mw.invoke(&mut ctx).await.expect("anonymous");

    assert_eq!(
        meter.counter_value(
            AUTH_REQUESTS_METRIC,
            &[("scheme", "none"), ("outcome", "anonymous")],
        ),
        1
    );
    assert_eq!(
        meter.counter_value(
            AUTH_REQUESTS_METRIC,
            &[("scheme", "fake"), ("outcome", "ok")],
        ),
        0
    );
}

#[test]
fn outcome_labels_are_stable() {
    assert_eq!(AuthOutcome::Authenticated.as_str(), "ok");
    assert_eq!(AuthOutcome::Anonymous.as_str(), "anonymous");
    assert_eq!(AuthOutcome::Unauthorized.as_str(), "unauthorized");
    assert_eq!(AuthOutcome::Error.as_str(), "error");
}

#[test]
fn auth_metrics_can_be_constructed_directly() {
    let meter = MemoryMeter::new();
    let schemes: Vec<Arc<dyn IAuthenticationScheme>> = vec![Arc::new(FakeScheme {
        behavior: Behavior::Claims,
        matches: true,
    })];
    let metrics = AuthMetrics::new(&meter, &schemes);
    metrics.record(
        "fake",
        AuthOutcome::Authenticated,
        std::time::Duration::ZERO,
    );
    assert_eq!(
        meter.counter_value(
            AUTH_REQUESTS_METRIC,
            &[("scheme", "fake"), ("outcome", "ok")],
        ),
        1
    );
}

#[test]
fn use_meter_registers_the_meter_in_di() {
    let meter = Arc::new(MemoryMeter::new());
    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_meter(meter)
        .try_build()
        .expect("host builds");

    assert!(
        host.provider().get::<dyn IMeter>().is_ok(),
        "use_meter must register dyn IMeter"
    );
}
