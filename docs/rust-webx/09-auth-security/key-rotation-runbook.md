# 认证密钥轮换与应急 Runbook

> 2.5 M2-05。目标：**密钥轮换、吊销、全端下线可在 5 分钟内完成并验证**；每一步都有可观察信号。

## 1. JWT 签名密钥

### 1.1 非对称（推荐，零停机）

适用 RS256/ES256/PS256 + JWKS。

1. **IdP/签发侧**发布新 key（新 `kid`），JWKS 同时包含新旧 key。
2. 等待一个 `Jwt.Jwks.CacheSeconds` + 最大 access token TTL + `ClockSkewSeconds`。
3. 验证：用新 key 签发的 token 能通过；旧 token 仍能通过直到过期。
4. IdP 移除旧 key；等待缓存 TTL；观察 `401 invalid_token` 不上升。
5. 回滚：重新发布旧 key（`kid` 不变）即可，JWKS 缓存最迟 TTL 恢复。

### 1.2 HS256（对称）

当前 host 只接受一个 `Jwt.Secret`（无 “previous secret” 列表），因此**无法零停机轮换**：

1. 低峰期滚动发布新 `Jwt.Secret`/`JWT_SECRET`。
2. 旧 token 全部失效：客户端重新登录（或提前把 access token 寿命降到分钟级）。
3. 验证：新登录成功；旧 token 401；回滚 = 恢复旧 secret（旧 token 重新有效，注意风险）。

> 需要 HS256 零停机时，迁移到非对称 JWKS 或由网关做双密钥验证；不要把 secret 同时写进代码与配置。

## 2. OIDC client secret

1. 在 IdP 同时启用新旧 client secret（或新建 client 并保留旧 client 一个 TTL）。
2. 发布新 `ClientSecretPost/Basic` 配置；观察 token 端点 401/502 不上升。
3. 删除旧 secret。

## 3. 吊销与全端下线

| 目标 | 操作 | 生效 |
|---|---|---|
| 单个 access token | `ISessionStore::revoke_until(jti, exp)` | 下一请求 |
| 单条 refresh 链 | `RefreshTokenService::revoke_family(family_id)` | 下一刷新 |
| 用户全部会话 | `revoke_all_for_subject` + `ISessionRepository::delete_all_for_subject` | 下一请求 |
| Cookie 会话 | `SessionCookieScheme::revoke_session(id)` | 下一请求 |
| 紧急全站 | 轮换签名密钥（非对称移除旧 kid / HS256 换 secret） | 一个 TTL 内 |

- 吊销存储故障必须 503（fail-closed），**不会**静默放行。
- 负缓存 `use_session_store_cached` 的跨实例窗口 = TTL（建议 1–5s）。

## 4. 健康检查

把 JWKS 探针挂到 `/health/ready`：

```rust
let resolver = Arc::new(JwksResolver::from_section(&options.jwt.jwks.clone().unwrap())?);
Host::builder()
    .add_health_check_async(JwksHealthCheck::new(resolver))
    .build();
```

- 缓存仍新鲜 → `pass`（不发网络请求）；缓存过期 → 触发一次刷新，失败 → `fail` → `/health/ready` 503。
- 探针失败意味着**无法验证外部 token**，不是“降级为匿名”。

## 5. 观测信号

| 指标/日志 | 用途 |
|---|---|
| `auth.requests{scheme,outcome}` | 哪个 scheme 在 401/503 |
| `auth.duration{scheme}` | 认证延迟 |
| `auth.refresh.events{outcome}` | issued/rotated/reuse/expired/revoked |
| `authentication schemes enabled` 启动日志 | 确认 schemes/CSRF/session store |
| `JWKS unavailable` health detail | IdP/JWKS 不可达 |

## 6. 5 分钟演练清单

- [ ] 发布新 `kid` 后 1 分钟内，新 token 通过、旧 token 仍通过。
- [ ] 移除旧 `kid` 后，旧 token 在 TTL+token TTL 内自然失效；无 5xx。
- [ ] `revoke_until` 后下一请求 401；负缓存窗口符合 TTL。
- [ ] `revoke_family` 后旧 refresh 401，重用触发整族撤销。
- [ ] `revoke_all_for_subject` 后该用户全部 access/refresh/session 失效。
- [ ] `/health/ready` 在 JWKS 不可达时 503，恢复后自动 pass。

下一节：[安全最佳实践](security-best-practices.md)
