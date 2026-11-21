# API Key 认证（与 JWT 共存）

管理面通常用 JWT（人登录），数据面通常用 `sk-` API Key（SDK/服务调用）。框架用
**认证 scheme 链**把两者放在同一台 host 上：`HostBuilder::add_authentication_scheme` 注册的
scheme 先尝试，JWT 由 `add_authentication()` 追加在最后。

```rust
use std::sync::Arc;
use webx::*;

Host::builder()
    .add_authentication_scheme(Arc::new(ApiKeyScheme::new(validator)))
    .add_authentication() // JWT，排在 API Key 之后
    .build();
```

## 选择规则

| 请求 | 命中的 scheme | 结果 |
|------|---------------|------|
| `Authorization: Bearer sk-…` | `ApiKeyScheme` | 交给 validator；有效则写入 claims |
| `x-api-key: sk-…` | `ApiKeyScheme` | 同上 |
| `Authorization: Bearer a.b.c` | `JwtScheme` | 验签；失败按匿名继续 |
| 无凭证 / 其他形状 | 无 | 匿名继续 |

- 第一个 `matches` 命中的 scheme 处理请求，**不会**再尝试后面的 scheme。
- JWT 只在 compact JWT 形状（两个点）时命中，所以不会吞掉 `sk-`；即使顺序反过来，
  `ApiKeyScheme` 也优先识别自己的前缀。
- 无 scheme 命中 = 匿名请求；受保护端点仍由 `#[authorize]` / `#[fromclaim]` 写 401。

## 自定义 validator

scheme 不碰存储；校验逻辑放在 `IApiKeyValidator`：

```rust
use webx::*;

struct DbKeys { /* pool, cache, ... */ }

#[async_trait::async_trait]
impl IApiKeyValidator for DbKeys {
    async fn validate(&self, presented_key: &str) -> Result<Option<Box<dyn IClaims>>> {
        // 1. 解析 key_id / secret；2. 查缓存或 DB；3. 常量时间比较 HMAC；
        // 4. 检查状态/过期；5. 返回 claims 或 None。
        Ok(Some(Box::new(
            ApiKeyClaims::new("key-1")
                .with_tenant("tenant-1")
                .with_roles(["model:invoke"])
                .with_claim("key_prefix", "sk-abc"),
        )))
    }
}
```

`ApiKeyClaims::new(key_id)` 把 key id 作为 `sub`；`with_tenant` 写入 `tenant_id`，因此
`#[fromclaim] user: IdentityUser`、`require_tenant()`、`is_in_role()` 与 JWT 完全一致。
`None` 表示"未知/已撤销"，scheme 返回 `401 invalid API key`（不回显 key）。

## 头部与前缀

```rust
ApiKeyScheme::new(validator)
    .header(ApiKeyHeader::XApiKey) // 默认 Both
    .prefix("sk-")                  // 默认 "sk-"
```

默认同时接受 `Authorization: Bearer sk-…` 和 `x-api-key: sk-…`。只允许一种头时显式设置
`ApiKeyHeader::AuthorizationBearer` 或 `XApiKey`。

## 认证失败限流

公开数据面的 `sk-` 端点可以被暴力尝试；按 `客户端 IP + key 前缀` 做固定窗口限流：

```rust
ApiKeyScheme::new(validator)
    .with_failure_limit(20, Duration::from_secs(60)) // 每 IP/前缀 60s 内最多 20 次失败
```

- 超限后直接 `429`（带 `Retry-After: 60`），不再查 store/HMAC；成功认证会清空该桶。
- 桶键含 IP 与 key 前缀，因此一个被猜的 key 不会锁死其他 key。
- 只统计"显式出示但校验失败"的尝试；匿名请求不受影响。
- 默认关闭（`max_failures = 0`）；生产建议开启，并按入口 IP 维度配合反代真实 IP 配置。
- 这是滥用防护，不替代授权/配额；配额仍由 `IQuotaStore` 判定。

## 现成的存储端口与 validator

`rust-webx-ai` 提供 `IApiKeyStore` / `ApiKeyRecord` / `MemoryApiKeyStore`，
`rust-webx-ai-gateway` 提供 `StoreApiKeyValidator`：

```rust
use std::sync::Arc;
use webx_ai::{hash_secret, ApiKeyRecord, MemoryApiKeyStore};
use webx_ai_gateway::StoreApiKeyValidator;

let store = Arc::new(MemoryApiKeyStore::new());
store.insert(
    ApiKeyRecord::new(
        "key-1",
        "tenant-1",
        hash_secret(pepper, secret).to_vec(),
        "v1",
    )
    .with_roles(["chat"]),
);

let validator = StoreApiKeyValidator::new(store, "v1", pepper.to_vec());

Host::builder()
    .add_authentication_scheme(Arc::new(ApiKeyScheme::new(Arc::new(validator))))
    .build();
```

- 每次请求都会检查 `status` / `revoked_at` / `expires_at`；`store.revoke("key-1")` 后
  **下一请求**即 401（没有 token 缓存窗口）。
- `pepper_id` 存在记录上，`StoreApiKeyValidator::with_pepper` 可同时接受新旧 pepper，支持轮换。
- `MemoryApiKeyStore` 适合测试/小部署；生产用 EF 账本 + TTL 缓存（见下节）。

## EF 账本 + TTL 缓存

`rust-webx-ai-ef` 提供持久化 store，`rust-webx-ai` 提供缓存包装，validator 不需要改：

```rust
use std::sync::Arc;
use std::time::Duration;
use webx_ai::CachedApiKeyStore;
use webx_ai_ef::EfApiKeyStore;

// DbContextOptions 来自应用自己的 rust-ef 配置；建立 ai_api_keys 索引/表由官方迁移负责。
let store = Arc::new(EfApiKeyStore::from_options(Arc::clone(&db_options)));
let cached = Arc::new(CachedApiKeyStore::new(store.clone(), Duration::from_secs(30)));

let validator = StoreApiKeyValidator::new(cached.clone(), "v1", pepper.to_vec());
Host::builder()
    .add_authentication_scheme(Arc::new(ApiKeyScheme::new(Arc::new(validator))))
    .build();
```

- `EfApiKeyStore` 只存 `HMAC-SHA256(pepper, secret)` 的十六进制摘要，绝不存明文；表
  `ai_api_keys` 的 `key_id` 是主键，`tenant_id`/`updated_at_unix_secs` 建索引。
- `upsert` 新增或更新（角色/权限/状态/过期时间），`revoke` 写 `status=revoked` +
  `revoked_at_unix_secs`。
- `CachedApiKeyStore` 缓存**命中**结果 `ttl` 秒；未命中不缓存（新建 key 下一请求即可用）。
- 撤销窗口：本进程撤销时调用 `cached.invalidate(key_id)` 立即失效；其他实例撤销时最坏延迟一个
  `ttl`。生产通常取 5–30s，或由控制面广播失效。
- `ttl = 0` 等价于关闭缓存，每次请求都查库（不推荐做数据面热路径）。
- 热路径仍然是"按 key_id 一次索引查找 + 一次常量时间 HMAC 比较"，缓存只省掉 DB 往返。

## 密钥存储建议（应用侧）

如果不用现成 store，`rust-webx-ai` 仍提供解析与 HMAC 助手，存储由应用决定：

```rust
use webx_ai::api_key::{hash_secret, parse_presented, verify_secret, DEFAULT_PREFIX};

// 创建：只存 hash，不存 secret
let hash = hash_secret(pepper, secret); // [u8; 32] = HMAC-SHA256(pepper, secret)

// 校验：按 key_id 查记录，再常量时间比较
let parsed = parse_presented(presented, DEFAULT_PREFIX)
    .ok_or_else(|| webx::Error::Unauthorized("invalid API key".into()))?;
let record = store.find(parsed.key_id).await?; // 缓存 + DB
if !verify_secret(pepper, parsed.secret, &record.secret_hash) {
    return Ok(None); // scheme 会转成 401
}
```

- `parse_presented`：secret 是**最后一个 `-` 之后**的段，因此 `key_id` 可以是 UUID（含 `-`）；
  secret 必须是 base62/hex（不含 `-`），长度 ≥ 16。
- **不要存明文**：`secret_hash = HMAC-SHA256(server_pepper, secret)`；热路径不要用 Argon2。
- 用 `key_id` 建唯一索引；日志/指标只打前缀，不打完整 key。
- 校验用 `webx::security::constant_time_eq`（`verify_secret` 已内置），不要自己写字节比较。
- 撤销：写 DB + 使缓存版本失效；TTL 决定最坏生效时间（可配 0）。
- 精确的 key 格式、轮换与配额由存储层决定；scheme 只要求"前缀 + 交给 validator"。

## 其他 scheme：HTTP Basic（2.5）

`BasicScheme` 走同一 scheme 链，只做 base64 解码并委托给业务校验器：

```rust
use webx::*;

struct EnvValidator { user: String, password: String }

#[async_trait::async_trait]
impl IBasicCredentialValidator for EnvValidator {
    async fn validate(&self, user: &str, password: &str)
        -> Result<Option<Box<dyn IClaims>>>
    {
        let ok = constant_time_eq(user.as_bytes(), self.user.as_bytes())
            & constant_time_eq(password.as_bytes(), self.password.as_bytes());
        Ok(ok.then(|| Box::new(BasicClaims::new(user)) as Box<dyn IClaims>))
    }
}

Host::builder()
    .add_authentication_scheme(Arc::new(BasicScheme::new(Arc::new(EnvValidator { .. }))))
    .build();
```

- **只在 TLS 后使用**：Basic 的密码只是 base64，不是加密。
- 适合遗留集成、健康探针、服务间调用；不要用于浏览器登录（用 Cookie/JWT）。
- 校验器负责存储与恒定时间比较；`Unknown user` 应返回 `None`（401），后端故障返回 `Err`（5xx）。
- 用户名不能含 `:`，密码可以；空用户名、非法 base64、缺少 `:` 都是 401。

## 错误语义

| 情况 | 响应 |
|------|------|
| 无效/撤销的 key | `401 problem+json` + `WWW-Authenticate` |
| 缺少 key（受保护端点） | `401`（身份绑定/授权写出） |
| key 有效但无权限 | `403`（`#[authorize]`/`RequestUser` 判定） |
| 匿名端点 | 无凭证正常 200 |

实现见 `rust-webx-host` 的 `auth` 模块；测试覆盖 Bearer、`x-api-key`、无效 key、JWT 共存与
既有 JWT 匿名语义（`crates/webx/tests/api_key_auth_test.rs`）。EF store 与 TTL 缓存测试在
`crates/ai-ef/src/api_key.rs`、`crates/ai/src/api_key_store.rs`。
