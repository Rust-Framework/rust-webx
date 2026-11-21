# OpenID Connect 登录（`rust-webx-oidc`）

> 。独立 crate：只做 **RP（relying party）**，不做 IdP、不做社交登录按钮目录。

## 1. 流程

```
浏览器 ──▶ 应用 /auth/login        ── 302 ──▶ IdP /authorize?response_type=code&PKCE...
IdP    ──▶ 应用 /auth/callback?code=...&state=...
应用   ──▶ IdP /token（code + code_verifier + client auth）
应用   ──▶ 校验 id_token（iss/aud/nonce/exp/alg/kid + JWKS）
应用   ──▶ 换发本框架 JWT（ITokenIssuer）或建立 Cookie 会话
```

## 2. 发现与客户端

```rust
use std::sync::Arc;
use std::time::Duration;
use webx_oidc::{ClientAuth, HttpOidcHttp, OidcClient};

let http = Arc::new(HttpOidcHttp::new("https://id.example.com", Duration::from_secs(3))?);
let oidc = OidcClient::discover(
    http,
    "https://id.example.com",
    "travel-web",
    ClientAuth::None, // 或 ClientSecretPost / ClientSecretBasic
).await?;
```

- `/.well-known/openid-configuration` 必须返回同一 `issuer`；`authorization_endpoint`/`token_endpoint`/`jwks_uri` 必须 HTTPS。
- 必须声明 `S256` 与 `response_types_supported` 含 `code`；否则启动失败。
- 只接受非对称 id_token 算法（`RS256`/`ES256`/`PS256`，默认 RS256+ES256）。

## 3. 登录起点

```rust
let request = oidc.authorization_url(
    AuthorizationRequest::new("https://app.example.com/auth/callback")
        .pkce_s256()
        .state(csrf_state)   // 建议由会话/一次性值生成
        .nonce(nonce),
)?;

// 保存到浏览器会话（Cookie/服务端 session）：
// request.state / request.nonce / request.pkce_verifier
redirect(request.url);
```

`AuthorizationUrl::pkce_verifier` 只应保存在服务端会话；`state` 用于回调 CSRF 校验。

## 4. 回调与 id_token 校验

```rust
// 先从会话取回 state/nonce/verifier，并校验回调 state 与之一致
let session = oidc
    .exchange_code(
        &code,
        "https://app.example.com/auth/callback",
        stored_verifier.as_deref(),
        Some(&stored_nonce),
    )
    .await?;

// 会话桥：换发本框架 JWT（推荐，复用吊销/授权）
let issued = token_issuer.issue(session.to_token_claims()).await?;
// 或：建立 ISessionRepository 的 Cookie 会话
```

校验项（任一失败 → 401）：

- `alg` 在 `OidcConfig::algorithms` 且被 provider 声明；
- `kid` 经 JWKS 解析（缓存/单飞/SSRF 守卫，来自 M2-01）；
- `iss` 精确等于 discovery issuer；`aud` 包含 client_id；
- `exp`/`nbf` 在 leeway 内；`iat` 不超前；
- `nonce` 与发起时一致；多 audience 时 `azp == client_id`；
- `sub` 非空；
- 若 id_token 带 `at_hash`，按签名算法哈希 `access_token` 的左半并常量时间比较；缺失 `access_token` 则拒绝。

`OidcSession` 自带 `subject/email/name/access_token/refresh_token/expires_at` 与归一化 claims；`Debug` 对令牌字段脱敏。

## 5. 配置

```rust
let config = OidcConfig {
    algorithms: vec![JwtAlgorithm::Rs256],
    leeway: Duration::from_secs(30),
    jwks_ttl: Duration::from_secs(300),
    max_keys: 32,
    scopes: vec!["openid".into(), "profile".into(), "email".into()],
};
let oidc = OidcClient::discover_with(http, issuer, client_id, auth, config).await?;
```

- `ClientAuth::None`：公共客户端，**必须**传 `code_verifier`（缺失直接 400/Validation），`authorization_url` 也必须启用 `.pkce_s256()`。
- `ClientSecretPost`：`client_secret` 放表单；`ClientSecretBasic`：`Authorization: Basic`。
- 私有 CA / 企业 IdP：`HttpOidcHttp::with_root_certificates(issuer, timeout, ca_pem)`（PEM 替换公共根，缺失/空文件 fail-fast）。
- 令牌端点 HTTP 非 2xx → **502**；发现/JWKS 拉取失败 → **503**（fail-closed）。

## 6. GitHub OAuth 不是 OIDC

GitHub 的 OAuth App 是 **OAuth 2.0**，不是 OpenID Connect：

- 没有标准 OIDC discovery/id_token/JWKS；
- 授权码换到的是 access token，身份需再调 `GET https://api.github.com/user`；
- 因此**不要**把 GitHub 配置成 `OidcClient` 的 issuer，`OidcDiscovery::fetch` 会失败。

需要 GitHub 登录时，应新增一个 OAuth2 provider 适配器（authorize → token → `/user`），
而不是放宽 OIDC 校验。GitHub 自 2025-07 支持授权码 + PKCE，但 token endpoint
仍要求 client secret（confidential client）。

## 7. 边界

- 不做 IdP、SAML、社交登录按钮、用户 CRUD；账户映射/角色分配由应用决定。
- 真实 IdP 互通：Duende 完整授权码+PKCE+id_token 已实测；Google/Microsoft 已完成 discovery+JWKS。
- 回调端点的 `state` 比对、一次性消费与 Cookie/会话存储由应用（或 `OneTimeSecret`）负责。

下一节：[JWT Bearer 认证](jwt-authentication.md)
