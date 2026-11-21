# 认证体系 Conformance / GA 门禁

> 2.5 M4 工作台账。每次改动认证能力时同步更新本页证据；未勾选项不得宣称 2.5.0 GA。
> 威胁与缓解对照见 `auth-threat-model.md`；性能方法与数字见 `auth-performance-2.5.md`；
> 架构师质量复核见 `auth-quality-audit-2.5.md`。

## 1. 里程碑证据矩阵

| 里程碑 | 能力 | 证据 |
|---|---|---|
| M1-01 | JWT iss/aud/sub/leeway/alg/size/Reject | `auth_validation_test.rs`（22 例） |
| M1-02 | Cookie 会话 + CSRF | `cookie_auth_test.rs`（8）、`csrf_test.rs`（9）、`auth-ef` `EfSessionRepository`（4） |
| M1-03 | 口令策略/登录限流/Argon2 闸门 | `login_hardening_test.rs`（10） |
| M1-04 | 吊销 TTL + 负缓存 | `session_revocation_test.rs`、`session_cache_test.rs` |
| M1-05 | 认证指标/审计 | `auth_metrics_test.rs`（8） |
| M1-06 | Basic | `basic_auth_test.rs`（5） |
| M2-01 | JWKS/RS256/ES256/kid 轮换 | `auth_jwks_test.rs`（9，含无 kid 单键/歧义拒绝） |
| M2-02 | Refresh 轮换/重用检测/auth-ef | `refresh_token_test.rs`（8）、`auth-ef`（4） |
| M2-03 | OIDC discovery/PKCE/id_token | `oidc_test.rs`（13，含 at_hash/PKCE/配置边界）、`oidc_live_test.rs`（Duende 真实授权码+PKCE+id_token、Google/Microsoft discovery/JWKS、token 负例，4 例）、`oidc_https_e2e_test.rs`（真实 HTTPS + 私有 CA + discovery/JWKS/token） |
| M2-04 | TOTP/恢复码/amr/acr | `mfa`（13） |
| M2-05 | JWKS readiness/refresh 指标/runbook | `auth_jwks_test`、`refresh_token_test`、runbook |
| M3-01 | WebAuthn 注册/断言 | `webauthn_test.rs`（6，含 malformed fuzz）+ `browser_e2e_test.rs`（Chromium 虚拟认证器真实 create/get） |
| M3-02 | mTLS | engine TLS（2，含实际握手 e2e）、`mtls_test.rs`（4，含启动 fail-closed） |
| M3-03 | DPoP | `dpop_test.rs`（5） |
| M3-04 | per-route scheme（按需） | `use_auth_scheme_for` + `auth_route_policy_test.rs`（5，含路径段边界） |
| M3-05 | `jwt_secret` shim 迁移 | docbit e2e（3） |
| M4-01/02 | RFC 7662/7009 | `introspection_test.rs`（6） |
| M4 | 性能预算 | `auth_budget_test.rs`（release） |
| M4 | Malformed fuzz | `auth_budget_test.rs`、`webauthn_test.rs` |
| M4 | Chaos/浸泡 harness | Chaos **3/3 通过**；Soak 600s + **30min（1802s，0 失败）** + **1h（3600.15s，0 失败，Linux RSS 稳定 17.7MB）** 通过 |

## 2. 性能预算（实测）

| 预算 | 结果 | 证据 |
|---|---|---|
| 匿名请求不进入 token 解析 | 通过（无 header 直接返回 None，<1ms） | `auth_budget_test::anonymous_request_short_circuits_before_token_parsing` |
| HS256 p99 ≤ 5µs | **p50 2.3µs / p99 3.9µs**（20k 次，release） | `cargo test --release -p rust-webx-host --test auth_budget_test hs256_p99 -- --ignored --nocapture` |
| 匿名 0 分配（认证层） | ✅ **认证逻辑 0 增量**：开启认证 +2.00，空中间件同样 +2.00（async_trait 的 invoke/after boxed future）；差值 = 0 | `ALLOC_PROBE_AUTH=1` vs `ALLOC_PROBE_NOOP_MW=1` 各跑 `alloc-probe compare 2000` |
| 整请求分配（额外观察） | ❌ framework 23.86 / hyper 6.00，tax **17.86 allocs/request**（与认证无关） | `cargo run --release -p alloc-probe -- compare 2000` |
| JWKS 缓存命中 0 I/O | 通过（fetch 计数不增） | `auth_jwks_test::jwks_cache_hits_and_unknown_kid_refreshes_once` |
| 吊销负缓存 0 后端 I/O | 通过（缓存测试） | `session_cache_test` |
| Argon2 有界并发 | 通过（上限 + 饱和 503） | `login_hardening_test::{bounded_hasher_caps_concurrency,bounded_hasher_fails_fast_when_saturated}` |

> 匿名 0 分配无法在 workspace `unsafe_code = "forbid"` 下用计数 allocator 断言；当前以“无 header 直接短路 + 无 token 解析”作为预算守卫，若要严格分配断言需独立 benchmark crate。

## 3. 对抗与鲁棒性

- **JWT malformed**：随机字节（0–64 KiB）、超长 token、空 token 均返回 Err/None，不 panic。
- **WebAuthn malformed**：随机 attestation/authData/signature（0–1 KiB）不 panic，只返回结构错误。
- **JWKS/吊销后端故障**：JWKS fetch 失败 → 503；吊销存储失败 → 503（不降级匿名）。
- **刷新令牌**：并发轮换单赢家、重用撤销整族、错误 secret 不撤销族、过期不撤销族。
- **DPoP**：`jti` 重放、`htu/htm/ath/iat` 篡改、`cnf.jkt` 不匹配全部 401。

## 4. 2.5.0 GA 核心门禁（已达成）

- [x] M1–M4 能力与对抗用例（§1 矩阵）。
- [x] 全 workspace `cargo fmt --all --check` + `clippy --workspace --all-targets -D warnings` + `test --workspace` 全绿。
- [x] **匿名 0 分配（认证层）**：auth 中间件增量 +2.00 与空中间件 +2.00 完全一致，认证逻辑本身 0 分配；+2 是 `async_trait` 中间件 invoke/after 的 boxed future 成本。
- [x] HS256 p99 3.9µs；JWKS/吊销缓存 0 I/O；Argon2 有界并发。
- [x] 启动 fail-closed（JWT/JWKS/mTLS/CSRF/Production guards）。
- [x] N-2 兼容（旧 JWT 配置、deprecated `jwt_secret`、bcrypt 迁移路径）。
- [x] mTLS 实际握手 e2e；WebAuthn malformed fuzz。
- [x] Chaos：连接风暴 503、优雅关闭、静默上游超时（3/3）。
- [x] Soak 600s + **30min（1802s，0 失败）**。
- [x] 2.5 分支/版本 `release/2.5` + `2.5.0`；CHANGELOG、迁移指南、发布说明、威胁模型、性能报告。

## 4.1 Post-GA 部署验证（不阻塞 2.5.0 GA）

- [x] Duende 完整授权码 + PKCE + id_token 交换；GitHub OAuth2 完整授权码+token+`/user`（真实用户 `lusida2026`）；Google/Microsoft discovery+JWKS 已联网；其他厂商待各自 client 注册。
- [x] WebAuthn 真实浏览器 API + CTAP2 虚拟认证器（Chromium）注册/断言；物理认证器、`packed`/`tpm`、RS256/EdDSA 仍可选。
- [ ] 整请求分配税 17.86 allocs/request（与认证无关，独立性能工作流）。
- [ ] `jwt_secret` shim 下一 major 删除计划。

## 4.2 OAuth 2.0 社交登录扩展（rust-webx-oauth2）

- [x] 授权码 + PKCE + state、token 交换、userinfo → `IdentityUser`。
- [x] 内置 GitHub/Facebook/Google/Microsoft/GitLab/Gitee/DingTalk/Feishu/WeChat/Weibo/QQ/Douyin 预设；Alipay 使用 RSA2 签名网关客户端。
- [x] Host 可直接挂载示例：`crates/oauth2/examples/github_host.rs`（start + callback + 本地 JWT）。
- [x] 真实 GitHub OAuth：authorize → code → token → `/user`，含 `/user/emails` 回退。
- [x] 本地 mock 测试：token/userinfo/email 回退/HTTPS 校验/PKCE。
- [x] 与 OIDC 边界：GitHub 等不接入 `OidcClient`；文档 `09-auth-security/oauth2-social.md`。

## 5. CI 门禁

`.github/workflows/stability.yml`（nightly / 手动）：

- `auth-budget`：release 下 HS256 p99 ≤5µs + malformed auth fuzz；
- `auth-interop`：OIDC 私有 CA HTTPS e2e、mTLS 握手/指纹注入、mTLS scheme + build fail-closed；
- `auth-live`（仅 `workflow_dispatch`）：Google/Duende/Microsoft 真实 discovery + JWKS（公网依赖，不阻塞 PR）；
- `webauthn-browser`（仅 `workflow_dispatch`）：Chromium CTAP2 虚拟认证器真实 create/get → Rust 验签；
- `soak` / `chaos`：`SOAK_SECONDS`（默认 300s）与连接风暴/优雅关闭/静默上游。

PR CI（`ci.yml`）保持快速门禁；耗时的 `#[ignore]` 用例只在 Stability 跑。

## 6. 复现命令

```powershell
cargo test -p rust-webx-host --test auth_budget_test
cargo test --release -p rust-webx-host --test auth_budget_test hs256_p99 -- --ignored --nocapture
cargo test -p rust-webx-webauthn
cargo clippy -p rust-webx-engine -p rust-webx-host --all-targets -- -D warnings
cargo run --release -p alloc-probe -- compare 1000
$env:ALLOC_PROBE_AUTH='1'; cargo run --release -p alloc-probe -- compare 2000
$env:ALLOC_PROBE_NOOP_MW='1'; cargo run --release -p alloc-probe -- compare 2000
```
