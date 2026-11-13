# 令牌签发、会话吊销与口令哈希

本章三个端口把「登录签发」「登出吊销」「口令哈希」从业务代码收回到框架：业务只注入 trait、配置参数，不再自写 `jsonwebtoken` 或 Argon2。

| 端口 | 作用 | 框架默认实现 | 注册方式 |
|------|------|--------------|----------|
| `ITokenIssuer` | 由 claims 签发 token | `JwtTokenIssuer`（HS256） | `add_authentication()` 自动注册（读 `Jwt.Secret` / `Jwt.ExpirationMinutes`） |
| `ISessionStore` | 按 `jti` 吊销/查询 token | `InMemorySessionStore`（可选） | `HostBuilder::use_session_store(...)` |
| `IPasswordHasher` | PHC 口令哈希/校验/升级 | `Argon2PasswordHasher`（Argon2id） | Host 启动自动注册；`use_password_hasher(...)` 替换/改参 |

## 1. 登录：签发 token

```json
{
  "Jwt": {
    "Secret": "your-256-bit-secret-change-in-production",
    "ExpirationMinutes": 60
  }
}
```

`add_authentication()` 会用 `Jwt.Secret` 注册 HS256 版 `JwtTokenIssuer`；Handler 注入端口即可：

```rust
use webx::{ITokenIssuer, TokenClaims};

let issued = token_issuer
    .issue(
        TokenClaims::new(&user.id)
            .with_roles(&user.roles)
            .with_permissions(&user.permissions)
            .with_claim("tenant_id", &user.tenant_id), // 自定义 claim 走 extra
    )
    .await?;

issued.token               // 返回给客户端
issued.jti                 // 存 session 表，供登出吊销
issued.expires_at_unix_secs // 注意单位：Unix 秒（Travel 侧毫秒字段要 ×1000）
```

- `JwtTokenIssuer` **只支持 HS256**（与本框架 `JwtAuth` 的 `Validation::default()` 对齐）。RSA/EC 需要自定义 `ITokenIssuer` 且同时替换验签 handler，不支持混用。
- 想改寿命而不改代码：`Jwt.ExpirationMinutes` 或 `APP__Jwt__ExpirationMinutes`；`0` 回退 60。

## 2. 登出：会话吊销

```rust
Host::builder()
    .add_authentication()
    .use_session_store(Arc::new(InMemorySessionStore::new()))
    .build()
```

认证中间件验签后，用 token 的 `jti` 查 `ISessionStore::is_revoked`；命中即返回框架统一 401（`application/problem+json` + `WWW-Authenticate: Bearer`，detail `token has been revoked`），端点不会执行。

```rust
// 登出：吊销单个会话
session_store.revoke(&issued.jti).await?;
```

持久化实现只需实现 trait（`revoke` / `is_revoked`，可选 `revoke_all_for_subject`）：

- 未注册 store 或 token 无 `jti`：不查吊销，按 `exp` 兜底（与 1.1 行为一致）。
- store 查询报错：请求失败（fail-closed），不会「无法证明未吊销却放行」。
- `revoke_all_for_subject` 默认返回 **501**（trait 默认实现，表示不支持该能力，不是服务器故障）；DB 型 store 有 subject 索引时覆盖它。
- **注册必须落到 `dyn ISessionStore`**：用 `use_session_store(...)`，或 `svc.try_add_singleton::<dyn ISessionStore>(...)`；注册具体类型（`svc.instance(Arc::new(...))`）不会启用吊销。Host 启动时 debug 日志会打印吊销开/关。

## 3. 口令哈希

```rust
use webx::IPasswordHasher;

let phc = hasher.hash(&password).await?;                 // 注册/改密
let ok = hasher.verify(&password, &stored_phc).await?;   // 登录；false = 密码错
if ok && hasher.needs_rehash(&stored_phc) {
    let upgraded = hasher.hash(&password).await?;        // 透明升级参数
}
```

- 返回 PHC 字符串（`$argon2id$v=19$m=…,t=…,p=…$salt$hash`），算法与参数随哈希存储，换参数无需改表。
- 默认参数 = `Argon2::default()`（19 MiB / 2 / 1）；手写 `Argon2::default()` 产生的历史哈希可直接验证。
- 未知算法 / 非法 PHC / 非法参数 → 500 `Error::Internal`（运维可见，**不是** `Ok(false)`，不会伪装成「密码错误」），不 panic。
- 调参：`Host::builder().use_password_hasher(Arc::new(Argon2PasswordHasher::with_params(Argon2Params::new(65536, 3, 1))))`。
- 不需要口令哈希的服务可关默认依赖：`rust-webx = { version = "1.2", default-features = false }`（trait 仍在，按需自行实现）。

## 4. 动态授权读取 `#[authorize]` 声明

声明式 `#[authorize]` 与 DB 授权不必是两套真相：实现 `IDynamicAuthorizer::authorize_with_rules` 即可拿到端点声明。

```rust
async fn authorize_with_rules(
    &self, claims: &dyn IClaims, route: &str, method: &str, declared: &DeclaredRules<'_>,
) -> Result<()> {
    // 每条 #[authorize] 是一个规则，所有规则 AND；
    // 单条规则内 roles/permissions 各自 any-of，空列表 = 该维度不限制。
    for permissions in declared.permissions() {
        if !permissions.is_empty() && !permissions.iter().any(|p| claims.permissions().iter().any(|c| c == p)) {
            // 交给业务侧资源/实例级判定，不再靠 route+method 反查
        }
    }
    Ok(())
}
```

- 只实现旧的 `authorize` 仍然可用（默认转调），但拿不到声明；新代码实现 `authorize_with_rules`。
- `DeclaredRules` 是路由元数据的零分配视图（`rules()` / `roles()` / `permissions()`），多规则保持 AND 结构，不拍平。

## 5. 统一入口速查（避免同一件事两个入口）

| 需求 | 唯一推荐入口 | 说明 |
|------|--------------|------|
| 启用会话吊销 | `HostBuilder::use_session_store` | 必须注册 `dyn ISessionStore`；`jwt_middleware_with_revocation` 仅自组 pipeline 的逃生用法 |
| 动态授权 | `IDynamicAuthorizer::authorize_with_rules` | `AuthorizerSet::authorize` 保留仅为 1.1 兼容 |
| 读 trace id | 中间件用 `IHttpContext::trace_id()`；Handler 用 `RequestContext::trace_id()` | 两者同值；Handler 不接触 `IHttpContext` |
| 后台服务类型 | `BackgroundService` / `PeriodicBackgroundService` / `CancellationToken` | `BackgroundServiceHost` 等适配器只在 `webx::background` 命名空间（逃生写法的 order 组合） |
| 签发 / 哈希 / 吊销存储 | `ITokenIssuer` / `IPasswordHasher` / `ISessionStore` | 业务只做「替换/配置」，不引 `jsonwebtoken`/`argon2` 直用 |

## 下一步

- [安全最佳实践](security-best-practices.md)：密钥管理、HTTPS、输入校验清单。
- [authorize 宏与声明式授权](authorize-macro.md)：`#[authorize]` 规则与动态授权后端。
