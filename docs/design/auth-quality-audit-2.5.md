# 认证质量审计（2.5.0）

> 视角：架构师复核「完整性、健壮性、高效性」。只记录本轮实际核验与修复；
> 未执行的外部环境项在 §4 明列。

## 1. 审计范围

- JWT 校验/签发/吊销/JWKS；
- OIDC discovery/PKCE/id_token/at_hash；
- Cookie 会话/CSRF/滑动过期；
- 登录限流/Argon2/口令策略；
- refresh 轮换/重用检测/EF 持久化；
- TOTP/恢复码/assurance；
- WebAuthn 注册/断言；
- mTLS/DPoP/Basic；
- RFC 7662/7009；
- 性能预算与缓存边界。

## 2. 本轮发现并修复

| # | 问题 | 修复 | 证据 |
|---|---|---|---|
| 1 | JWKS 单键但 token 无 `kid` 被错误拒绝 | 唯一候选键可推断；多候选必须拒绝（fail-closed） | `auth_jwks_test` 9 例，含无 kid 单键/歧义 |
| 2 | 公共 OIDC 客户端可能生成无 PKCE 的授权 URL | `authorization_url` 对 `ClientAuth::None` 强制 `.pkce_s256()` | `public_client_authorization_url_requires_pkce` |
| 3 | OIDC 配置可设无限 leeway/JWKS TTL/max_keys | `OidcConfig::validate` 上限 300s/3600s/128 | `oidc_config_rejects_unbounded_values` |
| 4 | 会话 Cookie 可配置 `HttpOnly=false` | 启动期拒绝，避免 XSS 读取会话 id | `http_only_is_mandatory` |
| 5 | 吊销负缓存 TTL 可无限大 → 撤销延迟/时间溢出 | 上限 300s，`Instant` 加法安全 | `oversized_ttl_is_clamped` |
| 6 | JWT builder 可接受空白 issuer/audience | 启动期拒绝空白值 | `empty_issuer_is_rejected` 等 |
| 7 | 会话可无 subject（空身份仍认证） | `issue_for/issue_session` 拒绝；认证时删除并匿名 | `empty_subject_session_is_rejected` |
| 8 | Microsoft 多租户支持 PKCE 但不声明 | `OidcConfig.require_pkce_advertised` 显式 opt-in（默认 fail-closed） | `unadvertised_pkce_requires_an_explicit_opt_in`、`oidc_live_test` |
| 9 | token endpoint 4xx（invalid_grant/client）被映射为 502 | 4xx → 401；仅 5xx → 503 | 真实 Duende token endpoint 负例断言 401 |
| 10 | workspace 同时启用 ring/aws-lc 时 rustls 无默认 provider，客户端/服务端 TLS 可能 panic | client/server TLS 配置显式安装 ring provider（已被应用先安装则保留） | `webx-ai` 100 例在 workspace feature set 下通过 |

## 2.1 真实联网核验（2026-11-21）

`cargo test -p rust-webx-oidc --test oidc_live_test -- --ignored --nocapture`：

- Google `accounts.google.com`：discovery 精确 issuer、2 个 RSA JWKS、`kid` 解析 + RS256 验签密钥解析；
- Duende demo：discovery、1 个 RSA JWKS；
- Microsoft 租户 `72f988bf-.../v2.0`：精确 issuer、`require_pkce_advertised=false` 下的 discovery/JWKS 路径；
- Duende token endpoint：伪 code 被拒绝并映射为 **401**（不是 502）；
- **Duende 完整授权码流程**：Chromium + 虚拟登录（bob/bob）→ 真实 authorize/callback → 本框架 `exchange_code` + PKCE + id_token JWKS 校验成功（`sub=2`，email 未包含）；
- **GitHub OAuth2（非 OIDC）**：`rust-webx-oauth2` 用真实 GitHub 账号完成 authorize → code → client secret 换 token → `GET /user` → `IdentityUser`（`subject=209404271`，email 回退 `/user/emails`）；`github_live_test` 可复现；明确不接入 OIDC；
- **OAuth2 provider crate**：内置 GitHub/Facebook/Google/Microsoft/GitLab/Gitee/DingTalk/Feishu/WeChat/Weibo/QQ/Douyin + Alipay 签名网关 + `preset(name)`；mock 覆盖 token/userinfo/email 回退/PKCE/HTTPS 校验；
- **WebAuthn 浏览器 e2e**：Chromium CDP CTAP2 虚拟认证器完成 `navigator.credentials.create/get`，原始 attestation/authData/signature 由 Rust RP 验签通过（credential 32B、sign_count=2）；
- 未执行：其他厂商完整授权码登录（需要各自 client 注册/测试用户），因此不宣称全厂商互操作。

## 2.2 门禁结果（本轮结束时）

- `cargo fmt --all --check` ✅
- `cargo clippy --workspace --all-targets -- -D warnings` ✅
- `cargo test --workspace` ✅（Windows，含新增 `redis`/`nats`/`rabbitmq`/`kafka`、OIDC live 4、AI 100 等）
- `oidc_live_test`：Google / Duende / Microsoft + token 负例 3/3 ✅（本机）
- **Linux（Ubuntu 24.04，4 vCPU/3.6GB，rustc 1.95.0）**：
  - 1h soak：`3600.15s`，0 失败；60 次 RSS 采样，预热后稳定 **17,728 KB**（min 6,584 启动态，max 17,728，avg 17,540.7）；
  - 认证测试套件：host auth/JWKS/validation/route/mTLS/introspection/refresh/cookie/CSRF、OIDC 13+1+4 live、WebAuthn 6、MFA 13 全绿；
  - 公网限制：Google discovery 从该服务器超时（已改为跳过不可达 provider，Microsoft/Duende 仍真实通过）。

## 2.3 OAuth2 社交登录 provider（新增 crate）

`rust-webx-oauth2` 提供不依赖 OIDC 的社交登录：

- 内置 `github`、`facebook`、`google`、`microsoft(tenant)`、`gitlab(base)`、
  `gitee`、`dingtalk`、`feishu`、`wechat`、`weibo`，并可用
  `preset(name)` 配置化选择；
- 其他提供方用 `OAuth2Provider::custom` + builder 描述端点、token 字段名、
  userinfo JSON pointer、client auth 方式；
- `authorization_url` 生成 state + PKCE；`exchange_code`；`userinfo` →
  `IdentityUser`；access token 在 Debug 中脱敏；
- 真实 GitHub OAuth 已通过（含 `/user/emails` 回退）；其余 provider 使用公开
  端点 + mock/单元测试，未逐个申请厂商应用实测。

## 3. 已核验的不变量
- 空 `sub`/`iss`/`aud`/`alg=none`/算法混淆/超大 token → 401，不降级匿名；
- JWKS 拉取失败 → 503，绝不匿名放行；未知 `kid` 单次刷新后拒绝；
- Cookie 会话：`__Host-` + `HttpOnly` + `Secure` + `SameSite`，CSRF 双提交常量时间比较；
- refresh：SHA-256 存储、原子 CAS 轮换、重用撤销整族、错误 secret 不误撤销；
- TOTP：RFC 6238 向量、±skew 限制、重放拒绝；
- WebAuthn：challenge/origin/rpId/UP/UV/签名/计数器，`none` attestation 必须空 `attStmt`；
- mTLS：TLS 终止器注入指纹，明文/上游伪造头被剥离；
- DPoP：`typ/alg/jwk/cnf.jkt/htm/htu/iat/jti/ath`，重放存储；
- introspection/revocation：Basic 常量时间、未知/失效 token 不泄漏；
- 认证层匿名 0 分配；HS256 p99 3.9µs；JWKS/吊销缓存 0 I/O。

## 4. 残余风险 / 未执行

- 其他外部 IdP 的完整授权码登录未执行（Duende 已完成）；WebAuthn 已过 Chromium 虚拟认证器，物理硬件/更多 attestation 未执行；
- 1h 浸泡已在 Linux（4 vCPU/3.6GB）通过，RSS 预热后稳定 17,728 KB；
- `InMemorySessionStore`/`InMemoryRefreshTokenStore` 仅单进程；
- `CachedSessionStore` 跨实例撤销最多延迟 `ttl`（已上限 300s，建议 1–5s）；
- DPoP 的 `cnf` 为对象型时需自定义 issuer（默认 issuer 写字符串类型）；
- 整请求分配税 17.86 allocs/request 与认证无关，属独立性能目标。

## 5. 回归命令

```powershell
cargo test -p rust-webx-host --test auth_validation_test --test auth_jwks_test --test auth_jwt_test
cargo test -p rust-webx-oidc --test oidc_test --test oidc_https_e2e_test
cargo test -p rust-webx-host --lib auth_cookie
cargo test -p rust-webx-host --lib session_cache
cargo test -p rust-webx-webauthn
cargo test -p rust-webx-mfa
cargo clippy --workspace --all-targets -- -D warnings
```
