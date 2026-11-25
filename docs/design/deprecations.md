# 弃用登记（Deprecations）

> 政策见 [compatibility.md](compatibility.md) §5。2.x 线**不新增 `#[deprecated]`**
> （下游常用 `-D warnings`）；本表是 2.9.1 起的强制登记处，并记录仍然存在的旧弃用，以及
> 维护者决定跳过窗口、直接移除的项。

## 登记规则

- 一项一行：`项 | 引入版本 | 计划移除 | 替代项 | 迁移指南 | 状态`；
- 状态：`planned`（已决定未标记）/ `deprecated`（已标记）/ `removed`；
- 窗口 ≥ 2 个 minor：N 标记 → 最早 N+2 移除；
- 移除前必须：迁移指南 + CHANGELOG + 专项验收测试全部就绪。

## 仍然存在的旧弃用

无。

## 2.x 线新增弃用

无。

## 已移除（无兼容窗口）

维护者决定**跳过弃用窗口**的项：登记在案，便于下游按版本号定位改动来源。

| 项 | 引入 | 移除 | 替代项 | 迁移指南 | 状态 |
|---|---|---|---|---|---|
| `HostBuilder::use_email_sender` / `use_sms_sender` | 2.7.0 | 2.9.1（维护者决定，无兼容别名） | `HostBuilder::use_email` / `use_sms`（端口类型已表明是 sender，与 `use_clock`/`use_meter`/`use_session_store` 同形） | CHANGELOG `[2.9.1]` → Breaking：调用点改一个标识符，注册方式与行为不变 | `removed` |
| `HostAppBuilder::use_options` | 1.5.0 | 2.9.1（维护者决定，无兼容别名） | `HostAppBuilder::options`；唯一 `AppOptions` 入口是 `HostBuilder::configure(|app| app.options(|o| …))` | [upgrade-to-2.9.1.md](../rust-webx/16-migration/upgrade-to-2.9.1.md) §4 | `removed` |
| `HostAppBuilder::useOptions`（驼峰别名，2.9.0 与 `use_options` 并存） | 2.9.0 | 2.9.1（维护者决定，无兼容别名） | 删除；只保留 `options`（Rust 命名一律 snake_case） | 同上 | `removed` |
| `rust-webx-ef`（lib 名 `webx_ef`）兼容壳 | 2.7.0 | 2.9.1（维护者决定，无兼容别名） | 持久化幂等：框架端口 `IIdempotencyStore` + 下游实现（参考适配器 `examples/adapters/idempotency/ef`，非发布）；持久层错误：rust-ef 的 `From<EFError>` + `?`，不再需要任何 crate | [upgrade-to-2.9.1.md](../rust-webx/16-migration/upgrade-to-2.9.1.md) §6.1 | `removed` |
| `rust-webx-stack`（feature 化元 crate，已删除） | 2.9.1 | 2.9.1（存在即删除：不保留 crate、不保留别名） | 直接依赖 `rust-webx`；持久化能力在业务 crate 实现端口，或复制 `examples/adapters/*` 参考实现；模型见 [端口与适配器归属](integrations.md) | [upgrade-to-2.9.1.md](../rust-webx/16-migration/upgrade-to-2.9.1.md) §6 | `removed` |
| 五个 `rust-webx-*-ef` 适配 crate 的发布（`integrations/` 迁移为 `examples/adapters/`） | 2.7.0 | 2.9.1（移出发布序列：`publish = false`，不再是可依赖的包） | 在业务 crate 实现端口，或复制 `examples/adapters/{ai,auth,idempotency,outbox,payment}/ef` 参考实现；outbox 的端口/派发器改由 `rust-webx-outbox` 发布 | 同上 §6 | `removed` |
| `SelectQueryBuilder::order_by(&str)` / `order_by_desc(&str)`（字符串排序） | 2.9.0（rust-ef 拼写） | 2.9.1（维护者决定，无兼容别名） | `order_by_raw(...)` / `order_by_desc_raw(...)`（显式原始表达式）；强类型优先 `order_by_field*` | [upgrade-to-2.9.1.md](../rust-webx/16-migration/upgrade-to-2.9.1.md) §3 | `removed` |
| `authz::resource_auth_middleware` + `ResourceAuthorization` / `IAuthorizationPolicy`（路由前看不到 `route_pattern`，已 fail-closed） | 1.5.0 | 2.9.1（维护者决定，无兼容别名） | `#[authorize]`（静态/动态）+ `IDynamicAuthorizer`；策略数据留在业务侧 | [authorize-macro.md](../rust-webx/09-auth-security/authorize-macro.md) §动态授权 | `removed` |
| `DeclaredRules` / `IDynamicAuthorizer::authorize_with_rules` / `AuthorizerSet::authorize_with_rules` | 1.2.0 | 2.9.1（维护者决定，无兼容别名） | `IDynamicAuthorizer::authorize_with_declared_rules(&'static [AuthorizeRule])`；只做路由级判断时覆写 `authorize` | [authorize-macro.md](../rust-webx/09-auth-security/authorize-macro.md) §动态授权：接口与参数 | `removed` |
| `init_jwt_secret` / `jwt_secret()`（全局签名密钥兼容 shim） | 2.5.0 | 2.9.1（维护者决定，无兼容别名） | `Arc<dyn ITokenIssuer>`（`JwtTokenIssuer`）；自定义 issuer 用 `register_with_options` 读 `options.jwt.secret` 或解析 `JwtSecret` | [token-session-password.md](../rust-webx/09-auth-security/token-session-password.md) §自定义 `ITokenIssuer` 如何拿到最终密钥 | `removed` |
| `RateLimitSection.trust_proxy` / `trusted_proxies`（`RateLimit.TrustProxy` / `RateLimit.TrustedProxies`） | 2.8.0 legacy | 2.9.1（维护者决定，无兼容别名） | `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`（转发头解析唯一入口） | [caching-rate-limit.md](../rust-webx/11-production/caching-rate-limit.md) §迁移：转发头信任改由 `ClientInfo` 控制 | `removed` |
| `webx_host::rate_limit::RateLimiter::with_trust_proxy`（no-op 兼容壳） | 2.8.0 legacy | 2.9.1（维护者决定，无兼容别名） | 删除调用；信任配置归 `ClientInfo` 节（限流器只读 `IHttpContext::client_info`） | 同上 | `removed` |

## 下一破坏性版本计划窗口

| 项 | 引入 | 计划移除 | 替代项 | 状态 |
|---|---|---|---|---|
| 配置 section 的 `#[non_exhaustive]`（字段字面量构造） | 2.9.0 | 2.9.1（一次性破坏：13 个 `*Section` 加 `#[non_exhaustive]`） | `Default` + 配置绑定 / `configure(|app| app.options(..))` | `removed` |
| `Pipeline { stages }` 结构体字面量构造 | 2.9.0 | 2.9.1（字段私有化） | `Pipeline::new(stages)` | `removed` |
