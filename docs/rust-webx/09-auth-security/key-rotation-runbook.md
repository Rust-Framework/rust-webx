# 认证密钥轮换与应急 Runbook

> 。目标：**密钥轮换、吊销、全端下线可在 5 分钟内完成并验证**；每一步都有可观察信号。

## 1. JWT 签名密钥

### 1.1 签发侧轮换（本服务自签，`Jwt.Signing`，零停机）

适用：本服务用 `Jwt.Signing` 的私钥签发（RS256/ES256/PS256），下游从本服务的
`/.well-known/jwks.json` 取公钥。轮换 = 滚动部署 + 保留旧公钥，没有运行期热轮换。

1. **生成新 key 对**（新 `kid`，如 `2026-12-a`），私钥落盘并 `chmod 600`：

   ```bash
   openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out jwt-signing-2026-12.pem
   openssl pkey -in jwt-signing-2026-12.pem -pubout -out jwt-signing-2026-12.pub.pem
   chmod 600 jwt-signing-2026-12.pem
   ```

2. **旧 key 的公钥转入 retired 列表**，新 key 作 active。`Jwt.Algorithms` 必须覆盖 active 与每个
   retired key 的算法（下面新旧同为 RS256，一条即可；换算法族时两条都要列）：

   ```json
   {
     "Jwt": {
       "Algorithms": ["RS256"],
       "Signing": {
         "Algorithm": "RS256",
         "KeyId": "2026-12-a",
         "PrivateKeyFile": "/run/secrets/jwt-signing-2026-12.pem"
       },
       "RetiredSigningKeys": [
         {
           "KeyId": "2026-11-a",
           "Algorithm": "RS256",
           "PublicKeyFile": "/run/secrets/jwt-signing-2026-11.pub.pem"
         }
       ]
     }
   }
   ```

3. **启动期先验证**：`Host::build()` 会加载密钥集、检查私钥权限与 RSA ≥ 2048 / EC P-256，
   并用 active key 签一份短命 token 再验回。任一步失败即启动失败——这是预期的拦截，
   不要把报错当成"配置可忽略"。
4. **滚动部署**：逐实例替换。部署后 `/.well-known/jwks.json` 同时包含
   `2026-12-a` 与 `2026-11-a` 两把公钥，新旧 token 都能验过。
5. **验证**：新签发的 token 头是 `kid=2026-12-a`；旧 `kid` 的 token 继续通过直到 `exp`；
   观察 401 不上升。
6. **等待旧 token 退场**：等满"最大 access token 寿命 + 下游 JWKS 缓存时间（`Jwks.CacheSeconds`）
   + `ClockSkewSeconds`"。
7. **删除 retired 条目**并重新发布，只留 active。再观察一个 `Jwks.CacheSeconds` 周期，
   401 不上升即完成。

回滚：

- 配置层面：把 `Signing` 指回旧私钥（`kid` 保持不变）、把新 key 公钥放进 `RetiredSigningKeys`，
  重新滚动部署。旧 token 立刻恢复可验。
- 已删除的私钥无法恢复：轮换前保留旧私钥到下一个周期结束（或按合规要求销毁），
  只把**公钥**放进 `RetiredSigningKeys`。
- 不要靠"同时配两个 `Signing`"实现双签发——`Jwt.Signing` 只有一个 active key。

### 1.2 IdP 侧轮换（外部 JWKS，零停机）

适用：本服务只验签，签发在外部 IdP（`Jwt.Jwks` 单源，或 `Jwt.JwksSources` 多源）。

1. **IdP/签发侧**发布新 key（新 `kid`），JWKS 同时包含新旧 key。
2. 等待一个 `Jwt.Jwks.CacheSeconds` + 最大 access token TTL + `ClockSkewSeconds`。
3. 验证：用新 key 签发的 token 能通过；旧 token 仍能通过直到过期。
4. IdP 移除旧 key；等待缓存 TTL；观察 `401 invalid_token` 不上升。
5. 回滚：重新发布旧 key（`kid` 不变）即可，JWKS 缓存最迟 TTL 恢复。

多源（`Jwt.JwksSources`）时每个 issuer 独立轮换；注意同一 `kid` 被两个来源发布会让请求
**503 ambiguous key id**——轮换时给新 key 的 `kid` 加上 issuer/日期前缀，避免跨来源撞名。

### 1.3 HS256（对称）

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
- 负缓存 `CachedSessionStore::new` 的跨实例窗口 = TTL（建议 1–5s）。

## 4. 健康检查

把 JWKS 探针挂到 `/health/ready`：

```rust
use webx::HealthCheckSpec;

let resolver = Arc::new(JwksResolver::from_section(&options.jwt.jwks.clone().unwrap())?);
Host::builder()
    .add_health_check(HealthCheckSpec::async_check(JwksHealthCheck::new(resolver)))
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
- [ ] 签发侧：`Signing` 切到新 key 后 `/.well-known/jwks.json` 同时含新旧 `kid`；回滚后旧 token 立刻可验。
- [ ] 签发侧：retired 公钥删除后，旧 token 在 TTL 内自然失效；无 5xx。
- [ ] 移除旧 `kid` 后，旧 token 在 TTL+token TTL 内自然失效；无 5xx。
- [ ] `revoke_until` 后下一请求 401；负缓存窗口符合 TTL。
- [ ] `revoke_family` 后旧 refresh 401，重用触发整族撤销。
- [ ] `revoke_all_for_subject` 后该用户全部 access/refresh/session 失效。
- [ ] `/health/ready` 在 JWKS 不可达时 503，恢复后自动 pass。

下一节：[安全最佳实践](security-best-practices.md)
