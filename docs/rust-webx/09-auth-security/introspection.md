# Token Introspection（RFC 7662）与 Revocation（RFC 7009）

> 。框架提供协议语义与端口，端点由应用用普通 Handler 暴露。

## 1. Introspection（RFC 7662）

```rust
use std::sync::Arc;
use webx::*;

let introspector = Arc::new(
    TokenIntrospector::new(jwt_auth)          // 与资源服务器同一 verifier
        .with_session_store(session_store),   // 吊销即 inactive
);

// Handler：先做客户端认证，再 introspect
let response = introspector.introspect(&request.token).await?;
// 200 application/json
```

- 有效 token：`active:true` + `sub/scope/client_id/username/token_type/exp/iat/nbf/aud/iss/jti`（缺失字段省略）。
- 无效/过期/**已吊销** token：`{"active":false}`，**不解释原因**，避免 oracle。
- 吊销存储故障：**503**（不把故障伪装成 inactive，fail-closed）。
- 客户端认证：`IntrospectionBasicAuth::new(client_id, secret)` + `authorize(authorization_header)`；比较常量时间，`Debug` 脱敏。
- 端点必须由 HTTPS + 客户端凭据保护；不要暴露给公网匿名调用。

## 2. Revocation（RFC 7009）

```rust
let revoker = TokenRevoker::new(jwt_auth, session_store)
    .with_refresh(refresh_service);   // 同时支持刷新令牌

revoker.revoke(&request.token).await?; // 总是 200（未知 token 也成功）
```

- JWT：校验后取 `jti` + `exp`，`ISessionStore::revoke_until(jti, exp)`。
- 刷新令牌：`refresh.revoke_token(token)` 先校验 secret，再撤销整个 family；未知/格式错误 → no-op。
- 未知、已过期、已吊销的 token 一律 **200**（不泄露状态）。
- 存储故障 → 503；不要让 503 变成静默成功。

## 3. 端点装配建议

| 端点 | 方法/Content-Type | 响应 |
|---|---|---|
| `/oauth/introspect` | `POST application/x-www-form-urlencoded`（`token`、`token_type_hint`） | 200 `IntrospectionResponse` |
| `/oauth/revoke` | `POST application/x-www-form-urlencoded`（`token`、`token_type_hint`） | 200 空体 |

- `token_type_hint` 仅作为提示，不改变校验结果。
- 请求体大小限制（默认框架上限）即可；token 本身不应记录到日志。
- 应用把 `TokenIntrospector`/`TokenRevoker` 注册为 singleton，Handler 以 `#[inject]` 注入。

## 4. 边界

- 只支持本框架签发的 JWT access token 与 `RefreshTokenService` 的不透明 refresh token；不支持数据库中的任意 opaque access token。
- 不实现 RFC 7662 的 `scope` 授权细分（端点级客户端认证即可）。
- 真实 IdP 的 introspection 转发属于网关/应用职责。

下一节：[密钥轮换与应急 Runbook](key-rotation-runbook.md)
