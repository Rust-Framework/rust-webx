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

### 自定义 `ITokenIssuer` 如何拿到最终密钥

`Jwt.SecretFile` 在**应用 service 注册之前**解析为 `Jwt.Secret`，所以自定义 issuer 不需要、也不应该自己再读密钥文件：

```rust
use std::time::Duration;
use webx::{ITokenIssuer, JwtTokenIssuer};

Host::builder()
    .add_authentication()
    .register_with_options(|svc, options| {
        // options.jwt.secret 已是 APP__Jwt__Secret / Jwt.Secret / SecretFile 的最终值
        let issuer = JwtTokenIssuer::from_secret(
            options.jwt.secret.as_bytes(),
            Duration::from_secs(3600),
        );
        svc.try_add_singleton::<dyn ITokenIssuer>(move |_| Arc::new(issuer))
    })
    .build()
```

若 issuer 由 DI 工厂惰性构造，改为解析 host 注册的 `JwtSecret` 单例（同样在用户 service 配置之前完成注册）：

```rust
.register(|svc| {
    svc.try_add_singleton::<dyn ITokenIssuer>(|resolver| {
        let secret = resolver.resolve_arc::<JwtSecret>()?;
        Arc::new(JwtTokenIssuer::from_secret(secret.as_str(), lifetime))
    })
})
```

只配置 `SecretFile` 时，这个顺序保证自定义 issuer 与同一 host 的 `JwtAuth` 使用同一把密钥。未配置任何密钥时**不会**注册 `JwtSecret`（解析失败即启动失败），不会用空密钥签发 token。既有 `register` 的签名与行为不变，老代码无需调整。

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

### 2.1 让吊销记录随 token 过期

```rust
// 知道 token 的 exp 时用 revoke_until，而不是永久 revoke
session_store
    .revoke_until(&issued.jti, issued.expires_at_unix_secs)
    .await?;
```

- `InMemorySessionStore` 按 `jti → exp` 存储；`is_revoked` 命中已过期记录时删除并返回 `false`；
  `purge_expired(now)` 清理再也不会被读取的记录。`revoke` 仍是无限期（兼容旧 store）。
- 自定义 store 覆盖 `revoke_until` 即可表达 TTL（DB 列 `expires_at` + 清理任务）。

### 2.2 吊销查询负缓存

```rust
Host::builder()
    .add_authentication()
    .use_session_store_cached(store, Duration::from_secs(5))
    .build();
```

- 只缓存「未吊销」；已吊销结果每次回源，撤销立即生效。
- 本实例 `revoke`/`revoke_until` 会立即失效对应缓存项；**其他实例**的撤销最迟 `ttl` 后可见（1–5s 推荐）。
- `ttl = 0` 关闭缓存；条目数有上限（默认 10 万），不会成为内存放大器。

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

## 4. 刷新令牌轮换

长会话不要把 access token 寿命拉到 24 小时；用短 access token + 可轮换的 refresh token：

```rust
use webx::{InMemoryRefreshTokenStore, RefreshTokenService};

let refresh = RefreshTokenService::new(
    Arc::new(InMemoryRefreshTokenStore::new()),
    Duration::from_secs(30 * 24 * 3600),   // 单个 refresh token
    Duration::from_secs(90 * 24 * 3600),   // 整条 family 的绝对上限
);

// 登录成功后
let issued = refresh
    .issue("user-1", Some("tenant-1".to_string()), Some("web".to_string()))
    .await?;
// 只把 issued.token 返回一次；服务端只存 SHA-256(secret)

// 刷新
let rotated = refresh.rotate(&presented).await?; // 旧 token 立即失效
```

如果业务把 refresh family 与 `identity_session` 行关联（多端会话管理），用 `rotate_outcome`
拿回被重放的 `family_id`，才能把**对应的 session**一起置为 revoked：

```rust
use webx::RotateOutcome;

match refresh.rotate_outcome(&presented).await? {
    RotateOutcome::Rotated(issued) => {
        // 返回 issued.token
    }
    RotateOutcome::Reused { family_id } => {
        // family 已由 rotate_outcome 撤销；按业务关联撤销配对 session，
        // 避免已泄露的 access token 活到自然过期。
        revoke_paired_session(&family_id).await?;
        // 统一 401
    }
    RotateOutcome::Expired | RotateOutcome::Revoked | RotateOutcome::Invalid => {
        // 统一 401，文案不要区分（防枚举）
    }
}
```

- 令牌格式 `token_id.secret`（不透明）；存储只保存 `SHA-256(secret)`，泄露账本不等于泄露会话。
- **单次使用 + 原子轮换**：并发刷新只有一个赢家；其他调用看到 `Used`。
- **重用检测**：已消费 token 再次出现 → 撤销整条 family（`revoke_family`），所有派生 token 失效；
  `rotate_outcome` 把被重放的 `family_id` 返回给调用方（`rotate` 只返回 401）。
- 错误 secret 只返回 401，**不**撤销 family（否则知道 token_id 就能把用户踢下线）。
- 过期或超过 family 绝对上限 → 401；**不**撤销 family（客户端可能还持有合法的 sibling token）。
- 登出用 `revoke_family`；全端下线用 `revoke_all_for_subject`。
- `InMemoryRefreshTokenStore` 只适合单进程；多实例用 `rust-webx-auth-ef` 的 `EfRefreshTokenStore`（`auth_refresh_tokens` 表、条件更新 + 事务实现同一原子语义）。

## 5. 登录防护：策略、防枚举、限流与并发闸门

登录端点除了校验口令，还承担四件安全职责。当前框架提供原语，业务不再各写一套：

### 5.1 口令策略（NIST 800-63B）

```rust
use webx::PasswordPolicy;

let policy = PasswordPolicy::nist()          // 至少 12 字符，最多 1024
    .with_blocklist(["password123!", "qwertyuiop"]);

policy.validate(&password)?;                 // 注册/改密：Validation 错误可回显
```

- 只做长度与黑名单，**不要求**大小写/数字/符号组合，也不要求定期改密。
- 登录时不要把策略错误回显给客户端，统一映射为「用户名或口令错误」。
- 上限同时受 `MAX_PASSWORD_BYTES = 4096` 保护，避免超长输入变成 Argon2 工作量。

### 5.2 防用户枚举

```rust
use webx::verify_password_or_dummy;

let ok = verify_password_or_dummy(&*hasher, &password, stored_hash.as_deref()).await?;
// 用户不存在（None）时框架执行一次等价成本的 hash 后返回 false
```

未知账号不再「秒回」：响应时间与真实校验同量级。不要自己写
`if user.is_none() { return 401 }` 的短路。

### 5.3 登录限流（按账号 + 客户端）

```rust
use webx::{ILoginThrottle, MemoryLoginThrottle, ThrottlePolicy};

let throttle = MemoryLoginThrottle::new(ThrottlePolicy::default()); // 5 次后指数退避

let decision = throttle.check(&account, &client_ip).await?;
if !decision.allowed {
    return Err(webx::Error::Status(
        429,
        format!("too many attempts; retry in {}s", decision.retry_after.as_secs()),
    ));
}
// 口令错误：throttle.record_failure(&account, &client_ip).await?;
// 登录成功：throttle.record_success(&account, &client_ip).await?;
```

- 账号键（`a:`）与客户端键（`i:`）独立计数：攻击者换账号仍会被同一 IP 限流。
- 指数退避，**不永久锁定**（防攻击者锁死受害者）；成功登录清空计数。
- 进程内实现有键上限（默认 10 万），满时淘汰最旧项，不会成为内存放大器；多实例用共享实现。

### 5.4 Argon2 并发闸门

Host 默认注册的 Argon2 hasher 已包在 `BoundedPasswordHasher` 里：
并发上限 `min(cpus, 4)`，排队 100ms，超时返回 **503**（可带 `Retry-After`），
而不是无限堆积阻塞任务。

```rust
// 自定义 hasher 也建议显式包一层
let bounded = BoundedPasswordHasher::new(
    Arc::new(Argon2PasswordHasher::with_params(params)),
    4,
    Duration::from_millis(100),
);
Host::builder().use_password_hasher(Arc::new(bounded)).build();
```

- `use_password_hasher` 是「替换」，不会自动再包一层；自备 hasher 时自行决定并发策略。
- 503 是过载信号，客户端应退避重试；不要把它映射成「密码错误」。

## 6. 动态授权读取 `#[authorize]` 声明

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

## 7. 统一入口速查（避免同一件事两个入口）

| 需求 | 唯一推荐入口 | 说明 |
|------|--------------|------|
| 启用会话吊销 | `HostBuilder::use_session_store` | 必须注册 `dyn ISessionStore`；`jwt_middleware_with_revocation` 仅自组 pipeline 的逃生用法 |
| 动态授权 | `IDynamicAuthorizer::authorize_with_rules` | `AuthorizerSet::authorize` 保留仅为 1.1 兼容 |
| 读 trace id | 中间件用 `IHttpContext::trace_id()`；Handler 用 `RequestContext::trace_id()` | 两者同值；Handler 不接触 `IHttpContext` |
| 后台服务类型 | `BackgroundService` / `PeriodicBackgroundService` / `CancellationToken` | `BackgroundServiceHost` 等适配器只在 `webx::background` 命名空间（逃生写法的 order 组合） |
| 签发 / 哈希 / 吊销存储 | `ITokenIssuer` / `IPasswordHasher` / `ISessionStore` | 业务只做「替换/配置」，不引 `jsonwebtoken`/`argon2` 直用 |
| 登录防护 | `PasswordPolicy` / `verify_password_or_dummy` / `ILoginThrottle` / `BoundedPasswordHasher` | 策略、防枚举、限流、并发闸门各一个入口 |

### 7.1 认证指标

```rust
Host::builder()
    .use_meter(Arc::new(MemoryMeter::new())) // 或接 Prometheus/OTel 适配器
    .build();
```

| 指标 | 标签 | 说明 |
|------|------|------|
| `auth.requests` | `scheme`, `outcome` | 每次认证决策计数 |
| `auth.duration` | `scheme` | 认证耗时直方图（毫秒） |

- `outcome`：`ok` / `anonymous` / `unauthorized` / `forbidden` / `rate_limited` / `error`。
- `scheme`：注册的 scheme 名；`none` 表示没有 scheme 命中（匿名继续）。
- 默认 `NoopMeter`（零开销）；`AuthMetrics` 在构建时预绑定 counter/histogram 句柄，热路径无标签分配。
- 启动日志打印生效的 schemes、CSRF、会话存储数量，便于确认装配。

## 下一步

- [安全最佳实践](security-best-practices.md)：密钥管理、HTTPS、输入校验清单。
- [authorize 宏与声明式授权](authorize-macro.md)：`#[authorize]` 规则与动态授权后端。
