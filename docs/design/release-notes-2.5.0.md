# rust-webx 2.5.0 发布说明（认证）

> 状态：**GA（2.5.0）**。认证域 M1–M4 已完成并通过核心 GA 门禁；外部 IdP/真实认证器
> interop 与 ≥1h 浸泡列为 post-GA 部署验证（见 `docs/design/auth-conformance.md`）。

## 1. 亮点

| 领域 | 交付 |
|---|---|
| JWT | 算法白名单、iss/aud、空 sub、leeway、MaxTokenBytes、Anonymous/Reject |
| 浏览器 | Cookie 会话（`__Host-`/HttpOnly/Secure/SameSite）+ 双提交 CSRF |
| 登录 | NIST 口令策略、登录限流、dummy hash、Argon2id + 有界并发 |
| 吊销 | `revoke_until` TTL、负缓存、EF 会话/刷新令牌持久化 |
| 联合身份 | JWKS + RS256/ES256/PS256 + `kid` 轮换；OIDC discovery/PKCE/id_token |
| 多因子 | TOTP（RFC 6238）+ 恢复码 + `amr`/`acr` |
| 强认证 | WebAuthn/Passkey 验签、mTLS、DPoP（RFC 9449）、HTTP Basic |
| 协议 | RFC 7662 introspection、RFC 7009 revocation |
| 可观测 | `auth.requests`、`auth.duration`、refresh 指标、`JwksHealthCheck` |

## 2. 新 crate

- `rust-webx-auth-ef`：`EfRefreshTokenStore`、`EfSessionRepository`
- `rust-webx-oauth2`：OAuth2 社交登录（GitHub/Google/Microsoft/GitLab/Gitee/DingTalk/Feishu/WeChat/Weibo/QQ/Douyin/Alipay/Facebook）
- `rust-webx-oidc`：RP 侧 OIDC（含私有 CA HTTPS）
- `rust-webx-mfa`：TOTP/恢复码/assurance
- `rust-webx-webauthn`：ES256 注册/断言验签

## 3. 兼容性（N-2）

- 旧 `Jwt.Secret` + HS256 配置继续工作；
- 旧 token（无 jti）继续验证；
- `init_jwt_secret`/`jwt_secret()` 仍可用但**已 deprecated**，下一 major 删除；
- bcrypt 哈希需渐进迁移到 `IPasswordHasher`/Argon2id；
- 新能力全部 opt-in；详见 `auth-migration-2.5.md`。

## 4. 验证

- 全 workspace `fmt`/`clippy -D warnings`/`test` 绿；
- HS256 p50 2.3µs / p99 **3.9µs**（release，20k）；
- 认证层匿名 **0 分配增量**（auth == no-op middleware）；
- OIDC 私有 CA 真实 HTTPS e2e（含 at_hash/PKCE 10 例）、mTLS 握手 e2e、30min/1h soak 0 失败（Linux 1h/3600.15s，RSS 稳定 17.7MB）、chaos 3/3；
- 对抗用例：JWT 22、Cookie 8、CSRF 9、登录 10、JWKS 7、Refresh 8、TOTP 13、WebAuthn 5、DPoP 5、introspection 6、mTLS 4。
- 方法/数字详见 `auth-performance-2.5.md`。

## 5. 已知限制（正式版前）

- 外部 IdP 已完成 Duende 完整授权码+PKCE+id_token、`rust-webx-oauth2` GitHub OAuth2 完整授权码+token+/user 真实联网，以及 Google/Microsoft discovery+JWKS；其他厂商待各自 client 注册；WebAuthn 已用 Chromium 虚拟认证器完成真实 create/get 验证；`at_hash` 已实现并有负例；
- WebAuthn 仅 ES256 + `none` attestation，真实浏览器/认证器 e2e 未做；
- 整请求分配税 17.86 allocs/request（与认证无关，独立性能目标）；
- ≥1h soak 尚未在干净环境记录；
- DPoP 对象型 `cnf` 需自定义 issuer；mTLS 仅 WebPKI 链。

## 6. 升级

1. 阅读 `auth-migration-2.5.md`；
2. 保留旧 secret 一个回滚窗口；
3. 按需启用 Cookie/CSRF、刷新令牌、OIDC/MFA/WebAuthn/mTLS；
4. 运行 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
