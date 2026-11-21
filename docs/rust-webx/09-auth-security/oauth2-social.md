# OAuth 2.0 社交登录（GitHub / Google / 国内平台）

> 适用：GitHub、Gitee、钉钉、飞书、微信等 **OAuth 2.0** 登录。
> 如果提供方支持 OpenID Connect（Google、Microsoft、Duende 等），优先用
> [OIDC 登录](oidc.md)，不要走本文的 userinfo 映射。

## 1. 为什么单独一个 crate

`rust-webx-oidc` 要求 discovery + `id_token` + JWKS。GitHub 等 OAuth 2.0
提供方：

- 没有标准 OIDC discovery/id_token/JWKS；
- 授权码换到的是 **access token**；
- 身份来自 REST 用户信息接口。

因此 `rust-webx-oauth2` 只做 **OAuth 2.0 授权码 + PKCE + userinfo → IdentityUser**，
不改变 OIDC 的校验语义。

## 2. 内置 provider

| `OAuth2Provider` | 用途 |
|---|---|
| `github()` | GitHub OAuth App |
| `facebook()` | Facebook Login / Graph v20 |
| `google()` | Google OAuth 2.0 / OIDC userinfo |
| `microsoft(tenant)` | Microsoft identity platform |
| `gitlab(base)` | GitLab.com / 自建 GitLab |
| `gitee()` | Gitee（码云） |
| `dingtalk()` | 钉钉 OAuth 2.0 |
| `feishu()` | 飞书/Lark OAuth 2.0 |
| `wechat()` | 微信开放平台扫码登录 |
| `weibo()` | 微博 OAuth 2.0 |
| `qq()` | QQ 互联 |
| `douyin()` | 抖音开放平台 |
| `AlipayClient` | 支付宝签名网关 |

配置驱动场景可用 `OAuth2Provider::preset("github")`（`microsoft` 使用 common，`gitlab` 使用 gitlab.com）。

其他提供方用 `OAuth2Provider::custom(...)` + builder 配置端点、scope、
token 字段名、JSON pointer、client auth 方式。

## 3. 最小用法

```rust
use webx_oauth2::{OAuth2Client, OAuth2Config, OAuth2Provider};

let provider = OAuth2Provider::github();
let config = OAuth2Config::new(
    client_id,
    Some(client_secret),
    "http://localhost:8765/auth/github/callback",
);
let client = OAuth2Client::new(provider, config)?;

// 1. 登录起点：把 state + pkce_verifier 存到会话/OneTimeSecret
let auth = client.authorization_url("state-from-session")?;
// redirect(auth.url)

// 2. 回调：先常量时间比对并消费 state，再换 token
let token = client.exchange_code(code, auth.pkce_verifier.as_deref()).await?;

// 3. 拉取并规范化身份
let user = client.userinfo(&token).await?;
let identity = user.into_identity(); // id/name/email/roles 空

// 4. 应用账户绑定 + 角色分配（不要直接信任 provider 的角色）
```

`OAuth2User::raw` 保留原始 JSON，供租户/组织等 provider-specific 字段使用。

## 4. GitHub 实测

本仓库使用真实 GitHub OAuth App 完成：

```text
authorize → callback code → client_secret token exchange
→ GET /api/user → IdentityUser{id=209404271,name=Start,email=...}
```

对应测试：

```powershell
$env:GITHUB_OAUTH_CODE = "..."
$env:GITHUB_CLIENT_ID = "..."
$env:GITHUB_CLIENT_SECRET = "..."
$env:GITHUB_REDIRECT_URI = "http://localhost:8765/auth/github/callback"
cargo test -p rust-webx-oauth2 --test github_live_test -- --ignored --nocapture
```

结果：

```text
GitHub OAuth OK: subject=209404271 name=Start email=Some("474309146@qq.com")
```

`email` 缺失时会走 GitHub `/user/emails` 的 primary 地址回退。

## 5. 安全边界

- `state`：应用生成、保存、一次性消费；本 crate 只返回 state，不替应用存储。
- PKCE：provider 支持时默认 S256；OAuth App 的 token endpoint 仍需要 client secret
  （confidential client）。
- Client secret 只在服务端；`Debug` 已脱敏，不写日志。
- 只接受 `https://` 端点；`allow_private_networks(true)` 仅用于本地测试/自建服务。
- 授权码只在 token endpoint 兑换，不接受前端传入 access token 作为登录凭据。
- `IdentityUser` 的 roles/permissions 由应用账户绑定策略决定，不从社会提供方继承。
- 回调必须校验 state，并在失败时拒绝；不要把 provider 返回的 `state` 回显给客户端。

## 6. 支付宝 / QQ / 抖音

### 支付宝（签名网关）

支付宝不是普通 OAuth2 token 端点；token 和用户信息都是 RSA2 签名网关请求。
使用专用客户端：

```rust
use webx_oauth2::{AlipayClient, AlipayConfig};

let client = AlipayClient::new(AlipayConfig::new(
    app_id,
    app_private_key_pem,       // 应用私钥
    alipay_public_key_pem,     // 支付宝公钥，用于验签
    "https://app.example.com/auth/alipay/callback",
))?;
let url = client.authorization_url(state)?;
let token = client.exchange_code(code).await?;
let user = client.userinfo(&token).await?;
```

- 响应签名失败 → 502，不会接受未验签数据；
- 沙箱：`AlipayConfig::new(...).sandbox()`；
- `AlipayToken` 的 access token 在 Debug 中脱敏。

### QQ 互联

```rust
let provider = OAuth2Provider::qq()
    .with_subject_endpoint("https://graph.qq.com/oauth2.0/me", "/openid");
```

token 后自动调用 `me` 取 `openid`，再调 `/user/get_user_info`；`oauth_consumer_key`
使用 client_id。

### 抖音开放平台

```rust
let provider = OAuth2Provider::douyin();
```

token 使用 JSON POST，响应字段在 `/data/*`；userinfo 以 JSON body 发送
`access_token + open_id`。

## 7. 直接挂到 Host

完整可运行示例：[crates/auth/oauth2/examples/github_host.rs](../../../crates/auth/oauth2/examples/github_host.rs)。

```powershell
$env:GITHUB_CLIENT_ID = "..."
$env:GITHUB_CLIENT_SECRET = "..."
$env:GITHUB_REDIRECT_URI = "http://localhost:5000/auth/github/callback"
cargo run -p rust-webx-oauth2 --example github_host
```

注册两个端点：

| 路径 | 行为 |
|---|---|
| `GET /auth/github/start` | 生成 state/PKCE，302 到 GitHub |
| `GET /auth/github/callback` | 校验 state、换 token、取用户、签发本地 JWT |

示例用内存 `PendingStore`；生产将 `state -> pkce_verifier` 存入会话或
`OneTimeSecret`，并在回调首次消费后删除。

## 8. 与 OIDC 的取舍

| 需求 | 选择 |
|---|---|
| Google/Microsoft/Duende 等有 id_token | `rust-webx-oidc` |
| GitHub/Gitee/钉钉/飞书/微信等只有 access token | `rust-webx-oauth2` |
| 需要统一 `IdentityUser` | 两者最终都映射到 `IdentityUser` |
| 需要 refresh token 轮换/重用检测 | OIDC/本地 refresh store；社交 provider 的 refresh 由各 provider 规则决定 |
