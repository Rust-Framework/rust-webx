# 弃用登记（Deprecations）

> 政策见 [compatibility.md](compatibility.md) §5。2.x 线**不新增 `#[deprecated]`**
> （下游常用 `-D warnings`）；本表是 3.0 起的强制登记处，并记录仍然存在的旧弃用，以及
> 维护者决定跳过窗口、直接移除的项。

## 登记规则

- 一项一行：`项 | 引入版本 | 计划移除 | 替代项 | 迁移指南 | 状态`；
- 状态：`planned`（已决定未标记）/ `deprecated`（已标记）/ `removed`；
- 窗口 ≥ 2 个 minor：N 标记 → 最早 N+2 移除；
- 移除前必须：迁移指南 + CHANGELOG + 专项验收测试全部就绪。

## 仍然存在的旧弃用

| 项 | 引入 | 计划移除 | 替代项 | 迁移指南 | 状态 |
|---|---|---|---|---|---|
| `authz::resource_auth_middleware` | 1.5.0 | 3.0 | `#[authorize]`（静态/动态）+ `IDynamicAuthorizer` | 见 rustdoc | `deprecated` |

## 2.x 线新增弃用

无。`init_jwt_secret` / `jwt_secret()` 是带文档说明的兼容 shim，不加 `#[deprecated]`；
替代项为 `Arc<dyn ITokenIssuer>`，迁移说明见 `auth-migration-2.5.md`。

## 已移除（无兼容窗口）

维护者决定**跳过弃用窗口**的项：登记在案，便于下游按版本号定位改动来源。

| 项 | 引入 | 移除 | 替代项 | 迁移指南 | 状态 |
|---|---|---|---|---|---|
| `HostBuilder::use_email_sender` / `use_sms_sender` | 2.7.0 | 2.9.1（维护者决定，无兼容别名） | `HostBuilder::use_email` / `use_sms`（端口类型已表明是 sender，与 `use_clock`/`use_meter`/`use_session_store` 同形） | CHANGELOG `[2.9.1]` → Breaking：调用点改一个标识符，注册方式与行为不变 | `removed` |

## 3.0 计划窗口

| 项 | 引入 | 计划移除 | 替代项 | 状态 |
|---|---|---|---|---|
| 配置 section 的 `#[non_exhaustive]`（字段字面量构造） | 3.0.0 | —（一次性破坏） | `Default` + 配置绑定 | `planned` |
| `Pipeline { stages }` 结构体字面量构造 | 3.0.0 | — | `Pipeline::new(stages)` | `planned` |
| `RateLimitSection.trust_proxy` / `trusted_proxies`（`RateLimit.TrustProxy` / `RateLimit.TrustedProxies`） | 2.8.0 legacy（不加 `#[deprecated]`） | 3.0 | `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`（转发头解析唯一入口） | `planned` |
| `DeclaredRules` / `AuthorizerSet::authorize_with_rules`（rustdoc 原承诺 2.0 删除，已顺延） | 1.2.0 | 3.0 | `authorize_with_declared_rules`（现已经是 host 唯一调用路径） | `planned` |
| `init_jwt_secret` / `jwt_secret()`（全局签名密钥兼容 shim，.add_authentication() 仍写入） | 2.5.0 | 3.0 | `Arc<dyn ITokenIssuer>` / `JwtSecret`（见 `auth-migration-2.5.md`） | `planned` |
| `webx_host::rate_limit::RateLimiter::with_trust_proxy`（兼容壳，no-op） | 2.8.0 legacy（不加 `#[deprecated]`） | 3.0 | 删除调用；信任配置归 `ClientInfo` 节 | `planned` |
