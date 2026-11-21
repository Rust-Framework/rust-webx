# 认证快速上手（TTFPE ≤ 10 分钟）

> 目标：从空项目到“登录拿 JWT → 访问受保护端点”不超过 10 分钟。
> 生产加固见 [安全最佳实践](security-best-practices.md) 与 [密钥轮换 Runbook](key-rotation-runbook.md)。

## 1. 依赖（1 分钟）

```toml
[dependencies]
rust-webx = "2.6.0"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
```

Argon2 默认启用（`password-argon2` feature）；无需自己写哈希。

## 2. 最小 Host（2 分钟）

```rust
use std::sync::Arc;
use webx::*;

#[tokio::main]
async fn main() -> Result<()> {
    let host = Host::builder()
        .configure(|app| {
            app.useOptions(|options| {
                options.app.urls = vec!["http://127.0.0.1:5000".to_string()];
                // 生产从 APP__Jwt__Secret / Jwt.SecretFile 读取，不要提交仓库
                options.jwt.secret =
                    "dev-only-secret-change-me-at-least-32-bytes".to_string();
            });
        })
        .add_authentication() // 注册 JWT 校验 + 默认 ITokenIssuer + IPasswordHasher
        .build();
    host.run().await
}
```

## 3. 登录（3 分钟）

```rust
#[derive(WebxRequestMeta, serde::Deserialize)]
struct LoginRequest {
    #[frombody] email: String,
    #[frombody] password: String,
}

#[derive(serde::Serialize)]
struct LoginResponse { token: String }

#[derive(Inject)]
struct LoginHandler {
    // 只注入端口：不接触 jsonwebtoken/bcrypt/argon2
    #[inject] tokens: Arc<dyn ITokenIssuer>,
    #[inject] hasher: Arc<dyn IPasswordHasher>,
}

#[post("/auth/login")]
impl IRequest<LoginResponse> for LoginRequest {}

#[handler(inject)]
#[async_trait::async_trait]
impl IRequestHandler<LoginRequest, LoginResponse> for LoginHandler {
    async fn handle(&mut self, req: LoginRequest) -> ActionResult<LoginResponse> {
        // 替换为你的数据库查询；store_hash 来自 users.password_hash
        let store_hash = load_password_hash(&req.email).await?;
        if !self.hasher.verify(&req.password, &store_hash).await? {
            return Err(Error::Unauthorized("invalid credentials".into()));
        }
        let claims = TokenClaims::new(&req.email) // 生产用稳定 user id
            .with_roles(["user"]);
        let issued = self.tokens.issue(claims).await?;
        Ok(LoginResponse { token: issued.token })
    }
}
```

> `load_password_hash` 是占位函数；演示可用 `hash = hasher.hash("...")` 预生成。
> 不要回显“邮箱不存在/密码错误”的区别，统一 401。
>
> 完整可编译示例：`cargo run -p rust-webx --example auth_quickstart`（演示密码 `admin123`，仅用于本地）。

## 4. 受保护端点（2 分钟）

```rust
#[derive(WebxRequestMeta)]
struct MeRequest {
    // 绑定层自动解析 Bearer token；缺失/无效 → 框架 401
    #[fromclaim] user: IdentityUser,
}

#[derive(serde::Serialize)]
struct MeResponse { id: String, tenant: Option<String> }

#[get("/me")]
impl IRequest<MeResponse> for MeRequest {}

#[derive(Default)]
struct MeHandler;

#[handler]
#[async_trait::async_trait]
impl IRequestHandler<MeRequest, MeResponse> for MeHandler {
    async fn handle(&mut self, req: MeRequest) -> ActionResult<MeResponse> {
        Ok(MeResponse {
            id: req.user.id.clone(),
            tenant: Some(req.user.tenant_id.clone()),
        })
    }
}
```

## 5. 验证（1 分钟）

```bash
cargo run
curl -s -X POST localhost:5000/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"alice@example.com","password":"correct horse battery staple"}'

# 用返回的 token
curl -s localhost:5000/me -H "Authorization: Bearer $TOKEN"
```

预期：登录 200 返回 `{"token":"..."}`；`/me` 无 token → **401 + problem+json + WWW-Authenticate**；有效 token → 200。

## 6. 下一步

| 需求 | 页面 |
|---|---|
| 刷新令牌/吊销 | [JWT 与会话](token-session-password.md) |
| Cookie BFF + CSRF | [Cookie 会话](cookie-session.md) |
| OIDC 登录 | [OpenID Connect](oidc.md) |
| TOTP/恢复码 | [TOTP](mfa.md) |
| WebAuthn/Passkey | [WebAuthn](webauthn.md) |
| 密钥轮换/应急 | [Runbook](key-rotation-runbook.md) |
| 生产配置 | [安全最佳实践](security-best-practices.md) |

### 生产差异

- `APP_ENV=Production`：强制强 JWT secret、禁止通配 CORS；
- `Jwt.SecretFile`（0600）或 `APP__Jwt__Secret`；
- 登录加 `ILoginThrottle`；
- 受保护端点加 `#[authorize]`/`RequestUser` 做细粒度授权；
- 多实例：`EfSessionRepository` + 共享 `IRefreshTokenStore`。
