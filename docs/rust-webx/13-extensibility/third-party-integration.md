# 第三方库集成

## 数据库（rust-ef 示例）

Docbit 使用 `rust-ef` + SQLite：

```rust
// host/src/startup/services/service_collection.rs
use std::sync::Arc;

use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_sqlite::DbContextOptionsBuilderExt as _; // 提供 .use_sqlite()
use webx::rust_dix::ServiceCollection;
use webx::*;

pub trait ServiceCollectionExt {
    fn add_app_db(self) -> Self;
}

impl ServiceCollectionExt for ServiceCollection {
    fn add_app_db(self) -> Self {
        // 路径来自 app_base()，不是相对路径
        let path = app_base().join("app.db");

        let mut options = DbContextOptionsBuilder::new();
        options.use_sqlite(&path.to_string_lossy());
        let options = Arc::new(options.build());
        options.create_provider().expect("provider initialization failed");

        // Scoped：每次请求生成一份 owned DbContext
        self.scoped(move |_| {
            Arc::new(DbContext::from_options(&options).expect("Failed to create DbContext"))
        })
    }
}
```

Handler 以 owned 字段注入上下文：

```rust
#[derive(Inject)]
pub struct CreateItemHandler {
    #[inject(owned)]
    ctx: DbContext,
}
```

生产升级走 rust-ef 的版本化迁移：`ctx.migrate("Migrations")` 应用迁移目录（历史表
`__ef_migrations_history`），`ensure_created` 只用于测试；启动期迁移放在 `IHostedService` 里，
失败即拒绝启动，schema 漂移用 `MigrationDiff::has_drift()` 接 `IHealthCheck`。
后台任务注入 rust-dix 内建的 `Arc<dyn ScopeFactory>` 并显式开 scope：

```rust
#[derive(Inject)]
struct DbInitService {
    #[inject]
    services: Arc<dyn ScopeFactory>,
}

async fn start(&self) -> Result<()> {
    let scope = self.services.create_scope();
    // `RdiError` / `EFError` 都实现了 `From<..> for webx::Error`，直接 `?`
    let mut ctx: DbContext = scope.get_owned()?;

    ctx.migrate("Migrations").await?;
    // 之后可执行 seed
    Ok(())
}
```

## 持久化错误映射（rust-ef 边界）

`EFError` → `webx::Error` 的脱敏映射由 **rust-ef** 自己实现
（`impl From<EFError> for rust_core::AppError`），不需要任何框架侧适配 crate：
`rust-webx-ef`（`webx_ef`）连同 `map_ef_error` / `EfResultExt` 已在 2.9.1 删除
（见 [upgrade-to-2.9.1.md](../16-migration/upgrade-to-2.9.1.md)），
Handler 里直接用 `?`：

```rust
// Handler
let blog = ctx
    .set::<Blog>()
    .query()
    .first_or_default()
    .await?; // NotFound→404、Validation→400、Conflict→409、Connection/Transaction→可重试 503、其余→隐藏 500
```

`Query`/`TypeConversion`（400）与 `UniqueViolation`（409）使用固定文案：SQL、列名、
constraint/table 不会回显；`Database` 的瞬态条件（`SQLSTATE` 08/40/57P0*、SQLite busy/locked、
MySQL 1205/1213）与 `Connection`/`Transaction` 一样是可重试 503；500 保留原始信息与 source 链
供操作者日志。完整契约见
[边界约定](../../design/compatibility.md#4-边界约定rust-ef-错误映射)。

## 密码哈希（IPasswordHasher）

框架启动时注册 Argon2 实现（包在带并发上限的 `BoundedPasswordHasher` 里），业务只注入端口：

```rust
#[derive(Inject)]
struct LoginHandler {
    #[inject]
    hasher: Arc<dyn IPasswordHasher>,
}

let stored = self.hasher.hash(&password).await?;
let valid = self.hasher.verify(&password, &stored).await?;
```

口令强度用 `PasswordPolicy::nist()`（长度 + 可选常见口令黑名单）在注册/改密端点校验；
登录用 `verify_password_or_dummy` 抹平“账号不存在”的时延。不要自己接 bcrypt/argon2 crate；
`HostBuilder::use_password_hasher` 可换实现或调 Argon2 成本。

## 令牌签发（ITokenIssuer）

```rust
#[derive(Inject)]
struct LoginHandler {
    #[inject]
    issuer: Arc<dyn ITokenIssuer>,
}

let claims = TokenClaims::new(&user.id)
    .with_roles(["member"])
    .with_claim("tenant_id", &tenant.id);
let issued = self.issuer.issue(claims).await?;
// issued.token / issued.jti（登出/下线用 ISessionStore::revoke）/ issued.expires_at_unix_secs
```

密钥来自 `Jwt.Secret`（或 `Jwt.SecretFile` / `JWT_SECRET`），不要手写 `jsonwebtoken` 签发；
WebSocket 长连接需要“令牌过期即断开”时启用 `WebSocketLimits::close_at_token_expiry()`。

## 出站 HTTP（HttpClient）

服务端调用第三方 HTTP 统一使用框架客户端（连接池、超时分层、W3C trace、流式 body），
经 `add_http_client` 注册后注入 `Arc<dyn IHttpClientFactory>`：

```rust
let client = self.clients.create("github")?;
let response = client.execute(OutboundRequest::get("/repos/owner/repo")).await?;
```

详见[出站 HTTP](outbound-http.md)；不要在业务里另起 reqwest/裸 hyper 客户端。

## 邮件 / 短信发送（IEmailSender / ISmsSender）

端口在 core；host 不注册默认发送器（避免静默吞信）。测试与本地开发用捕获实现：

```rust
let email = Arc::new(InMemoryEmailSender::new());
Host::builder().use_email(email.clone()).build();

#[derive(Inject)]
struct ForgotPasswordHandler {
    #[inject]
    email: Arc<dyn IEmailSender>,
}

self.email
    .send(&EmailMessage::new(to, "Reset your password", &link))
    .await?;
```

多租户/品牌发件人、`Reply-To` 与附加头都在消息上表达，而不是塞进适配器字段：

```rust
self.email
    .send(
        &EmailMessage::new(to, "Reset your password", &link)
            .with_from("support@tenant.example")       // 可选；缺省用适配器配置
            .with_reply_to("help@tenant.example")      // 可选
            .with_header("List-Unsubscribe", "<https://tenant.example/unsubscribe>"),
    )
    .await?;
```

`validate()` 会校验地址与头：空地址、无 `@`、空白，以及头名/头值里的 CRLF/控制字符都会被拒绝
（防头注入）；适配器必须在发送前调用它。

生产实现（SMTP/SES/Postmark、Twilio）把传输失败映射为可重试 503，且失败细节只进操作者日志；
不要在数据库事务或锁内发送。多实例要求“恰好一次”投递时，用 outbox + 后台任务
（见[后台服务](../06-di-lifecycle/hosted-services.md)），请求内只写库。
`use_sms` 与 `InMemorySmsSender` 同构，用于断言验证码短信。

## 一次性令牌与防枚举（OneTimeSecret）

密码重置/邮箱验证：令牌由应用生成（如 `uuid::Uuid::new_v4()`），存储只放摘要，
TTL + 原子单赢家消费用框架原语：

```rust
let slot = OneTimeSecret::new(Duration::from_secs(30 * 60));
slot.issue(sha256_hex(&minted))?;                  // 进程内；持久化场景把摘要写库
let valid = slot.consume(&sha256_hex(&candidate)); // 原子 check-and-consume，并发只有一个赢家
```

- 多实例/容器重启：把摘要 + 过期时间存数据库，用条件更新
  （`UPDATE ... WHERE used = 0 AND expires_at > now()`）得到同样的单赢家语义；
- 忘记密码端点对“账号存在/不存在”返回同一响应（固定 `202 Accepted` + 固定文案），
  只在存在时发送邮件；登录用 `verify_password_or_dummy`；
- 重置成功后用 `ISessionStore::revoke` 让该用户旧 `jti` 会话立刻失效。

## 集成原则

1. 基础设施在宿主组合根注册（`.add_http_client(...)`、`.use_email(...)`、
   `svc.singleton::<dyn Port>(...)`）；
2. 业务代码只依赖 core 端口（trait），不依赖具体 crate/SDK；
3. 初始化与升级走官方迁移 / 后台服务，不在请求路径里做全量 schema 操作；
4. domain/application 层不直接依赖第三方 crate；适配器放宿主或独立 crate；
5. 持久化侧的框架端口（幂等 store、outbox、支付 inbox、会话/用量账本）由框架给**端口 + 默认实现**，
   持久化适配是**下游的数据适配**：在业务 crate 实现端口，或复制 `examples/adapters/*` 的参考实现
   （全部 `publish = false`，不随框架发布）——见 [端口与适配器（下游接入）](integrations.md)。

## 小结

框架不限制技术选型，通过 DI 和分层与任何库集成。

下一节：[端口与适配器（下游接入）](integrations.md)
