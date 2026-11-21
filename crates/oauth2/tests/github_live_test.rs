use std::time::Duration;

use webx_oauth2::{OAuth2Client, OAuth2Config, OAuth2Provider};

/// Manual live check with a fresh GitHub OAuth code:
///
/// ```powershell
/// $env:GITHUB_OAUTH_CODE = "..."
/// $env:GITHUB_CLIENT_ID = "..."
/// $env:GITHUB_CLIENT_SECRET = "..."
/// $env:GITHUB_REDIRECT_URI = "http://localhost:8765/auth/github/callback"
/// cargo test -p rust-webx-oauth2 --test github_live_test -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "requires a fresh GitHub OAuth code and app credentials"]
async fn github_exchange_and_userinfo() {
    let Ok(code) = std::env::var("GITHUB_OAUTH_CODE") else {
        eprintln!("GITHUB_OAUTH_CODE not set; skipping");
        return;
    };
    let client_id = std::env::var("GITHUB_CLIENT_ID").expect("GITHUB_CLIENT_ID");
    let secret = std::env::var("GITHUB_CLIENT_SECRET").expect("GITHUB_CLIENT_SECRET");
    let redirect = std::env::var("GITHUB_REDIRECT_URI")
        .unwrap_or_else(|_| "http://localhost:8765/auth/github/callback".to_string());
    let config = OAuth2Config::new(client_id, Some(secret), redirect).without_pkce();
    let client = OAuth2Client::new(OAuth2Provider::github(), config).expect("client");
    let token = client.exchange_code(&code, None).await.expect("exchange");
    assert_eq!(token.token_type, "bearer");
    let user = client.userinfo(&token).await.expect("userinfo");
    assert!(!user.subject.is_empty());
    eprintln!(
        "GitHub OAuth OK: subject={} name={} email={:?}",
        user.subject, user.name, user.email
    );
    let _ = Duration::from_secs(1);
}
