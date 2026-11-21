# 认证体系威胁模型（2.5）

> 本文回答“认证架构为什么是安全的”，不是功能说明。每个威胁都给出**缓解措施 +
> 代码落点 + 验证证据**；未覆盖项明确列为残余风险。

## 1. 资产与信任边界

| 资产 | 说明 |
|---|---|
| 用户凭据 | 密码、TOTP 种子、恢复码、WebAuthn 凭据、客户端证书私钥（客户端持有） |
| 会话/令牌 | JWT access token、refresh token、Cookie session id、CSRF token |
| 签名密钥 | HS256 secret、JWKS 私钥（IdP 持有）、OIDC client secret |
| 身份/授权数据 | `IdentityUser`、roles/permissions、tenant |

信任边界：

```
浏览器/客户端 ──TLS──▶ rust-webx host (auth middleware) ──▶ Handler/DB
                         │
                         ├──JWKS/HTTPS──▶ IdP
                         └──TLS client cert──▶ mTLS
```

框架**不信任**任何客户端输入（header/body/query/cookie/token），只信任：
- 已验证的 token 签名与 claims；
- TLS 层验证过的客户端证书链；
- 服务端会话存储中的记录；
- 启动时解析的配置/密钥文件。

## 2. 威胁与缓解

| # | 威胁 | 缓解 | 代码/测试 |
|---|---|---|---|
| T1 | 算法混淆/`alg:none` | 算法白名单 + 按 token header 收窄；拒绝 none/HS·RS 混淆 | `auth_validation_test` |
| T2 | 空/伪造 `sub` | 空 `sub` 永远拒绝；认证失败不构造身份 | `auth_validation_test` |
| T3 | `iss`/`aud` 错配 | 强制配置匹配；错误 issuer/audience → 401 | `auth_validation_test` |
| T4 | 过期/未来 token | `exp`/`nbf`/`iat` + 有界 leeway | `auth_validation_test` |
| T5 | 超大 token DoS | `MaxTokenBytes` 在验签前拒绝 | `auth_validation_test` |
| T6 | JWKS 投毒/SSRF | HTTPS-only、SSRF 守卫、文档大小上限、`kid` 精确匹配 | `auth_jwks_test` |
| T7 | JWKS 后端故障 | 503 fail-closed，不降级匿名 | `auth_jwks_test` |
| T8 | refresh 重放 | 原子轮换 + 重用撤销整族；错误 secret 不撤销族 | `refresh_token_test`/`auth-ef` |
| T9 | refresh 过期误撤销 | 过期与撤销分离 | `refresh_token_test` |
| T10 | 会话固定/劫持 | 登录轮换 256-bit session id；`__Host-`、HttpOnly、Secure、SameSite | `cookie_auth_test` |
| T11 | CSRF | 双提交 token + 常量时间比较 + `Sec-Fetch-Site` | `csrf_test` |
| T12 | 登录爆破 | 账号/客户端双维度限流 + 退避；未知账号仍做 dummy hash | `login_hardening_test` |
| T13 | Argon2 CPU DoS | 有界并发（默认 min(cpus,4)）；饱和 100ms → 503 | `login_hardening_test` |
| T14 | 弱口令/超长口令 | NIST 策略（≥12，≤1024）；blocklist | `login_hardening_test` |
| T15 | TOTP 重放 | `matched_step` 单调；重复/更早 step → 401 | `mfa` |
| T16 | TOTP 种子泄漏 | Debug 脱敏；只存加密种子（应用） | `mfa` |
| T17 | 恢复码重放 | 只存 SHA-256；单次消费 | `mfa` |
| T18 | WebAuthn challenge 重放 | 一次性 256-bit challenge + 绑会话 | `webauthn_test` |
| T19 | WebAuthn 克隆/降级 | RP ID hash、origin、UP/UV、签名计数器、ES256 | `webauthn_test` |
| T20 | 客户端证书伪造 | rustls 链验证 + 叶 SHA-256 指纹；engine 删除客户端自带指纹头 | `tls.rs`、`mtls_test` |
| T21 | mTLS 半配置降级 | build 拒绝无 CA/无 HTTPS/Off+CA | `mtls_test` |
| T22 | DPoP proof 重放 | `jti` 重放存储 + TTL；`htu/htm/ath/iat` 校验 | `dpop_test` |
| T23 | DPoP key 替换 | `cnf.jkt` 与 proof `jwk` thumbprint 常量时间比较 | `dpop_test` |
| T24 | introspection oracle | 无效/过期/吊销统一 `active:false`；不返回原因 | `introspection_test` |
| T25 | introspection 客户端伪造 | HTTP Basic 常量时间比较；端点由应用保护 | `introspection_test` |
| T26 | 吊销存储故障 | 503 fail-closed；负缓存只缓存“已吊销” | `session_cache_test` |
| T27 | 密钥轮换窗口 | `kid` 多 key + JWKS TTL；runbook 演练 | `key-rotation-runbook.md` |
| T28 | 日志泄漏令牌/密钥 | Debug 脱敏、错误不回显 secret、日志只记 scheme/outcome | `auth_*`、OIDC/DPoP |
| T29 | 弱 Production 配置 | 弱 JWT secret / 通配 CORS / 无 CSRF 启动失败 | `production_guard_test` |
| T30 | 匿名请求分配放大 | 认证层 0 分配增量；无 header 直接短路 | `auth_budget_test` |

## 3. 残余风险

| 风险 | 状态 |
|---|---|
| 外部 IdP 厂商差异（`at_hash`、非标准 claims） | Duende/GitHub/Google/Microsoft 已验证；其他厂商需 client 注册后复测 |
| WebAuthn 真实浏览器/认证器差异 | Chromium CTAP2 虚拟认证器已验证；物理认证器未纳入门禁 |
| 整请求框架税 10.42 | 非认证层；见 `alloc-budget.md` |
| 共享主机侧信道 | 未做 CPU/缓存侧信道评估；Argon2 并发已限 |
| 密钥托管 | `SecretFile`（0600）由运维保证；无 HSM/KMS 集成 |
| 分布式 DPoP/吊销存储 | 接口已留；生产需 Redis/DB 适配器 |

## 4. 评审问题清单

- [ ] 新端点是否要求认证/授权，是否有匿名路径？
- [ ] 新 token/secret 是否进日志、错误、`Debug`？
- [ ] 新存储故障是否 fail-closed（503）？
- [ ] 是否复用了 `ITokenIssuer`/`IPasswordHasher`/`constant_time_eq`/`OneTimeSecret`？
- [ ] 是否补了对抗用例（过期/重放/错配/超大/畸形）？
- [ ] 是否更新本威胁模型与 `auth-conformance.md`？
