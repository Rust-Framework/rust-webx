# rust-webx-oauth2

OAuth 2.0 social-login client for rust-webx.

> This is **not** an OIDC client. For OIDC providers use `rust-webx-oidc`;
> GitHub and several domestic providers only expose OAuth 2.0 + a REST user
> profile.

## Providers

| Preset | Authorization / token / profile |
|---|---|
| `OAuth2Provider::github()` | GitHub OAuth App |
| `OAuth2Provider::facebook()` | Facebook Login / Graph |
| `OAuth2Provider::google()` | Google OAuth 2.0 / OIDC userinfo |
| `OAuth2Provider::microsoft(tenant)` | Microsoft identity platform |
| `OAuth2Provider::gitlab(base)` | GitLab.com or self-hosted |
| `OAuth2Provider::gitee()` | Gitee |
| `OAuth2Provider::dingtalk()` | DingTalk |
| `OAuth2Provider::feishu()` | Feishu/Lark |
| `OAuth2Provider::wechat()` | WeChat QR login |
| `OAuth2Provider::weibo()` | Weibo OAuth 2.0 |
| `OAuth2Provider::qq()` | QQ Connect |
| `OAuth2Provider::douyin()` | Douyin |
| `AlipayClient` | Alipay signed gateway |

Any other provider:

```rust
let provider = OAuth2Provider::custom(
    "custom",
    "https://id.example.com/oauth/authorize",
    "https://id.example.com/oauth/token",
    "https://id.example.com/oauth/userinfo",
)
.with_scopes(["profile", "email"])
.with_mapping("/sub", Some("/name"), Some("/email"), Some("/picture"));
```

## Host integration

`examples/github_host.rs` mounts two real endpoints on `rust-webx`:

```text
GET /auth/github/start     -> 302 to GitHub
GET /auth/github/callback  -> code/token/user + local JWT
```

```powershell
$env:GITHUB_CLIENT_ID = "..."
$env:GITHUB_CLIENT_SECRET = "..."
$env:GITHUB_REDIRECT_URI = "http://localhost:5000/auth/github/callback"
cargo run -p rust-webx-oauth2 --example github_host
```

The example keeps `state -> pkce_verifier` in an in-memory store; production
uses a session or `OneTimeSecret` and consumes it once.

## Security boundary

- The client generates `state` and PKCE S256 where supported.
- The authorization code is exchanged only at the token endpoint.
- Access tokens are redacted from `Debug`; secrets are never logged.
- The application owns account linking, `state` storage/consumption, role
  assignment and session creation. Social claims are untrusted until mapped to
  an application account.
- `allow_private_networks(true)` is for tests/self-hosted providers only; the
  default keeps the framework SSRF guard.

## Example

```rust
let provider = OAuth2Provider::github();
let config = OAuth2Config::new(client_id, Some(client_secret), redirect_uri);
let client = OAuth2Client::new(provider, config)?;

let auth = client.authorization_url("state")?;
// persist auth.state/auth.pkce_verifier, redirect to auth.url
let token = client.exchange_code(code, auth.pkce_verifier.as_deref()).await?;
let user = client.userinfo(&token).await?;
let identity = user.into_identity();
```
