//! 2.5 M4 — authentication performance budgets and malformed-input fuzz.

mod test_utils;

use std::time::{Duration, Instant};

use jsonwebtoken::{DecodingKey, Validation};
use webx_core::auth::IAuthenticationHandler;
use webx_core::security::{ITokenIssuer, TokenClaims};
use webx_host::auth_jwt::{JwtAuth, JwtTokenIssuer};

const SECRET: &str = "budget-test-secret-at-least-32-bytes-long";

fn validator() -> JwtAuth {
    JwtAuth::new(
        DecodingKey::from_secret(SECRET.as_bytes()),
        Validation::default(),
    )
}

async fn token() -> String {
    JwtTokenIssuer::from_secret(SECRET.as_bytes(), Duration::from_secs(3600))
        .issue(TokenClaims::new("user-1").with_roles(["admin"]))
        .await
        .expect("issue")
        .token
}

#[tokio::test]
async fn anonymous_request_short_circuits_before_token_parsing() {
    let auth = validator();
    let mut ctx = test_utils::TestHttpContext::new("GET", "/resource");
    let started = Instant::now();
    let result = auth.authenticate(&mut ctx).await.expect("anonymous");
    let elapsed = started.elapsed();
    assert!(result.is_none());
    // The no-header path must be a direct return, not a decode attempt.
    assert!(
        elapsed < Duration::from_millis(1),
        "anonymous path took {elapsed:?}"
    );
}

#[tokio::test]
#[ignore = "performance budget: run with `cargo test --release -- --ignored`"]
async fn hs256_p99_is_within_five_microseconds() {
    let auth = validator();
    let token = token().await;
    // Warm up the verifier and JSON machinery.
    for _ in 0..64 {
        assert!(auth.validate_token(&token).await.expect("valid").is_some());
    }

    let iterations = 20_000;
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        assert!(auth.validate_token(&token).await.expect("valid").is_some());
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p99 = samples[samples.len() * 99 / 100];
    let p50 = samples[samples.len() / 2];
    println!("HS256 validate: p50={p50:?} p99={p99:?} over {iterations} iterations");
    assert!(
        p99 <= Duration::from_micros(5),
        "HS256 p99 {p99:?} exceeds the 5µs budget"
    );
}

#[tokio::test]
async fn malformed_tokens_never_panic_and_stay_bounded() {
    let auth = validator();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for length in [0usize, 1, 2, 7, 32, 127, 1024, 8192, 8193, 65_536] {
        for _ in 0..50 {
            let mut bytes = Vec::with_capacity(length);
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                bytes.push((state & 0xff) as u8);
            }
            let candidate = String::from_utf8_lossy(&bytes);
            let result = auth.validate_token(&candidate).await;
            // Invalid input is either rejected or (for the empty string) no
            // identity; it must never panic, hang, or authenticate.
            assert!(result.is_err() || result.expect("checked").is_none());
        }
    }
}
