# 认证一致性（Conformance）

> 本文是 2.5.0 认证能力的**当前**验收矩阵：能力、证据、性能预算、已知边界与复现命令。
> 威胁与缓解见 `auth-threat-model.md`；升级步骤见 `auth-migration-2.5.md`。
> 不记录里程碑编号、评审轮次或历史缺陷。

## 1. 能力与证据

| 能力 | 证据（测试文件） |
|---|---|
| JWT 校验：`iss`/`aud`/算法白名单/leeway/大小上限/空 `sub`/Reject 模式 | `host/tests/auth_validation_test.rs` |
| Cookie 会话 + CSRF（`__Host-`、HttpOnly/Secure/SameSite、双提交、`Sec-Fetch-Site`） | `host/tests/cookie_auth_test.rs`、`csrf_test.rs`、`rust-webx-auth-ef` 会话测试 |
| 登录硬化：NIST 口令策略、限流/退避、dummy hash、Argon2id 有界并发 | `host/tests/login_hardening_test.rs` |
| 吊销：`revoke_until` TTL、负缓存、存储故障 fail-closed | `host/tests/session_cache_test.rs`、`crates/core/src/security/` |
| JWKS/非对称 JWT：RS256/ES256/PS256、`kid` 轮换、single-flight、SSRF 防护 | `host/tests/auth_jwks_test.rs` |
| Refresh 令牌：SHA-256 存储、原子轮换、重用检测整族撤销、EF 持久化 | `host/tests/refresh_token_test.rs`、`rust-webx-auth-ef` |
| OIDC：discovery、PKCE S256、id_token 校验、私有 CA HTTPS | `auth/oidc/tests/*` |
| OAuth2 社交登录：授权码 + PKCE、provider 预设、userinfo → `IdentityUser` | `auth/oauth2/tests/*` |
| MFA：RFC 6238 TOTP、恢复码、`amr`/`acr` | `auth/mfa` 单元测试 |
| WebAuthn：ES256 注册/断言、RP/origin/challenge/UV/counter | `auth/webauthn/tests/*` |
| mTLS：客户端证书链、叶指纹注入、scheme | `engine/tests/tls.rs`、`host/tests/mtls_test.rs` |
| DPoP：RFC 9449 proof、`cnf.jkt`、`htm/htu/iat/jti/ath`、重放 | `host/tests/dpop_test.rs` |
| introspection / revocation：RFC 7662 / 7009 | `host/tests/introspection_test.rs` |
| 按路径 scheme 策略 | `host/tests/auth_route_policy_test.rs` |
| 认证指标 | `host/tests/auth_metrics_test.rs` |

## 2. 性能预算

| 预算 | 结果 |
|---|---|
| 匿名请求不进入 token 解析 | 通过（无 header 直接短路） |
| HS256 p99 ≤5µs | **p50 2.3µs / p99 3.9µs**（release，20k） |
| 认证层增量分配 | **0**：开启认证 +2.00 与空中间件 +2.00 相同（差额为 `async_trait` 中间件 future） |
| 整请求框架税 | **10.42 allocs/request**（见 `alloc-budget.md`） |
| JWKS 缓存命中 / 吊销负缓存 | 0 后端 I/O |
| Argon2 并发闸门 | 上限生效，饱和 → 503 |

## 3. 对抗与失败语义

- JWT malformed（随机字节、超长、空 token）返回 `Err`/`None`，不 panic。
- WebAuthn malformed attestation/authData/signature 返回结构错误，不 panic。
- JWKS/吊销后端故障 → 503，不降级为匿名。
- 刷新令牌并发轮换单赢家；重用撤销整族；错误 secret/过期不撤销族。
- DPoP 重放、`htu/htm/ath/iat` 篡改、`cnf.jkt` 不匹配 → 401。

## 4. 已知边界

- 外部 IdP：Duende 完整授权码 + PKCE、GitHub OAuth2、Google/Microsoft discovery/JWKS 已联网验证；
  其他厂商需各自 client 注册后复测。
- WebAuthn：仅 ES256 + `none` attestation；浏览器级验证使用 Chromium CTAP2 虚拟认证器，
  物理认证器与 `packed`/`tpm`/RS256/EdDSA 未纳入门禁。
- DPoP 对象型 `cnf` 需自定义 issuer；mTLS 仅 WebPKI 证书链。
- 浸泡：Linux 1h（3600s、0 失败、RSS 稳定 ~17.7MB）已记录；CI nightly 默认 300s。

## 5. 门禁与复现

PR CI 运行快速负例；`stability.yml` 运行耗时项：

- `auth-budget`：HS256 p99 + malformed fuzz；
- `auth-interop`：OIDC 私有 CA、mTLS 握手/指纹注入；
- `auth-live` / `webauthn-browser`：仅手动触发的真实外部依赖验证。

```powershell
cargo test -p rust-webx-host --test auth_budget_test
cargo test --release -p rust-webx-host --test auth_budget_test hs256_p99 -- --ignored --nocapture
cargo test -p rust-webx-webauthn
cargo run --release -p alloc-probe -- compare 1000
```
