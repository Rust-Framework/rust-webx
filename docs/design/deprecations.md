# 弃用登记（Deprecations）

> 政策见 [compatibility.md](compatibility.md) §5。2.x 线**不新增 `#[deprecated]`**
> （下游常用 `-D warnings`）；本表是 3.0 起的强制登记处，并记录仍然存在的旧弃用。

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

## 3.0 计划窗口

| 项 | 引入 | 计划移除 | 替代项 | 状态 |
|---|---|---|---|---|
| 配置 section 的 `#[non_exhaustive]`（字段字面量构造） | 3.0.0 | —（一次性破坏） | `Default` + 配置绑定 | `planned` |
| `Pipeline { stages }` 结构体字面量构造 | 3.0.0 | — | `Pipeline::new(stages)` | `planned` |
| `authz::resource_auth_middleware` | 1.5.0 | 3.0 | `#[authorize]` + `IDynamicAuthorizer` | `planned` |
| `RateLimitSection.trust_proxy` / `trusted_proxies`（`RateLimit.TrustProxy` / `RateLimit.TrustedProxies`） | 2.8.0 legacy（不加 `#[deprecated]`） | 3.0 | `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`（转发头解析唯一入口） | `planned` |
| `webx_host::rate_limit::RateLimiter::with_trust_proxy`（兼容壳，no-op） | 2.8.0 legacy（不加 `#[deprecated]`） | 3.0 | 删除调用；信任配置归 `ClientInfo` 节 | `planned` |
| `IdempotentResponse` 结构体字面量构造（2.8.0 新增 `headers` 字段，见 CHANGELOG Breaking） | 2.8.0 | 3.0 | 迁移：补齐 `headers`；3.0 加 `#[non_exhaustive]` 收口 semver 例外（见 compatibility.md §3/§8） | `planned` |
