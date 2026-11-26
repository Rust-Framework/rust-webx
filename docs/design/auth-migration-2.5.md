# 认证体系 2.5 迁移指南（N-2 兼容）

> 目标：2.1 部署可**原地升级到 2.5**，旧配置与旧 token 继续工作；新能力按需启用。
> 回滚：2.5 认证改动没有删除旧配置/旧 API；回滚到 2.1 时只需移除新配置项与新表。

## 1. 兼容矩阵

| 2.1 用法 | 2.5 行为 | 动作 |
|---|---|---|
| `Jwt.Secret` + HS256 | 继续工作（默认算法不变） | 无 |
| `Jwt.Issuer`/`Audience` | 继续校验；新配置可省略 | 无 |
| `init_jwt_secret()` / `jwt_secret()` | 2.5 可用（文档标注 deprecated），**2.9.1 已删除**（无别名） | 迁移到 `ITokenIssuer`；密钥来自 `Jwt.SecretFile` <!-- docs:allow-stale-api --> |
| 手写 `jsonwebtoken` 签发 | 继续编译 | 改为注入 `Arc<dyn ITokenIssuer>` |
| bcrypt 密码哈希 | Argon2 hasher 不识别 bcrypt | 见 §3 |
| 默认匿名请求 | 不变 | 无 |
| 旧 token（无 `jti`） | 继续验证；不做吊销匹配 | 无 |

## 2. JWT / JWKS

新增配置全部有安全默认，旧部署无需改动：

```jsonc
"Jwt": {
  "Algorithms": ["HS256"],              // 可配；JWKS 用 RS256/ES256/PS256
  "ClockSkewSeconds": 30,
  "MaxTokenBytes": 1048576,
  "InvalidTokenBehavior": "Anonymous",  // 或 "Reject"
  "Issuer": "https://id.example.com",
  "Audience": "travel-api",
  "Jwks": { "Url": "https://id.example.com/jwks", "CacheSeconds": 300 }
}
```

- 迁移到非对称：`Algorithms` 改 RS256/ES256，配置 `Jwks.Url`，可移除 `Jwt.Secret`。
- 启动 fail-closed：空算法、空 issuer、非 HTTPS JWKS、弱 Production secret 都会拒绝启动。
- 旧 HS256 token 在切换算法后自然失效；按 token TTL 安排切换窗口。

## 3. 口令哈希（bcrypt → Argon2id）

`IPasswordHasher` 默认实现是 Argon2id，无法验证 bcrypt：

1. **推荐**：登录时先 bcrypt 验证，成功后用 `IPasswordHasher::hash` 重写该行（渐进迁移）。
2. 或一次性脚本：用户首次登录/重置密码时强制重设。
3. 或保留自定义 `IPasswordHasher` 兼容旧哈希（不推荐长期）。

示例登录逻辑：

```rust
let ok = if user.password_hash.starts_with("$2") {
    bcrypt::verify(&password, &user.password_hash).unwrap_or(false)
} else {
    hasher.verify(&password, &user.password_hash).await?
};
if ok && hasher.needs_rehash(&user.password_hash) {
    user.password_hash = hasher.hash(&password).await?;
    ctx.update(user);
    save_changes(&mut ctx).await?;
}
```

## 4. 按需启用的新能力

| 能力 | 启用方式 | 数据/配置 |
|---|---|---|
| Cookie 会话 + CSRF | `use_csrf()` + 注册 `ISessionRepository` | 新表（应用实现） |
| 刷新令牌 | `RefreshTokenService` + `IRefreshTokenStore` | `auth_refresh_tokens`（auth-ef） |
| OIDC | `rust-webx-oidc` + discovery 配置 | 无新表（会话由应用存） |
| TOTP/恢复码 | `rust-webx-mfa` | 应用存 secret/hash |
| WebAuthn | `rust-webx-webauthn` | 凭据表（应用） |
| DPoP | `DpopScheme` + 自定义 issuer 写 `cnf` | 重放存储（可共享 Redis） |
| mTLS | `Tls.ClientAuth=Required` + `MtlsScheme` | 证书注册表 |
| 自省/吊销 | `TokenIntrospector`/`TokenRevoker` | 无 |

## 5. 部署检查清单（TTFPE ≤10 分钟）

1. `APP_ENV=Production`，配置强 `Jwt.Secret` 或 `Jwt.SecretFile`（0600）。
2. `Algorithms` 与签发侧一致；JWKS issuer/audience 一致。
3. 旧 bcrypt 账号有迁移路径（§3）。
4. 若启用 Cookie：`use_csrf` 且 Cookie `Secure`/`SameSite` 正确。
5. 若启用 mTLS：`ClientCaPath` + `ClientAuth=Required` + 注册 resolver。
6. `/health/ready` 加入 `JwksHealthCheck`（若用 JWKS）。
7. 回滚包保留旧 `Jwt.Secret` 与旧配置；新表可保留（不破坏旧代码）。

## 6. 已知限制

- 全局签名密钥 shim（`jwt_secret()`）已在 2.9.1 删除：密钥只来自 `Jwt.Secret` / `Jwt.SecretFile`
  （解析为 `JwtSecret`），自定义 issuer 用 `register_with_options` 读 `options.jwt.secret`。
- DPoP 默认 issuer 只能写字符串 claim，`cnf` 对象需自定义 issuer。
- WebAuthn 仅 ES256 + `none` attestation；mTLS 仅 WebPKI 链。
- 真实 IdP/认证器 e2e 与浸泡基线见 `auth-conformance.md`。
