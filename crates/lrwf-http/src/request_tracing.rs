//! Structured request tracing middleware with trace_id injection.

use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::middleware::IMiddleware;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct RequestTracing {
    pub log_all: bool,
}

impl RequestTracing {
    pub fn new() -> Self {
        Self { log_all: true }
    }
    pub fn errors_only() -> Self {
        Self { log_all: false }
    }
}

impl Default for RequestTracing {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl IMiddleware for RequestTracing {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let tid = next_trace_id();
        ctx.response_mut().set_header("x-trace-id", &tid);
        Ok(())
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let status = ctx.response().status();
        let is_err = status >= 400;

        if self.log_all || is_err {
            let count = COUNTER.fetch_add(1, Ordering::Relaxed);
            let method = ctx.request().method().to_string();
            let path = ctx.request().path().to_string();
            let user = ctx.claims().map(|c| c.subject().to_string());

            if is_err {
                tracing::warn!(
                    count = count, method = %method, path = %path,
                    status = status, user = %user.as_deref().unwrap_or("anon"),
                    "request error"
                );
            } else {
                tracing::info!(
                    count = count, method = %method, path = %path,
                    status = status, user = %user.as_deref().unwrap_or("anon"),
                    "request ok"
                );
            }
        }
        Ok(())
    }
}

struct XorShift(std::sync::Mutex<u64>);
static XORSHIFT: LazyLock<XorShift> = LazyLock::new(|| {
    XorShift(Mutex::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1),
    ))
});

static TRACE_BUF: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::with_capacity(16)));

fn next_trace_id() -> String {
    let mut x = XORSHIFT.0.lock().unwrap();
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    let id = *x;
    let mut buf = TRACE_BUF.lock().unwrap();
    buf.clear();
    use std::fmt::Write;
    write!(&mut *buf, "{:016x}", id).unwrap();
    buf.clone()
}
