//! Per-IP token-bucket rate-limiting middleware.
//!
//! Limits the number of requests per client IP using a classic
//! token-bucket algorithm.  Configure via:
//!
//! ```ignore
//! .register(|svc| svc.singleton(|_| {
//!     Arc::new(RateLimitMiddleware::new(10.0, 20))
//! }))
//! ```
//!
//! The middleware reads the client IP from `X-Forwarded-For` or
//! `X-Real-IP` headers.  Exceeded clients receive a 429 response.

use rust_webapp_core::error::Result;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Instant;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// TokenBucket â€?per-IP state
// ---------------------------------------------------------------------------

struct TokenBucket {
    /// Current number of available tokens.
    tokens: f64,
    /// When this bucket was last refilled.
    last_refill: Instant,
}

impl TokenBucket {
    fn new(burst_size: f64) -> Self {
        Self {
            tokens: burst_size,
            last_refill: Instant::now(),
        }
    }

    /// Refill the bucket based on the elapsed time and try to consume
    /// one token.  Returns `true` when the request is allowed.
    fn try_consume(&mut self, rate: f64, burst: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * rate).min(burst);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// RateLimiter â€?inner state shared across middleware instances
// ---------------------------------------------------------------------------

/// Inner rate-limiting state shared behind a [`Mutex`].
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, TokenBucket>>,
    rate: f64,  // tokens per second
    burst: f64, // max tokens
}

impl RateLimiter {
    /// Create a new limiter.
    ///
    /// * `requests_per_second` â€?sustained request rate (e.g. `10.0`).
    /// * `burst_size` â€?maximum burst before throttling kicks in.
    pub fn new(requests_per_second: f64, burst_size: u32) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            rate: requests_per_second,
            burst: burst_size as f64,
        }
    }

    /// Check whether `ip` is allowed.  Returns `true` when the request
    /// should proceed.
    async fn allow(&self, ip: IpAddr) -> bool {
        let mut buckets = self.buckets.lock().await;
        let bucket = buckets
            .entry(ip)
            .or_insert_with(|| TokenBucket::new(self.burst));
        bucket.try_consume(self.rate, self.burst)
    }
}

// ---------------------------------------------------------------------------
// RateLimitMiddleware
// ---------------------------------------------------------------------------

/// Built-in per-IP rate-limiting middleware.
///
/// Reads the client IP from the `X-Forwarded-For` or `X-Real-IP` header
/// and applies a token-bucket algorithm.  Exceeded clients receive a
/// `429 Too Many Requests` JSON response.
pub struct RateLimitMiddleware {
    limiter: RateLimiter,
}

impl RateLimitMiddleware {
    /// Create middleware with the given rate and burst parameters.
    ///
    /// ```ignore
    /// Arc::new(RateLimitMiddleware::new(10.0, 20))
    /// ```
    pub fn new(requests_per_second: f64, burst_size: u32) -> Self {
        Self {
            limiter: RateLimiter::new(requests_per_second, burst_size),
        }
    }
}

#[async_trait::async_trait]
impl IMiddleware for RateLimitMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let ip = extract_client_ip(ctx);

        if !self.limiter.allow(ip).await {
            ctx.response_mut().set_status(429);
            ctx.response_mut()
                .set_header("content-type", "application/json");
            let body = serde_json::json!({
                "error": "Too Many Requests",
                "status": 429
            });
            let _ = ctx
                .response_mut()
                .write_bytes(serde_json::to_vec(&body).unwrap_or_default())
                .await;
        }

        Ok(())
    }
}

/// Best-effort extraction of the client IP address.
fn extract_client_ip(ctx: &dyn IHttpContext) -> IpAddr {
    // Prefer X-Forwarded-For (take the first entry when chained)
    if let Some(fwd) = ctx.request().header("x-forwarded-for") {
        let first = fwd.split(',').next().unwrap_or("").trim();
        if let Ok(ip) = IpAddr::from_str(first) {
            return ip;
        }
    }

    // Fallback: X-Real-IP
    if let Some(real) = ctx.request().header("x-real-ip") {
        if let Ok(ip) = IpAddr::from_str(real.trim()) {
            return ip;
        }
    }

    // Last resort â€?this should never happen behind a proper reverse proxy.
    IpAddr::from_str("127.0.0.1").unwrap()
}
