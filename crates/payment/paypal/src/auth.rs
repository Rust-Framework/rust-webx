//! PayPal OAuth2 client-credentials token cache.
//!
//! PayPal access tokens live for the `expires_in` returned by
//! `POST /v1/oauth2/token` (hours for a standard app). The cache refreshes
//! [`TOKEN_EXPIRY_SKEW_SECONDS`] before the provider expiry and serializes
//! refreshes behind one mutex, so concurrent requests for the same account
//! share a single network round trip.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::Deserialize;
use tokio::sync::Mutex;
use webx_pay::PaymentError;

/// Seconds taken off the provider `expires_in` to avoid using a token that
/// expires in flight.
pub const TOKEN_EXPIRY_SKEW_SECONDS: i64 = 60;

/// Fallback token lifetime when PayPal omits `expires_in` (documented responses
/// always include it; a short fallback keeps a broken response fail-safe).
const DEFAULT_EXPIRES_IN_SECONDS: i64 = 300;

/// Time source for token expiry decisions.
///
/// Production uses [`SystemClock`]; tests inject a fixed/advancing clock so the
/// expiry logic is exercised without waiting or touching the network.
pub trait PayPalClock: Send + Sync {
    /// Current Unix time in seconds.
    fn now_secs(&self) -> i64;
}

/// Wall-clock implementation of [`PayPalClock`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl PayPalClock for SystemClock {
    fn now_secs(&self) -> i64 {
        unix_now_secs()
    }
}

/// Current Unix time in seconds.
pub fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// PayPal `POST /v1/oauth2/token` success body (the fields this adapter uses).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct TokenResponse {
    #[serde(default)]
    pub(crate) access_token: String,
    #[serde(default)]
    pub(crate) expires_in: Option<i64>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at: i64,
}

/// Per-account access-token cache.
///
/// Clones share the same token slot and clock (accounts are held behind `Arc`),
/// so every request for one merchant account reuses one credential.
#[derive(Clone)]
pub(crate) struct TokenCache {
    state: Arc<Mutex<Option<CachedToken>>>,
    clock: Arc<dyn PayPalClock>,
}

impl TokenCache {
    pub(crate) fn with_clock(clock: Arc<dyn PayPalClock>) -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            clock,
        }
    }

    /// Return a cached token, or await `fetch` once and cache its result.
    ///
    /// The lock is held across the refresh, so N concurrent callers produce one
    /// fetch; the waiters observe the fresh token when they acquire the lock. A
    /// failed fetch leaves the slot untouched, allowing the next call to retry.
    pub(crate) async fn token_or_refresh<Fut>(&self, fetch: Fut) -> Result<String, PaymentError>
    where
        Fut: std::future::Future<Output = Result<TokenResponse, PaymentError>>,
    {
        let mut state = self.state.lock().await;
        let now = self.clock.now_secs();
        if let Some(cached) = state.as_ref() {
            if cached.expires_at > now {
                return Ok(cached.access_token.clone());
            }
        }

        let response = fetch.await?;
        if response.access_token.trim().is_empty() {
            return Err(PaymentError::Serialization(
                "paypal token response has no access_token".to_string(),
            ));
        }
        let lifetime = response
            .expires_in
            .unwrap_or(DEFAULT_EXPIRES_IN_SECONDS)
            .max(0);
        let expires_at = now + lifetime.saturating_sub(TOKEN_EXPIRY_SKEW_SECONDS);
        *state = Some(CachedToken {
            access_token: response.access_token.clone(),
            expires_at,
        });
        Ok(response.access_token)
    }
}

/// Base64 `client_id:client_secret` HTTP Basic credential for the token request.
pub(crate) fn basic_auth_header(client_id: &str, client_secret: &str) -> String {
    format!(
        "Basic {}",
        BASE64.encode(format!("{client_id}:{client_secret}"))
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    struct FakeClock(AtomicI64);

    impl FakeClock {
        fn new(now: i64) -> Arc<Self> {
            Arc::new(Self(AtomicI64::new(now)))
        }

        fn set(&self, now: i64) {
            self.0.store(now, Ordering::SeqCst);
        }
    }

    impl PayPalClock for FakeClock {
        fn now_secs(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn token(access_token: &str, expires_in: Option<i64>) -> TokenResponse {
        TokenResponse {
            access_token: access_token.to_string(),
            expires_in,
        }
    }

    #[test]
    fn decodes_basic_credentials() {
        let header = basic_auth_header("test-client", "test-secret");
        let encoded = header.strip_prefix("Basic ").unwrap();
        let decoded = BASE64.decode(encoded).unwrap();
        assert_eq!(
            String::from_utf8(decoded).unwrap(),
            "test-client:test-secret"
        );
    }

    #[test]
    fn parses_token_response() {
        let value = serde_json::json!({
            "scope": "https://uri.paypal.com/services/payments",
            "access_token": "A21AA-test-token",
            "token_type": "Bearer",
            "app_id": "APP-1",
            "expires_in": 32400,
            "nonce": "2024-01-01T00:00:00Znonce"
        });
        let response: TokenResponse = serde_json::from_value(value).unwrap();
        assert_eq!(response.access_token, "A21AA-test-token");
        assert_eq!(response.expires_in, Some(32_400));
    }

    #[tokio::test]
    async fn reuses_a_fresh_token_without_fetching() {
        let clock = FakeClock::new(1_700_000_000);
        let cache = TokenCache::with_clock(clock.clone());
        let fetches = Arc::new(AtomicUsize::new(0));

        let first = {
            let fetches = Arc::clone(&fetches);
            cache
                .token_or_refresh(async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(token("token-1", Some(3_600)))
                })
                .await
                .unwrap()
        };
        assert_eq!(first, "token-1");

        // 59 seconds before expiry: still fresh.
        clock.set(1_700_000_000 + 3_600 - TOKEN_EXPIRY_SKEW_SECONDS - 1);
        let second = {
            let fetches = Arc::clone(&fetches);
            cache
                .token_or_refresh(async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(token("token-2", Some(3_600)))
                })
                .await
                .unwrap()
        };
        assert_eq!(second, "token-1");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refreshes_after_expiry_with_skew() {
        let clock = FakeClock::new(1_700_000_000);
        let cache = TokenCache::with_clock(clock.clone());
        let fetches = Arc::new(AtomicUsize::new(0));

        cache
            .token_or_refresh({
                let fetches = Arc::clone(&fetches);
                async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(token("token-1", Some(3_600)))
                }
            })
            .await
            .unwrap();

        // 3600 - 60 = 3540 seconds after the fetch the cached token is stale.
        clock.set(1_700_000_000 + 3_540);
        let refreshed = cache
            .token_or_refresh({
                let fetches = Arc::clone(&fetches);
                async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(token("token-2", Some(3_600)))
                }
            })
            .await
            .unwrap();
        assert_eq!(refreshed, "token-2");
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn short_lifetimes_expire_immediately_after_skew() {
        let clock = FakeClock::new(1_000);
        let cache = TokenCache::with_clock(clock.clone());
        let fetches = Arc::new(AtomicUsize::new(0));

        cache
            .token_or_refresh({
                let fetches = Arc::clone(&fetches);
                async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(token("token-1", Some(30)))
                }
            })
            .await
            .unwrap();
        clock.set(1_000);
        cache
            .token_or_refresh({
                let fetches = Arc::clone(&fetches);
                async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(token("token-2", Some(30)))
                }
            })
            .await
            .unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_callers_share_one_refresh() {
        let clock = FakeClock::new(1_700_000_000);
        let cache = Arc::new(TokenCache::with_clock(clock));
        let fetches = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let fetches = Arc::clone(&fetches);
            handles.push(tokio::spawn(async move {
                cache
                    .token_or_refresh(async move {
                        fetches.fetch_add(1, Ordering::SeqCst);
                        // Let the other tasks reach the mutex before this fetch
                        // completes; they must wait and reuse the result.
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        Ok(token("shared-token", Some(3_600)))
                    })
                    .await
                    .unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), "shared-token");
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_refresh_is_retried() {
        let clock = FakeClock::new(1_700_000_000);
        let cache = TokenCache::with_clock(clock);
        let fetches = Arc::new(AtomicUsize::new(0));

        let first = cache
            .token_or_refresh({
                let fetches = Arc::clone(&fetches);
                async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Err(PaymentError::provider(
                        webx_pay::ProviderId::PayPal,
                        Some("AUTHENTICATION_FAILURE".to_string()),
                        "invalid client",
                        Some(401),
                        None,
                        false,
                    ))
                }
            })
            .await;
        assert!(first.is_err());

        let second = cache
            .token_or_refresh({
                let fetches = Arc::clone(&fetches);
                async move {
                    fetches.fetch_add(1, Ordering::SeqCst);
                    Ok(token("token-after-retry", Some(3_600)))
                }
            })
            .await
            .unwrap();
        assert_eq!(second, "token-after-retry");
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn empty_token_is_a_serialization_error() {
        let cache = TokenCache::with_clock(FakeClock::new(1_700_000_000));
        let error = cache
            .token_or_refresh(async { Ok(token("   ", Some(3_600))) })
            .await
            .unwrap_err();
        assert!(matches!(error, PaymentError::Serialization(_)));
    }
}
