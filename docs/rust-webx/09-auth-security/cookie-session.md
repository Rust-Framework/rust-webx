# Cookie 会话与 CSRF（BFF）

> 浏览器同源 / BFF 场景使用：`EventSource`/SSE 无法携带 `Authorization` 头，
> 而 Cookie 由浏览器自动附带。纯 API 场景继续用 JWT/API Key，二者可在同一
> Host 共存（Cookie scheme 在请求带 `Authorization` 时主动让位）。

## 1. 为什么是不透明会话 id

Cookie 里只放 **256 位随机 id**，不放 JWT：

- 服务端状态是唯一真相：登出、全端下线、权限变更**下一请求**生效；
- 没有客户端可读的 claims：XSS 拿不到令牌，也无法自行延长会话；
- 没有令牌体积与轮换窗口问题：会话删除即失效。

代价是每次请求要查一次 `ISessionRepository`（缓存/索引由实现负责）。

## 2. 端口

| 类型 | 作用 |
|---|---|
| `ISessionRepository` | `create/get/touch/delete/delete_all_for_subject` |
| `SessionRecord` | `subject/name/email/tenant_id/roles/permissions/issued_at/expires_at/last_seen` |
| `InMemorySessionRepository` | 测试与单进程部署；过期记录在 `get` 时惰性清理 |
| 参考适配器 `examples/adapters/auth/ef` 的 `EfSessionRepository` | 多实例持久化（`auth_sessions` 表）；`publish = false`，复制进业务仓使用；`purge_expired` 供后台清理 |

仓库错误**不会**被吞掉：存储不可用时请求失败（5xx），不会降级成匿名。

## 3. 接入 Host

```rust
use std::sync::Arc;
use webx::*;

let repo = Arc::new(InMemorySessionRepository::new());
let scheme = Arc::new(SessionCookieScheme::new(
    repo,
    CookieOptions::default(), // __Host-webx_session; HttpOnly; Secure; SameSite=Lax
)?);

Host::builder()
    .add_authentication_scheme(scheme)
    .use_csrf(CsrfConfig::double_submit()) // Cookie scheme 必需，否则启动失败
    .build();
```

`CookieOptions` 默认：`idle_timeout = 30m`、`absolute_timeout = 12h`、
`sliding = false`（开启后每请求一次 `touch` 写入）。`SameSite=None` 必须配
`Secure`；`__Host-` 前缀必须 `Secure + Path=/`，违反即启动失败。

## 4. 登录 / 登出

登录端点声明 `IRequest<Response>`，用 `ContentResult` 同时写 JSON 与两个
`Set-Cookie`（框架按多值头处理，不会互相覆盖）：

```rust
let session = scheme
    .issue_session(
        SessionRecord::new("", &user.id)
            .with_name(&user.name)
            .with_tenant(&user.tenant_id)
            .with_roles(user.roles.clone())
            .with_permissions(user.permissions.clone()),
    )
    .await?;
let csrf = csrf_config.issue_token();
let body = serde_json::to_vec(&LoginResponse { /* ... */ })?;

Ok(Box::new(
    ContentResult::bytes(body, "application/json")
        .header("set-cookie", session.set_cookie)
        .header("set-cookie", csrf_config.set_cookie(&csrf)),
))
```

登出：

```rust
scheme.revoke_session(&session_id).await?;
response.set_header("set-cookie", &scheme.options().clear_cookie());
response.append_header("set-cookie", &csrf_config.clear_cookie());
```

前端把 CSRF 值写进 `X-CSRF-Token` 头（`CsrfConfig::header_name`，默认
`x-csrf-token`）；POST/PUT/PATCH/DELETE 缺少或不等即 403。

## 5. 会话交换端点（浏览器 → WebSocket）

浏览器用 Cookie 升级 WebSocket，但登录通常仍走 JSON API（Bearer）。推荐加一个**会话交换端点**
把短期凭据换成 HttpOnly 会话 Cookie，而不是让浏览器自己拼接 Cookie：

```text
POST /api/session            Authorization: Bearer <access token>
  → 200 {"user": {...}}
  → Set-Cookie: __Host-webx_session=...; HttpOnly; Secure; SameSite=Lax
  → Set-Cookie: <csrf cookie>; ...（双提交，前端读值写入 X-CSRF-Token）
```

端点实现就是上一节的签发写法：校验 Bearer 身份 → `issue_session` → 用 `ContentResult` 一次写
JSON 与两个 `Set-Cookie`。之后：

- 浏览器用 Cookie 升级 WebSocket（`wss://` 同源）；升级是 GET，CSRF 不拦握手，跨站风险由
  `use_websocket_origins` 白名单兜住（见
  [WebSocket 会话](../05-request-pattern/websocket.md#浏览器接入cookie--csrf--origin)）；
- 登出删除会话后，开启 `WebSocketLimits::session_revalidation` 的节点会在复查周期内以
  close `1008`（`SessionRevoked`）关闭该浏览器的长连接；
- Bearer 客户端继续用 `Authorization`，不参与 Cookie/CSRF 路径（Cookie scheme 遇到
  `Authorization` 会主动让位）。

## 6. 安全属性

- `HttpOnly` 防 XSS 读取会话；`Secure` 防明文传输；`SameSite=Lax` 防跨站表单。
- 登录成功轮换 session id（防固定）；权限变更后删除旧会话。
- 绝对过期 + 空闲过期；`touch` 失败只告警，不把有效会话变成认证失败。
- CSRF 双提交 cookie + 请求头，常量时间比较；`Sec-Fetch-Site: cross-site` 直接拒绝。
- 删除会话即失效，无 JWT 的 `exp` 窗口。

## 7. 错误语义

| 情况 | 结果 |
|---|---|
| 无 Cookie | 匿名（受保护端点由 `#[fromclaim]`/`#[authorize]` 401） |
| 未知 / 过期 / 已删除会话 | 匿名 |
| 仓库查询失败 | 5xx（fail-closed，不匿名放行） |
| Cookie 请求缺 CSRF 头/Cookie 或不等 | 403 `problem+json` |
| `Sec-Fetch-Site: cross-site` | 403 |
| 带 `Authorization` 的请求 | Cookie scheme 不匹配，交给 JWT/API Key |

## 8. 多实例

`InMemorySessionRepository` 只适合单进程；多实例复制参考适配器 `examples/adapters/auth/ef`
（`publish = false`，不随框架发布）的 `EfSessionRepository`（`auth_sessions` 表，支持
`purge_expired`），改成自己的包名后注册。CSRF 双提交不依赖服务端状态，天然多实例可用。

## 9. 验收

`crates/host/tests/cookie_auth_test.rs`（8 例）与
`crates/host/tests/csrf_test.rs`（9 例）覆盖签发/命中/过期/注销/Bearer 让位、
CSRF 缺失/不等/跨站/安全方法豁免，以及"Cookie scheme 无 CSRF 启动失败"守卫。

下一节：[令牌签发、会话吊销与口令哈希](token-session-password.md)
