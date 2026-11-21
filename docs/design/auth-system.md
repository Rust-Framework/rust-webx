# rust-webx 认证体系：设计规划与实施细则（2.5 单线）

> 状态：**GA（2.5.0）**；目标版本 **2.5**（内部里程碑 M1–M4 完成）。GA 核心门禁见 `auth-conformance.md`，post-GA 部署验证见 §4.1；威胁模型见 `auth-threat-model.md`，性能见 `auth-performance-2.5.md`，质量复核见 `auth-quality-audit-2.5.md`，N-2 迁移见 `auth-migration-2.5.md`。
> 版本约定：2.2 / 2.3 / 2.4 已被其他领域并行占用；认证域**全部**工作包收敛到 `2.5` 分支，不再进入 2.2–2.4 的任何工作项。
> 基线：rust-webx 2.1.0（`dd23b92`）。本文所有 `文件:行` 已对照该提交核实。
> 上位约束：`docs/design/framework-constitution.md` 的 P1–P12、D3（端口 + starter crate）、D9（浏览器认证）、§4.2。本文只做认证域的展开与裁决，不重复宪法论证。
> 证据等级：**实测**（本仓库源码/测试）＞**规范**（RFC/OIDC/OWASP/NIST，附录 A）＞**推断**（威胁建模）。

---

## 0. 结论摘要

**判断：rust-webx 的认证骨架（scheme 链 + 端口 + 身份绑定 + 声明式/动态授权）方向正确，但只完成了"内部 API 的 Bearer 令牌"这一条主路径。距离行业顶尖缺的不是更多 scheme，而是三件事：可组合的凭证验证内核（算法/密钥/声明校验）、浏览器与联邦身份的一等支持、可量化的安全与性能门禁。**

六条结论：

1. **认证 = 从凭证到主体的可信映射**；信任根只有两类：**共享密钥/公钥**（签名令牌）与**服务端状态**（会话/一次性票据）。框架必须把这两类的签发、轮换、吊销、过期做成一等能力，而不是让每个应用重写。
2. **"流行"≠"都进 core"**。按宪法 D3：core 只放端口与身份模型；JWT/JWKS/Cookie/CSRF/Refresh 进 host（feature 化）；OIDC、TOTP、WebAuthn、EF 存储进独立 starter crate。**不做 IdP/SAML 服务端/LDAP/用户 CRUD**。
3. 当前代码有 **12 项已核实缺陷**，其中 **4 项 P0**：JWT 不校验 `iss`/`aud`（多环境共享密钥时 token 可串用）、仅 HS256 且无 JWKS/轮换（无法与任何外部 IdP 互操作）、空 `sub` 可形成"已认证但无主体"的身份、无浏览器 Cookie/CSRF 支持（宪法 D9 已列为认证域交付）。见 §4.2。
4. **认证热路径必须预算化**：匿名请求 0 分配；HS256 验签 p99 ≤5µs；JWKS/吊销缓存命中 0 I/O；Argon2 有并发闸门；JWKS/会话存储等外部依赖有超时、熔断、stale-while-error。
5. **安全默认 fail-closed**：坏配置启动失败（issuer/audience 缺失、算法不匹配、JWKS 非 HTTPS、Cookie 无 CSRF、生产弱密钥）；运行时"无法证明凭证有效/未吊销"一律拒绝，不做降级。
6. **验收不是"能登录"**，而是对抗测试 + 性能预算 + N-2 兼容。§6 的 M1–M4 每个里程碑都带可执行退出标准；2.5 分支内一次规划、分段交付，不拆回 2.2–2.4。

### 0.1 现状盘点（实测）

| 能力 | 现状 | 证据 | 缺口 |
|---|---|---|---|
| Scheme 链（Strategy + CoR） | ✅ 公开、可插拔、首个 `matches` 命中即处理 | `crates/core/src/auth.rs:112-127`、`crates/host/src/auth.rs:429-443` | 无 per-route 选择；无重叠诊断 |
| JWT Bearer（HS256） | ✅ `add_authentication()` + `Jwt.Secret/SecretFile` | `crates/host/src/server.rs:1222-1250` | 算法/iss/aud/leeway/大小不可配；无 JWKS |
| API Key | ✅ `ApiKeyScheme` + 失败限流 + HMAC store | `crates/host/src/auth.rs:217-357`、`crates/ai/core/src/api_key_store.rs` | 401 挑战头仍是 `Bearer`；scopes 依赖 claims 约定 |
| 口令 | ✅ `IPasswordHasher`（Argon2id，PHC，blocking 线程） | `crates/core/src/security/password.rs:27-53`、`crates/host/src/password_argon2.rs:150-181` | 无策略/防枚举/登录限流/并发闸门 |
| 吊销 | ✅ `ISessionStore`（按 `jti`），持久化可实现 | `crates/core/src/security/session.rs:22-105` | 无 TTL/GC；无 subject 索引（默认 501）；每请求存储调用无缓存 |
| 身份/授权 | ✅ `IdentityUser`/`RequestUser`、`#[authorize]`、`IDynamicAuthorizer`（2.x fail-closed） | `crates/core/src/auth.rs:355-419` | claim 规范化契约未文档化 |
| 浏览器会话 | ❌ 仅 `Authorization` 头；`Set-Cookie` 可写但无 scheme | `crates/host/src/auth_jwt.rs:199-212`、宪法 §4.2 P1 | 无 Cookie scheme / CSRF / 会话仓储 |
| 刷新令牌 | ❌ | 全仓无实现 | 轮换/重用检测/族撤销 |
| 联邦身份 | ❌ | 无 OIDC/JWKS 代码 | 资源服务器验签 + OIDC 客户端 |
| 可观测 | 🟡 仅启动日志 | `crates/host/src/auth_jwt.rs:1235-1241` | 按 scheme 的成功/失败/延迟/吊销指标 |

### 0.2 与宪法的对齐

| 宪法条款 | 对认证域的约束 | 本文落点 |
|---|---|---|
| P1 一个关注点一种写法 | 只保留一条认证主路径 + 文档化逃生舱 | §3.2 canonical pipeline；`IAuthenticationScheme` 为唯一扩展点 |
| P3 默认安全、失败关闭 | 坏配置启动失败；运行时不确定即拒绝 | §5.3 配置校验表；§4.3 负例 |
| P4 性能是预算 | 每层声明分配/延迟上限 | §5.4 |
| P7 一切有界 | 令牌大小、JWKS 键数、队列、超时 | §5.2/§5.4 |
| P9 端口而非插件 | 每个端口 ≥2 实现 + 契约测试 | `ISessionRepository`/`IRefreshTokenStore` 各有内存 + EF 实现 |
| D3 端口 + starter | 领域能力外置 | `rust-webx-oidc`/`-mfa`/`-webauthn`/`-auth-ef` |
| D9 浏览器认证 | Cookie/BFF + 一次性连接票据 | §5.2.3、§6 |

---

## 1. 第一性原理

### 1.1 认证的物理定义

```
凭证(credential) ──验证──▶ 主体(subject) ──授权──▶ 资源
      │                        │
      └── 信任根(trust root) ───┘
```

- 凭证只有四种物理形态：**持有物**（私钥/API Key/会话 cookie）、**知识**（口令/TOTP）、**继承断言**（JWT/OIDC id_token）、**服务端状态**（session id 查表）。
- 认证回答"你是谁"，授权回答"你能做什么"，会话回答"这次交互的有效期"。三者共用身份载体 `IdentityUser`，但生命周期不同，**不能揉成一个 `AuthService`**。
- 信任根必须可轮换：共享密钥会泄露、公钥会过期、会话会失效。

### 1.2 五个不可约约束

| # | 物理约束 | 设计后果 |
|---|---|---|
| C1 | 凭证会泄露 | 短寿命访问令牌 + 可吊销会话 + 最小权限 claim |
| C2 | 信任根会轮换 | `kid`/key ring/双密钥重叠窗口；JWKS 缓存 + stale-while-error |
| C3 | 浏览器是敌对环境 | `HttpOnly`/`Secure`/`SameSite` + CSRF 双提交 + 严格 CORS |
| C4 | 外部依赖会失败 | JWKS/会话存储/口令服务：超时、熔断、有界队列、fail-closed |
| C5 | 用户与运维会犯错 | 安全默认 + 启动期校验 + 可诊断错误码 + 配置生效报告 |

### 1.3 认证域判定规则（R1–R10）

| # | 规则 | 检验 |
|---|---|---|
| R1 | 标准优先，不造密码学 | 算法/格式引用 RFC；禁止自写 HMAC/JWT 编解码 |
| R2 | 一个凭证类型 = 一个 scheme | scheme 只做"匹配 + 验证 + claims"，不碰授权 |
| R3 | 失败关闭 | 无法验证/无法查吊销 → 拒绝；仅"无凭证"是匿名 |
| R4 | 热路径有界 | 令牌 ≤8KB、JWKS 键数有上限、Argon2 并发有闸门 |
| R5 | 端口 ≥2 实现 | 每个新端口先写内存实现 + 测试 fake，再写持久化实现 |
| R6 | 语义稳定 | 行为是 API；默认值变更走 CHANGELOG + 弃用 ≥2 minor |
| R7 | 可观测 | 每次认证决策有 scheme、结果、原因、耗时；不记录令牌 |
| R8 | 可退出 | 每个内建 scheme 都能用 `IAuthenticationScheme` 手写等价物替换 |
| R9 | 性能预算 | 新增每请求成本必须给分配/延迟上限并进 CI |
| R10 | 概念预算 | 新增 ≤2 个核心概念/阶段；能用现有 trait 表达就不加 |

---

## 2. "必须支持好"的内容

### 2.1 "支持好"的六条判据（Definition of Done）

一个认证能力只有全部满足才算"支持好"：

1. **标准符合**：对照 RFC/OIDC/OWASP ASVS 的 MUST 项；
2. **安全默认**：默认配置在 Production 可用且 fail-closed；
3. **双实现 + 契约测试**：端口至少内存与持久化两种实现，同一套契约测试；
4. **可观测**：指标 + 结构化日志 + 可诊断错误码；
5. **性能预算**：热路径分配/延迟上限进 CI；
6. **迁移路径**：N-2 兼容、弃用策略、配置报告。

### 2.2 能力矩阵（2.5 M1–M4）

| # | 能力 | 标准 | 为什么必须 | 归属 | 里程碑 |
|---|---|---|---|---|---|
| A1 | 用户名 + 口令 | NIST 800-63B、OWASP ASVS V2 | 最普遍的登录方式；也是被攻击最多的一环 | host（策略/限流）+ core 端口 | M1 |
| A2 | JWT HS256 访问令牌 | RFC 7515/7519/8725 | 已有；需补 `iss`/`aud`/算法/大小校验 | host | M1 |
| A3 | JWT 非对称 + JWKS | RFC 7517/8725、OIDC Core | 资源服务器与任何 IdP 互操作的前提 | host（feature `auth-jwks`） | M2 |
| A4 | 刷新令牌轮换 | OAuth 2.0 BCP（RFC 9700） | 长会话的安全前提；重用检测是 MUST 级建议 | core 端口 + host 服务 + `auth-ef` | M2 |
| A5 | API Key | 行业事实标准 | 数据面/SDK 调用；已有 | host + `ai` | ✅ 1.5 |
| A6 | Cookie 会话 + CSRF | RFC 6265bis、OWASP CSRF | 浏览器（SSE/EventSource、BFF）无法带 Bearer | host + core 仓储端口 | M1 |
| A7 | OIDC/OAuth2 登录 | OIDC Core、RFC 7636（PKCE） | 企业 SSO/社交登录的事实标准 | 独立 `rust-webx-oidc` + `rust-webx-oauth2`（OAuth2 社交） | M2 |
| A8 | TOTP MFA + 恢复码 | RFC 6238、NIST 800-63B AAL2 | 合规与后台账号的最低要求 | 独立 `rust-webx-mfa` | M2 |
| A9 | WebAuthn/Passkey | W3C WebAuthn L3 | 防钓鱼，消费级趋势 | 独立 `rust-webx-webauthn` | M3 |
| A10 | mTLS 客户端证书 | RFC 8446 | 企业内网/服务间 | engine + host scheme | M3 |
| A11 | HTTP Basic | RFC 7617 | 遗留集成、健康检查 | host（约 60 行） | M1 |
| A12 | 令牌内省/撤销端点 | RFC 7662/7009 | API 平台资源服务器 | 独立 starter | M4 |
| A13 | DPoP（发送方约束） | RFC 9449 | 高价值令牌防重放 | 独立 starter | M4 |

### 2.3 明确不做

- **不做 IdP/OP、SAML 服务端、LDAP/AD 目录服务**：SAML/LDAP 通过前置 IdP 或网关换算成本框架令牌。
- **不做用户 CRUD、注册/找回密码/改密流程、社交登录 UI/验证码**：属于应用领域；OAuth2 社交协议客户端由 `rust-webx-oauth2` 提供，账户绑定/角色仍由应用决定。
- **不做自定义密码学**：禁止自写 JWT/HMAC/Argon2；只允许组合成熟 crate。
- **不做"万能 AuthService"**：认证、会话、授权、运维各自独立。

---

## 3. 目标架构

### 3.1 分层与依赖方向

```
应用/业务 crate
   │  使用 IdentityUser / RequestUser / #[authorize] / 端口
   ▼
starter crates（可选，同版本号）
   rust-webx-oidc  rust-webx-mfa  rust-webx-webauthn  rust-webx-auth-ef
   │                        │
   ▼                        ▼
rust-webx-host（scheme 实现、JWT/JWKS、Cookie/CSRF、Refresh、口令、限流）
   │  实现 core 的端口
   ▼
rust-webx-core（IAuthenticationScheme / IClaims / 端口 / IdentityUser / 配置）
```

依赖方向单向；core 不依赖任何密码学 crate。这与宪法 D3 一致。

### 3.2 Canonical 认证管道（唯一主路径）

```
1. parse      : 读取凭证头（大小/重复值有界），O(1) 形状检查
2. match      : 按注册顺序找第一个 matches=true 的 scheme（cheap，无 I/O）
3. verify     : 验签/查 hash/查会话；算法与密钥由配置固定（无协商）
4. normalize  : 校验 claims 契约（sub 非空、角色/权限形状、租户），得到 IClaims
5. revoke     : jti/会话吊销检查（缓存优先；存储失败 = 503 fail-closed）
6. identity   : ctx.set_claims；请求字段 #[fromclaim] 绑定 IdentityUser
7. authorize  : #[authorize] / IDynamicAuthorizer（独立阶段，不混入认证）
8. handler    : 业务只读 RequestUser
```

语义表（必须写进手册与测试）：

| 情况 | 受保护端点 | 公开端点 | 备注 |
|---|---|---|---|
| 无凭证 | 401 | 匿名 | 公开端点必须显式 `#[fromclaim(optional)]` |
| 凭证格式正确但无效/过期 | 401（strict 可选：立即） | 匿名（默认） | `Jwt.InvalidTokenBehavior` 控制 |
| 吊销命中 | 401 | 401 | 凭证问题，不是匿名 |
| 有效但角色/权限不足 | 403 | 403 | 授权阶段 |
| 有效但无租户 | 403 | 403 | scope 问题 |
| 存储/JWKS 不可用 | 503 + `Retry-After` | 503 | 不允许降级为匿名 |

### 3.3 抽象与所有权

| 抽象 | 位置 | 状态 | 实现 | 契约 |
|---|---|---|---|---|
| `IAuthenticationScheme` | core | 已有 | JWT/API Key/Cookie/Basic/自定义 | `matches` 无 I/O；首个命中即处理；`Ok(None)`=匿名 |
| `IAuthenticationHandler` | core | 已有 | `JwtAuth` 等低层验证器 | 只返回 claims |
| `IClaims` | core | 已有 | `JwtClaims`/`ApiKeyClaims`/`SessionClaims` | `sub` 非空；roles/permissions 为字符串数组 |
| `IKeyResolver` | host | 新增 | `StaticKeys`（测试）/`JwksResolver`（生产） | 按 `(kid, alg)` 返回公钥；未知 kid 触发一次有界刷新 |
| `ISessionStore` | core | 已有 | `InMemorySessionStore`/EF | 增补 `revoke_until(jti, exp)` 默认方法（additive） |
| `ISessionRepository` | core | 新增 | `InMemorySessionRepository`/`EfSessionRepository` | 会话 CRUD、idle/absolute 过期、按 subject 列出与全端登出 |
| `IRefreshTokenStore` | core | 新增 | 内存/EF | 以哈希存储；`rotate` 原子 CAS；重用检测撤销族 |
| `ILoginThrottle` | host | 新增 | 内存（滑窗）/分布式适配 | 账号+IP 双维度；指数退避；不永久锁死 |
| `PasswordPolicy` | host | 新增 | 规则结构体（非端口） | 长度/黑名单/最大长度；错误信息不泄露 |
| `IAuthMetrics` | host（复用 `IMeter`） | 新增指标名 | 默认 noop | 按 scheme/结果/原因打标签 |

> 概念预算：2.5 新增 **2 个 core 端口**（`ISessionRepository`、`IRefreshTokenStore`）+ 1 个 host 端口（`IKeyResolver`），其余复用现有抽象。

### 3.4 设计模式落点

| 模式 | 落点 | 解决什么 | 护栏 |
|---|---|---|---|
| **Strategy** | 每个凭证类型一个 `IAuthenticationScheme` | 避免 `match credential_type` 的上帝函数 | `matches` 必须无 I/O、无副作用 |
| **Chain of Responsibility** | `AuthenticationMiddleware` 按序尝试 | scheme 共存（JWT + API Key + Cookie） | 首个命中即终止，不 fallback（写进契约测试） |
| **Template Method** | verify 管道（parse→verify→normalize→revoke） | 所有 scheme 共享同一失败语义与审计 | 子类只实现 verify，不跳过 revoke |
| **Decorator** | 吊销检查、指标、超时包装 handler/scheme | 横切关注点与验证逻辑解耦 | 装饰顺序固定（验签 → 吊销 → 指标） |
| **Adapter** | OIDC/JWKS 适配为 `IKeyResolver`/scheme | 外部协议差异隔离在 crate 边界 | 不把 OIDC 类型泄进 core |
| **Facade** | `HostBuilder::{add_authentication,use_session_store,use_csrf,...}` | 一行装配安全默认 | 每个 facade 方法必须有生产守卫 |
| **Builder** | `JwtValidation::builder()` | 校验参数多且有默认值 | `build()` 做交叉校验（fail-fast） |
| **Null Object** | 匿名 = `None` claims | 公开端点无需特判 | 匿名必须显式声明 |
| **Circuit Breaker** | JWKS/会话存储/限流后端 | 依赖过载时不拖垮认证 | 熔断打开 → 503 fail-closed，不匿名 |
| **Cache-Aside** | JWKS、吊销负缓存 | 热路径零 I/O | 负缓存 TTL 有界（≤5s），撤销即时失效本地 |
| **Bulkhead** | Argon2/口令验证并发 | 防 CPU 打满 | 队列有界 + 超时 → 503 |
| **State Machine** | 会话/刷新令牌族（active/rotated/revoked/expired） | 重用检测与级联撤销 | 状态迁移只经 store 原子操作 |

**明确避免**：`AuthService` 上帝对象、继承式 scheme、每请求查库、进程级可变全局注册表、用 `unwrap` 处理凭证。

### 3.5 错误与 claims 契约

错误码（additive，映射到现有 401/403/503 problem+json）：

| code | HTTP | 触发 | 客户端动作 |
|---|---|---|---|
| `auth_required` | 401 | 无凭证 | 重新登录 |
| `invalid_token` | 401 | 验签/格式/算法失败 | 刷新或重新登录 |
| `token_expired` | 401 | `exp` 过期 | 刷新 |
| `token_revoked` | 401 | 吊销命中 | 重新登录 |
| `insufficient_scope` | 403 | 角色/权限不足（RFC 6750 `error="insufficient_scope"`） | 申请权限 |
| `account_locked` | 429 | 登录限流 | 退避后重试 |
| `auth_unavailable` | 503 | JWKS/存储不可用 | 重试（带 `Retry-After`） |

claims 契约（所有 scheme 归一化后必须满足）：

- `sub`：非空、≤256 字节；空主体 token 一律无效；
- `iss`/`aud`：配置了就必须精确匹配；
- `exp`：必须存在且在 leeway 外无效；`nbf` 存在则校验；`iat` 不得晚于 now+leeway；
- `roles`/`permissions`：字符串数组，单项 ≤128 字节、总数 ≤256；
- `tenant_id`：字符串；**只来自 claims，不来自 body/query**；
- `amr`/`acr`：MFA/step-up 的事实来源（M2 起）；
- 令牌/claims 总大小上限 8KB（header 总大小另有传输层上限）。

---

## 4. 对抗审查

### 4.1 威胁模型（STRIDE）

| 威胁 | 资产 | 攻击面 | 防线 |
|---|---|---|---|
| Spoofing | 口令、令牌、API Key | 登录端点、Bearer 头、Cookie | Argon2id、短寿命、恒定时间比较、MFA、防枚举 |
| Tampering | JWT、会话 id | 客户端可改的一切 | 签名/加密、`HttpOnly`、服务端状态 |
| Repudiation | 登录/撤销事件 | 审计缺失 | 结构化审计事件 + trace_id |
| Information disclosure | 令牌、密钥、口令哈希 | 日志、错误详情、时序 | 脱敏、统一 401 detail、恒定时间 + dummy verify |
| Denial of service | Argon2、JWKS、会话存储 | 登录爆破、超大 token、JWKS 风暴 | 限流、大小上限、单飞、熔断、并发闸门 |
| Elevation of privilege | claims、动态授权 | 客户端伪造 tenant/roles、authorizer 漏洞 | claims 只信签名、服务端注入租户、fail-closed |

### 4.2 已核实缺陷（按严重度）

**定级口径**：P0 = 安全正确性缺陷或阻塞主流场景；级别按影响定，**不等于阶段**（例如 JWKS 因依赖网络能力排在 M2，但仍是 P0 能力）。宪法 §4.2 将"JWT 无 Issuer/Audience"列为全局 P2 待办；本域按 RFC 8725 的 MUST 与跨环境串用风险重定为 P0（修复成本 <1 天）。

| ID | 级别 | 缺陷 | 证据 | 攻击/影响 | 修复 |
|---|---|---|---|---|---|
| F-A | **P0** | JWT 不校验 `iss`/`aud` | `crates/core/src/config.rs:91-115`（无字段）；`crates/host/src/server.rs:1226-1228`（`Validation::default()`） | 多环境/多服务共享 secret 时，staging token 可打生产；跨服务 token 混用 | 新增 `Jwt.Issuer/Audience`，配置即强制校验；启动期 fail-fast |
| F-B | **P0** | 仅 HS256，无 JWKS/`kid`/轮换 | `crates/host/src/auth_jwt.rs:261-266` | 无法验证任何 OIDC IdP 的 RS256 token；密钥轮换需停机 | `JwksResolver` + RS256/ES256/PS256 算法白名单 + 双密钥重叠 |
| F-C | **P0** | 空 `sub` 可成为已认证身份 | `crates/core/src/auth.rs:361-369`（`subject()` 直接转 `id`）；`crates/host/src/auth_jwt.rs:96-105`（`#[serde(default)] sub`） | 主体为空的 token 能通过 `#[fromclaim]` 绑定，审计/租户逻辑失真 | verify 后强制 `sub` 非空（含自定义 scheme 契约测试） |
| F-D | **P0** | 无 Cookie 会话/CSRF | `crates/host/src/auth_jwt.rs:199-212`；宪法 §4.2 P1 | 浏览器 `EventSource`/BFF 无法认证；若用户自建 Cookie 认证则无 CSRF 防线 | `SessionCookieScheme` + `CsrfMiddleware` + 启动期守卫 |
| F-E | P1 | "无效 token = 匿名" 无严格模式 | `crates/host/src/auth_jwt.rs:218-234` | 显式出示坏凭证的请求在公开端点继续，掩盖误配置；与 OAuth 资源服务器语义不一致 | 新增 `Jwt.InvalidTokenBehavior = Anonymous(默认)/Reject`；`Reject` 直接 401 |
| F-F | P1 | 吊销存储无 TTL/GC、无 subject 索引、每请求无缓存 | `crates/core/src/security/session.rs:50-105` | 内存单调增长；`revoke_all_for_subject` 501；DB store 每请求一次往返 | `revoke_until` + TTL GC；`ISessionRepository` 提供 subject 索引；负缓存 ≤5s |
| F-G | P1 | 无刷新令牌轮换 | 全仓无实现 | 应用自造轮换，重用检测普遍缺失 → 长期会话被窃后无法发现 | `IRefreshTokenStore` + 轮换 + 重用即撤销族 |
| F-H | P1 | 口令无策略/防枚举/登录限流/并发闸门 | `crates/host/src/password_argon2.rs:150-181`；`crates/host/src/auth.rs` 仅 API Key 有失败限流 | 用户枚举（时序）、凭证填充、Argon2 CPU DoS | `PasswordPolicy` + dummy verify + `ILoginThrottle` + semaphore |
| F-I | P1 | 无认证指标/审计事件 | `crates/host/src/auth_jwt.rs:1235-1241`（仅启动日志） | 无法回答"401 来自哪个 scheme/原因"；on-call 盲飞 | `IMeter` 指标 + 结构化事件（无令牌） |
| F-J | P2 | 全局 `init_jwt_secret`/`jwt_secret()`（panic） | `crates/host/src/auth_jwt.rs:457-476` | 多 Host 同进程测试串密钥；未初始化即 panic | 2.5 弃用警告，下一 major 删除；只保留 `JwtSecret` DI |
| F-K | P2 | 401 挑战固定 `Bearer realm` | `crates/host/src/problem_response.rs:76-88` | API Key 客户端收到错误挑战；无 `error=`/`scope` | scheme 感知挑战（`IHttpContext` 扩展），按 RFC 6750 输出 |
| F-L | P2 | scheme 首个命中即终止，无 per-route 选择/重叠诊断 | `crates/host/src/auth.rs:429-443` | BFF Cookie + 数据面 API Key 同 Host 时可能误吞凭证 | 契约测试 + 启动诊断；per-route 选择留 M3 按需求评估 |

### 4.3 对抗测试清单（红队用例，必须进 CI）

| 攻击 | 测试 | 期望 |
|---|---|---|
| `alg=none` / 算法混淆（RSA 公钥当 HS256 secret） | 构造对应 token 打受保护端点 | 401，`invalid_token`；日志有原因 |
| 未知/伪造 `kid`、`kid` 路径穿越 | 构造 header，JWKS 返回畸形 | 拒绝；刷新有界（≤1 次/请求） |
| JWKS SSRF（私网/元数据地址） | 配置内网 URL | 启动失败或请求被 SSRF 守卫拦截 |
| JWKS 风暴 | 1000 并发未知 kid | 单飞合并；上游 QPS 有界；p99 不塌 |
| `exp`/`nbf`/`iat` 边界、超大 `exp` | 边界矩阵 | 仅在 leeway 内通过 |
| `iss`/`aud` 不匹配（跨环境 token） | staging token 打生产 | 401 |
| 空 `sub`、超长 `sub`、roles 注入 | 构造 claims | 401 / 拒绝 |
| 吊销后下一请求 | revoke → 立即请求 | 401（含多实例负缓存窗口断言） |
| 会话存储宕机 | fake store 返回 Err | 503，不是匿名放行 |
| Cookie 固定/CSRF | 登录前后 session id、跨站表单 | 登录轮换 session id；无 CSRF token 的写请求 403 |
| Refresh 重用 | 旧 refresh 再次使用 | 撤销整个族；下一请求 401 |
| 凭证填充/枚举 | 不存在用户 vs 错口令 | 状态码与耗时无显著差异（dummy verify） |
| Argon2 DoS | 并发 10k 登录 | 排队有界，超时 503 + `Retry-After`；CPU 不饱和 |
| 头注入/重复 Authorization | 双头、超长头、Unicode | 400/401；不出现"取第二个头"分歧 |
| 日志泄露 | 全链路日志扫描 | 无 token/口令/密钥明文字样 |

### 4.4 红队裁决

| 决策 | 裁决 | 理由 | 翻案条件 |
|---|---|---|---|
| 保留 scheme 链 first-match | **维持** | 概念最少；与 `add_authentication_scheme` 兼容 | BFF + 多凭证同 Host 的真实冲突 ≥2 例 |
| 保留"无效 = 匿名" | **修改**（加 strict 开关） | 登录端点需要；资源服务器需要严格语义 | 下一 major 若默认改 Reject，需 N-2 迁移 |
| `iss`/`aud` 不校验 | **推翻** | 跨环境串用是真实事故类 | 无 |
| 仅 HS256 | **推翻**（加 JWKS） | 联邦身份的最低门槛 | 无 |
| 吊销只按 jti | **修改**（TTL + 仓储） | 无状态令牌的固有短板需要补齐 | 无 |
| 全局 secret shim | **删除**（下一 major） | 违反 P2 无隐式状态 | 无 |
| 认证与授权分离 | **维持** | 职责单一；动态授权已有 fail-closed | 无 |

---

## 5. 实施细则

### 5.1 文件与 crate 规划（改哪些、为什么）

| 文件/包 | 改动 | 为什么 |
|---|---|---|
| `crates/core/src/security/refresh.rs` | 新增 `IRefreshTokenStore`/`RefreshToken`（哈希、族、代次） | 刷新轮换需要稳定端口 |
| `crates/core/src/security/session_repo.rs` | 新增 `ISessionRepository`/`SessionRecord` | Cookie 会话需要服务端状态 |
| `crates/core/src/security/session.rs` | 增补 `revoke_until` 默认方法 | TTL GC，additive |
| `crates/core/src/security/mod.rs` | 导出新端口 | 公共面 |
| `crates/core/src/config.rs` | `JwtSection` 增 `Issuer/Audience/Algorithms/ClockSkewSeconds/InvalidTokenBehavior/Jwks`；新增 `AuthSection`（Cookie/Csrf/Refresh/Throttle） | 配置驱动安全默认 |
| `crates/core/src/auth.rs` | 定义 claims 契约常量/校验助手；`AuthError` 码枚举（映射到 `Error`） | 归一化与错误码统一 |
| `crates/host/src/auth_jwt.rs` | `JwtValidation` builder；`sub` 非空；leeway；大小上限；`aud`/`iss`；`Reject` 模式 | 补 P0 |
| `crates/host/src/auth_jwks.rs` | 新增 `JwksResolver`（缓存/单飞/超时/SSRF 复用 `rust-webx-client`） | 非对称验签 |
| `crates/host/src/auth_cookie.rs` | 新增 `SessionCookieScheme` + cookie 属性 | 浏览器认证 |
| `crates/host/src/csrf.rs` | 新增 `CsrfMiddleware`（双提交 + Origin/Sec-Fetch-Site） | Cookie 的必备防线 |
| `crates/host/src/refresh.rs` | 新增轮换服务（issue/rotate/revoke_family） | A4 |
| `crates/host/src/login.rs` | 新增 `PasswordPolicy`、dummy verify、`ILoginThrottle`、Argon2 semaphore | A1 |
| `crates/host/src/auth.rs` | scheme 诊断（顺序/重叠警告）；401 挑战可定制 | F-K/F-L |
| `crates/host/src/server.rs` | `use_jwt_validation`/`use_csrf`/`use_refresh` 等 builder；Production 守卫 | 安全默认 |
| `crates/host/Cargo.toml` | features：`auth-cookie`/`auth-csrf` 默认开（零新依赖）；`auth-jwks`/`auth-refresh` 默认关，依赖预算评审通过后于 M2 默认开 | 可裁剪 |
| `crates/webx/src/lib.rs` | 导出新 API | 伞形 crate |
| `crates/auth/oidc/`（新） | OIDC 客户端（discovery/PKCE/token/id_token 验证/会话桥） | A7 |
| `crates/auth/mfa/`（新） | TOTP + 恢复码 | A8 |
| `crates/auth/webauthn/`（新） | WebAuthn/Passkey（M3） | A9 |
| `crates/auth/ef/`（新） | `EfSessionRepository`/`EfRefreshTokenStore` | 持久化 |
| `docs/rust-webx/09-auth-security/*` | 手册与负例 | 用户第一入口 |
| `scripts/`/CI | 认证负例、分配/延迟门禁、依赖方向检查 | 门禁 |

### 5.2 目标 API（拟，2.5）

**5.2.1 JWT 校验（配置优先，代码可覆盖）**

```rust
// appsettings.json
// "Jwt": {
//   "SecretFile": "data/jwt.key",
//   "Issuer": "https://id.example.com",
//   "Audience": "travel-api",
//   "Algorithms": ["HS256"],
//   "ClockSkewSeconds": 30,
//   "InvalidTokenBehavior": "Reject",
//   "Jwks": { "Url": "https://id.example.com/.well-known/jwks.json", "CacheSeconds": 300 }
// }

Host::builder()
    .add_authentication()
    .use_jwt_validation(JwtValidation::builder()
        .algorithms([Algorithm::RS256, Algorithm::ES256])
        .require_issuer("https://id.example.com")
        .require_audience("travel-api")
        .leeway(Duration::from_secs(30))
        .max_token_bytes(8192)
        .build()?);
```

规则：`build()` 交叉校验（非对称必须给 JWKS 或静态公钥；`Reject` 模式必须配置 `iss` 或 `aud` 之一；HS256 密钥 ≥32B）。Production 下 `reject` 为推荐默认，`anonymous` 需显式。

**5.2.2 刷新令牌**

```rust
// 登录成功后
let refresh = refresh_tokens
    .issue(RefreshSubject::new(user.id).with_tenant(tenant).with_client("web"))
    .await?;                       // 返回明文一次，库中只存 SHA-256

// 刷新
let rotated = refresh_tokens.rotate(&presented).await?; // 原子 CAS；旧值重用 → 撤销整族
```

存储记录：`token_hash`、`family_id`、`generation`、`subject`、`expires_at`、`used_at`、`status`。重用检测 = 命中 `used_at` 非空 → 撤销族并返回 401。

**5.2.3 Cookie + CSRF**

```rust
Host::builder()
    .add_authentication_scheme(Arc::new(SessionCookieScheme::new(
        session_repo,
        CookieOptions::default()   // HttpOnly; Secure; SameSite=Lax; Path=/
            .name("__Host-webx_session")
            .idle_timeout(Duration::from_secs(30 * 60))
            .absolute_timeout(Duration::from_secs(12 * 3600)),
    )))
    .use_csrf(CsrfConfig::double_submit()); // 无 CSRF 却启用 cookie scheme → 启动失败
```

- Cookie 只放**不透明 session id**（32B 随机），不放 JWT；会话状态在服务端，登出/全端下线即时生效。
- CSRF：双提交 cookie + `X-CSRF-Token` 头 + `Origin`/`Sec-Fetch-Site` 复核；安全方法（GET/HEAD/OPTIONS）豁免。
- 登录成功轮换 session id（防固定）；权限变更（改密/提权）强制轮换。

**5.2.4 口令与登录防护**

```rust
let policy = PasswordPolicy::nist().min_len(12).max_len(1024);
policy.validate(&password)?;                    // 不要求大小写/符号组合

// 未知用户也要跑一次等价成本的 verify，避免时序枚举
let ok = match find_user(&id).await? {
    Some(user) => hasher.verify(&password, &user.phc).await?,
    None => { hasher.verify(&password, DUMMY_PHC).await?; false }
};
```

`ILoginThrottle`：账号（归一化）+ IP 双桶，指数退避，超限 429 + `Retry-After`；**不永久锁定**（防锁定 DoS），管理员可清除。Argon2 验证并发 = `min(cpus, 4)`，队列 32，等待超时 100ms → 503。

**5.2.5 OIDC（独立 crate，M2）**

```rust
let oidc = OidcClient::discover("https://id.example.com", http_client).await?;
let url = oidc.authorization_url(AuthorizationRequest::new("https://app.example.com/cb")
    .pkce_s256()
    .state(csrf_state)
    .nonce(nonce));
let session = oidc.exchange_code(code, pkce_verifier, nonce).await?; // 校验 id_token: iss/aud/exp/nonce/alg
```

会话桥两种模式，二选一：① 换发本框架短寿命 JWT（推荐，复用现有吊销/授权）；② 建立 `ISessionRepository` 会话（BFF）。

**5.2.6 指标（复用 `IMeter`）**

| 指标 | 标签 | 用途 |
|---|---|---|
| `auth.requests` | scheme, outcome(anonymous/ok/401/403/429/503) | 登录/鉴权健康度 |
| `auth.duration` | scheme | p99 预算 |
| `auth.jwks.fetch` | result, cached | 依赖健康 |
| `auth.revocation.cache` | hit/miss/error | 存储压力 |
| `auth.refresh.reuse` | — | 安全告警（应长期为 0） |

### 5.3 配置与启动校验（fail-closed）

| 条件 | 行为 |
|---|---|
| `add_authentication()` 且无密钥/无 JWKS | Production 启动失败 |
| `Algorithms` 含非对称但无 `Jwks.Url`/静态公钥 | 启动失败 |
| `Jwks.Url` 非 HTTPS 或命中私网（未显式允许） | 启动失败 |
| `Issuer`/`Audience` 配置了但为空串 | 启动失败 |
| Cookie scheme 启用但 `use_csrf` 未注册 | 启动失败 |
| `SameSite=None` 但无 `Secure` | 启动失败 |
| 刷新令牌 TTL > 绝对上限或 = 0 | 启动失败 |
| 生产口令策略低于 12 位 | 启动失败（可用显式 opt-out，但打印告警） |
| CORS `allow_credentials=true` + `*` | 启动失败（已有守卫） |

配置报告：启动时打印"认证生效报告"（scheme 顺序、算法、iss/aud、JWKS 来源、TTL、限流），与 Spring Boot `--debug` 对齐（宪法 §2.1）。

### 5.4 性能预算（CI 可执行）

| 路径 | 预算 | 保障手段 |
|---|---|---|
| 匿名请求（无凭证） | 0 分配，≤1µs | `matches` 只做头形状/存在性检查 |
| HS256 验签（1KB token） | p99 ≤5µs，≤8 allocs | `DecodingKey` 缓存；claims 预分配；避免中间 `String` |
| RS256/ES256 验签 | p99 ≤80µs（RSA）/≤150µs（EC） | 公钥缓存；`jsonwebtoken` 单次解析 |
| JWKS 命中 | 0 网络 I/O，≤2µs 查找 | `ArcSwap` 快照 + `kid` 哈希表 |
| JWKS 未命中 | ≤1 次刷新，≤2s 超时 | 单飞（in-flight 合并）+ 熔断 + stale-while-error |
| 吊销检查 | 缓存命中 0 I/O；未命中 ≤50ms | 负缓存 ≤5s + 本地撤销即时失效 + 存储熔断 |
| Argon2 验证 | 并发 ≤min(cpus,4)，排队 ≤100ms | semaphore + 有界队列；超时 503 |
| 令牌解析 | 输入 ≤8KB，header ≤传输层上限 | 先验大小再分配 |

基准方法：criterion（墙钟）+ 分配计数（固定端点）+ 同机基线与候选各 5 轮取中位；只对确定性信号设硬门禁（宪法 §6.1）。

### 5.5 可观测与运维

- **日志**：`scheme`、`outcome`、`reason_code`、`trace_id`、`route`；**禁止**记录 token/口令/密钥/完整 claims。
- **审计事件**：登录成功/失败、登出、全端下线、密钥轮换、刷新重用（安全告警）。
- **健康检查**：JWKS 过期超过 TTL → `degraded`（不 fail，stale 可用）；会话/刷新存储不可用 → `/health/ready` fail。
- **轮换 runbook**：① 发布新密钥（`kid`）并保留旧 key；② 验证新 token；③ 旧 key 保留 ≥ 最大 token TTL + 时钟偏移；④ 移除旧 key。
- **应急**：`revoke_all_for_subject` + 全局 `kid` 下线；刷新族批量撤销；配置报告与指标用于确认生效。

### 5.6 兼容与迁移

- 2.5 只做加法 + 安全修复：`sub` 空值拒绝、Production 守卫增强（CHANGELOG 标注行为变化）。
- `JwtSection` 新字段全部 `#[serde(default)]`；旧 appsettings 行为不变（除安全修复）。
- `init_jwt_secret`/`jwt_secret()` 2.5 标记 deprecated（保留一个 minor），下一 major 删除。
- 新 feature 默认值按"安全默认开、依赖可关"：`auth-cookie`/`auth-csrf` 默认开；`auth-jwks`/`auth-refresh` 于 M2 评估依赖预算后默认开。
- N-2 升级测试：旧示例代码零改动编译（除弃用警告）。

### 5.7 测试策略

| 层 | 内容 | 工具/位置 |
|---|---|---|
| 单元 | 校验矩阵（alg/iss/aud/exp/nbf/leeway/size）、claims 归一化、口令策略 | `crates/host/tests/auth_validation_test.rs` |
| 契约 | 每个 scheme 同一套语义测试（匿名/无效/吊销/存储故障） | `crates/host/tests/scheme_contract_test.rs` |
| 集成 | Host e2e：HS256/RS256/JWKS 轮换/Cookie+CSRF/Refresh 重用 | `crates/webx/tests/auth_*_test.rs` |
| 规范向量 | RFC 7515 附录 A、RFC 7519 示例、OIDC id_token 负例 | 独立 test data |
| 模糊 | Authorization 头、JWT header JSON、Cookie、JWKS JSON | `cargo-fuzz` 语料零 panic |
| 负载 | 匿名/HS256/RS256 吞吐与 p99；JWKS 风暴；Argon2 饱和 | criterion + 开环压测 |
| 混沌 | JWKS 宕机/慢、会话存储宕机、时钟偏移、SIGTERM 带载 | 现有 chaos 场景扩展 |
| 安全回归 | §4.3 全表 | CI 必跑 |

---

## 6. 路线图（2.5 单线，M1–M4）

> 2.2 / 2.3 / 2.4 已分配给其他领域并行开发；认证域**全部**工作包收敛到 `2.5` 分支。2.5 内部分 M1–M4，按依赖串行；同一分支、同一版本线，不拆回 2.2–2.4。

### 6.0 分支与并行约定

- **唯一目标分支** `2.5`；工作项编号 `AUTH-2.5-M{1..4}-xx`。
- 不修改 2.2–2.4 正在开发的代码路径；需要共享的底层（`ProblemDetails`、`Error` codes、`IMeter`、配置报告）以 additive 方式先行或在 M1 内对齐。
- 所有新行为 feature-gated；默认值变更必须带 CHANGELOG + 迁移说明 + N-2 编译证据。
- 每个里程碑合并前：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test`、§4.3 负例、分配/延迟基准、`cargo-semver-checks`。
- 台账滚动更新：本文档内维护"工作项 / 状态 / 证据（文件:行或测试名）"表，未过门禁不得标 ✅。

### 6.1 M1 "Hardening"（2.5.0-alpha，第 1–8 周）

| 项 | 交付 | 退出标准 |
|---|---|---|
| AUTH-2.5-M1-01 | JWT 校验配置化（alg/iss/aud/leeway/size/sub 非空/Reject 模式） | §4.3 前 6 类负例全绿；启动校验表全绿 |
| AUTH-2.5-M1-02 | Cookie 会话 + CSRF + `ISessionRepository`（内存） | 浏览器 e2e 通过；登录轮换 session id；无 CSRF 启动失败 |
| AUTH-2.5-M1-03 | 口令策略 + dummy verify + `ILoginThrottle` + Argon2 闸门 | 枚举耗时差 <10%；1 万并发登录不饱和；429 语义正确 |
| AUTH-2.5-M1-04 | 吊销 TTL/GC + 负缓存 + subject 索引 | 内存不增长；revoke 下一请求生效（单实例 0 窗口） |
| AUTH-2.5-M1-05 | 认证指标 + 审计事件 + 配置报告 | 401 可按 scheme/原因聚合；日志零令牌 |
| AUTH-2.5-M1-06 | HTTP Basic scheme（约 60 行） | 标准向量通过；默认关闭 |
| AUTH-2.5-M1-07 | 文档/负例/参考实现合规 | docbit 不再手写 JWT/哈希（宪法 D12） |

### 6.2 M2 "Federation & Sessions"（2.5.0-beta，第 9–18 周）

| 项 | 交付 | 退出标准 |
|---|---|---|
| AUTH-2.5-M2-01 | JWKS + RS256/ES256 + `kid` 轮换 + 单飞/熔断 | 与至少 2 个真实 IdP 的 JWKS 互通；轮换不停机 |
| AUTH-2.5-M2-02 | 刷新令牌轮换 + 重用检测 + `auth-ef` 存储 | 重用 100% 撤销族；多实例一致 |
| AUTH-2.5-M2-03 | `rust-webx-oidc` MVP（discovery/PKCE/id_token 验证/会话桥） | 授权码 + PKCE 端到端；nonce/state 负例 |
| AUTH-2.5-M2-04 | `rust-webx-mfa`（TOTP + 恢复码 + `amr`/`acr` step-up） | RFC 6238 向量；重放拒绝；恢复码一次性 |
| AUTH-2.5-M2-05 | 认证指标/健康/轮换 runbook 完善 | 演练：密钥轮换 + 全端下线 <5 分钟 |

### 6.3 M3 "Passwordless & Transport"（2.5.0-rc，第 19–26 周）

| 项 | 交付 | 退出标准 |
|---|---|---|
| AUTH-2.5-M3-01 | WebAuthn/Passkey（注册/断言/条件 UI） | 主流浏览器矩阵通过；无 attestation 依赖 |
| AUTH-2.5-M3-02 | mTLS scheme（engine 暴露客户端证书） | 双向 TLS e2e；证书映射可配置 |
| AUTH-2.5-M3-03 | DPoP（发送方约束） | RFC 9449 负例（重放/错误 htm/htu） |
| AUTH-2.5-M3-04 | per-route scheme 选择（仅当 ≥2 真实冲突） | 决策记录 + 回归；默认不启用 |
| AUTH-2.5-M3-05 | 删除 `jwt_secret` shim 的迁移路径 | 旧代码编译仅弃用警告；下一 major 移除 |

### 6.4 M4 "Interop & GA Hardening"（2.5.0 GA，第 27–30 周）

| 项 | 交付 | 退出标准 |
|---|---|---|
| AUTH-2.5-M4-01 | RFC 7662/7009 内省/撤销 starter | 资源服务器 e2e；撤销下一请求生效 |
| AUTH-2.5-M4-02 | 规范一致性 + 模糊 + 跨实现测试 | 向量全绿；fuzz 零 panic；与 2 个实现互验 |
| AUTH-2.5-M4-03 | 混沌/浸泡/过载 | 24h soak 无泄漏；JWKS/存储故障不降级 |
| AUTH-2.5-M4-04 | 文档/迁移/LTS 说明 + 发布门禁 | 六门禁 + 认证专项全绿；2.5.0 发布 |

**2.5.0 发布门槛**：M1–M4 全部退出标准满足；任何未满足项必须显式降级为 `2.5.x` 且登记风险，不回退版本线。

### 6.5 落地台账（滚动）

> 规则：未过 §6.0 门禁不得标 ✅；证据写 `文件:行` 或测试名。并行开发期间 fmt 全量可能被其他领域未格式化改动阻塞，需在合并前收敛。

| 工作项 | 状态 | 证据 |
|---|---|---|
| AUTH-2.5-M1-01 JWT 校验硬化 | 🚧 实现完成；host 全量测试 + clippy 通过，全量 fmt 待并行改动收敛 | `crates/core/src/config.rs`（`JwtSection` 新字段 + `JwtAlgorithm`/`InvalidTokenBehavior`）、`crates/host/src/auth_jwt.rs`（`JwtValidation`/`JwtAuth`/`sub`/`iat`/size/Reject）、`crates/host/src/server.rs`（`use_jwt_validation` + 默认 issuer 写入 `iss`/`aud`）、`crates/host/tests/auth_validation_test.rs`（22 例）、`docs/rust-webx/09-auth-security/jwt-authentication.md` |
| AUTH-2.5-M1-02 Cookie + CSRF + 会话仓储 | ✅ 完成（含 EF 持久化） | `crates/core/src/security/session_repo.rs`（`ISessionRepository`/`SessionRecord`/内存实现）、`crates/host/src/auth_cookie.rs`（`SessionCookieScheme`/`CookieOptions`/`SameSite`）、`crates/host/src/csrf.rs`（双提交 + `Sec-Fetch-Site`）、`crates/host/src/server.rs`（`use_csrf` + 无 CSRF 启动守卫）、`crates/core/src/action.rs`（多 `Set-Cookie` 不覆盖）、`crates/auth/ef/src/session.rs`（`EfSessionRepository`/`auth_sessions`）、`crates/host/tests/cookie_auth_test.rs`（8 例）、`crates/host/tests/csrf_test.rs`（9 例）、`crates/auth/ef` session 测试（4 例）、`docs/rust-webx/09-auth-security/cookie-session.md` |
| AUTH-2.5-M1-03 口令策略/防枚举/登录限流/Argon2 闸门 | 🚧 实现完成；host 全量测试 + clippy 通过 | `crates/host/src/login.rs`（`PasswordPolicy`/`verify_password_or_dummy`/`ILoginThrottle`/`MemoryLoginThrottle`/`BoundedPasswordHasher`）、`crates/host/src/server.rs`（默认 hasher 包闸门）、`crates/host/tests/login_hardening_test.rs`（10 例）、`docs/rust-webx/09-auth-security/token-session-password.md` |
| AUTH-2.5-M1-04 吊销 TTL/GC/负缓存/subject 索引 | 🚧 实现完成；host 全量测试 + clippy 通过 | `crates/core/src/security/session.rs`（`revoke_until`/TTL/`purge_expired`）、`crates/host/src/session_cache.rs`（`CachedSessionStore` 负缓存 + 本地失效）、`crates/host/src/server.rs`（`use_session_store_cached`）、`crates/host/tests/session_cache_test.rs`、`docs/rust-webx/09-auth-security/token-session-password.md` |
| AUTH-2.5-M1-05 认证指标/审计/配置报告 | 🚧 实现完成；host 全量测试 + clippy 通过 | `crates/host/src/auth.rs`（`AuthMetrics`/`AuthOutcome`/`auth.requests`/`auth.duration`）、`crates/host/src/server.rs`（`use_meter` + 启动配置报告）、`crates/host/tests/auth_metrics_test.rs`（8 例）、`docs/rust-webx/09-auth-security/token-session-password.md` |
| AUTH-2.5-M1-06 HTTP Basic | 🚧 实现完成；host 全量测试 + clippy 通过 | `crates/host/src/auth_basic.rs`（`BasicScheme`/`IBasicCredentialValidator`/`BasicClaims`）、`crates/host/tests/basic_auth_test.rs`（5 例）、`docs/rust-webx/09-auth-security/api-key-auth.md` |
| AUTH-2.5-M1-07 文档/负例/参考实现合规 | ✅ 完成 | 手册：`jwt-authentication.md`（iss/aud/Reject）、`cookie-session.md`、`token-session-password.md`（TTL/缓存/登录防护/指标）、`api-key-auth.md`（Basic）、`security-best-practices.md`（2.5 清单）；负例：`auth_validation_test`（22）、`cookie_auth_test`（8）、`csrf_test`（9）、`login_hardening_test`（10）、`auth_metrics_test`（8）、`basic_auth_test`（5）、`auth_fail_closed_test`（2）；参考实现 docbit 改用 `ITokenIssuer`/`IPasswordHasher`，移除 `bcrypt`/`jsonwebtoken` |

**M1 里程碑状态**：7/7 工作项实现并验证；负例、文档、参考实现合规已覆盖。全量 `cargo fmt --all` 仍受并行任务未格式化改动影响，合并前统一收敛，不改动他人代码。

**M2 进度**

| 工作项 | 状态 | 证据 |
|---|---|---|
| AUTH-2.5-M2-01 JWKS + RS256/ES256 + `kid` 轮换 | 🚧 实现完成；host 全量测试 + clippy 通过 | `crates/core/src/config.rs`（`JwtJwksSection`）、`crates/host/src/auth_jwks.rs`（`JwkSet`/`JwksResolver`/`StaticKeyResolver`/`HttpJwksFetcher`）、`crates/host/src/auth_jwt.rs`（多算法 + `kid` 解析 + 503 fail-closed）、`crates/host/src/server.rs`（JWKS-only 启动/守卫）、`crates/host/tests/auth_jwks_test.rs`（6 例 + 1 单元）、`docs/rust-webx/09-auth-security/jwt-authentication.md` |
| AUTH-2.5-M2-02 刷新令牌轮换 + 重用检测 | ✅ 完成 | `crates/core/src/security/refresh.rs`（`IRefreshTokenStore`/原子 `rotate`/内存实现）、`crates/host/src/refresh.rs`（`RefreshTokenService`/SHA-256/重用撤销族）、`crates/auth/ef/src/lib.rs`（`EfRefreshTokenStore`：条件更新 + 事务）、`crates/host/tests/refresh_token_test.rs`（7 例）、`crates/auth/ef` SQLite 测试（4 例）、`docs/rust-webx/09-auth-security/token-session-password.md` |
| AUTH-2.5-M2-03 `rust-webx-oidc` MVP | ✅ 完成（本地 HTTPS IdP e2e；外部厂商待补） | `crates/auth/oidc/src/{discovery,client,http,pkce}.rs`（discovery、PKCE S256、code exchange、id_token iss/aud/nonce/exp/alg/kid/at_hash、会话桥、私有 CA）、`crates/auth/oidc/tests/oidc_test.rs`（13）+ `oidc_live_test.rs`+ `oidc_https_e2e_test.rs`（真实 HTTPS discovery/JWKS/token）、`docs/rust-webx/09-auth-security/oidc.md` |
| AUTH-2.5-M2-04 `rust-webx-mfa`（TOTP） | ✅ 完成 | `crates/auth/mfa/src/{totp,recovery,assurance}.rs`（RFC 6238 SHA1/256/512、±skew、重放防护、恢复码哈希、`amr`/`acr`）、13 个单元测试、`docs/rust-webx/09-auth-security/mfa.md` |
| AUTH-2.5-M2-05 指标/健康/轮换 runbook | ✅ 完成 | `crates/host/src/auth_jwks.rs`（`JwksHealthCheck` + `ensure_fresh`）、`crates/host/src/refresh.rs`（`auth.refresh.events`）、`crates/host/tests/{auth_jwks,refresh_token}_test.rs`、`docs/rust-webx/09-auth-security/key-rotation-runbook.md` |

**M3 进度**

| 工作项 | 状态 | 证据 |
|---|---|---|
| AUTH-2.5-M3-03 DPoP（RFC 9449） | 🚧 实现完成；专项测试通过 | `crates/host/src/auth_dpop.rs`（proof 校验、cnf.jkt、htm/htu/iat/jti/ath、重放存储）、`crates/host/tests/dpop_test.rs`（5 例）、`docs/rust-webx/09-auth-security/dpop.md` |
| AUTH-2.5-M3-01 WebAuthn/Passkey | 🚧 MVP 验签完成；浏览器协议/更多 attestation 待补 | `crates/auth/webauthn/src/{lib,verify}.rs`（ES256、none attestation、challenge/origin/rpId/flags/counter）、4 个集成测试、`docs/rust-webx/09-auth-security/webauthn.md` |
| AUTH-2.5-M3-05 `jwt_secret` shim 迁移 | ✅ 完成（保留 N-2 兼容） | `dmbit/handlers/src/auth.rs` 迁移到 `ITokenIssuer`/`IPasswordHasher`；seed 改 Argon2id；`init_jwt_secret`/`jwt_secret` 标记 deprecated 并在下一 major 删除；docbit e2e 3 例通过 |
| AUTH-2.5-M3-02 mTLS | ✅ 完成 | `Tls.ClientCaPath/ClientAuth`、engine 客户端证书链验证 + 叶证书 SHA-256 指纹注入、`MtlsScheme`/`IMtlsIdentityResolver`、HostBuilder 启动期 fail-closed 校验；engine 10 例 TLS + host 4 例 mTLS |
| AUTH-2.5-M3-04 per-route scheme（按需） | ✅ 完成 | `AuthenticationMiddleware::with_route_policies` + `HostBuilder::use_auth_scheme_for`；5 例策略/匿名/全局链/路径段边界测试 |

**M4 进度**

| 工作项 | 状态 | 证据 |
|---|---|---|
| AUTH-2.5-M4-01/02 RFC 7662 / RFC 7009 | 🚧 服务原语完成；端点由应用装配 | `crates/host/src/introspection.rs`（active=false 语义、Basic 客户端认证、JWT/refresh 吊销）、`crates/host/src/refresh.rs::revoke_token`、`crates/host/tests/introspection_test.rs`（6 例）、`docs/rust-webx/09-auth-security/introspection.md` |
| AUTH-2.5-M4-03 性能预算/模糊/GA 台账 | 🚧 认证层 0 分配达成；整请求税/真实 e2e/浸泡待补 | `crates/host/tests/auth_budget_test.rs`（HS256 p99 3.9µs、malformed fuzz）、`crates/auth/webauthn/tests/webauthn_test.rs`、`crates/engine/tests/tls.rs`（mTLS 握手 e2e）、`crates/alloc-probe`（auth +2.00 == noop +2.00 ⇒ 认证层 0 增量）、`docs/design/auth-conformance.md` |

### 6.6 M2 入口（当前阶段）

M2-01 已完成：JWKS 拉取/缓存/单飞/SSRF 守卫、RS256/ES256/PS256 验签、`kid` 轮换与 JWKS-only 启动。M2-02 已完成：`IRefreshTokenStore` 原子轮换、SHA-256 存储、重用撤销族、`rust-webx-auth-ef` 持久化。M2-03 已完成：OIDC discovery、授权码 + PKCE S256、id_token 校验与会话桥、本地 HTTPS test IdP e2e；外部厂商互操作/`at_hash` 待补。M2-04 已完成：TOTP/恢复码/`amr`/`acr`。M2-05 已完成：JWKS readiness、refresh 指标、密钥轮换/吊销 runbook。**M2 完成**。M3：DPoP、WebAuthn MVP、`jwt_secret` shim 迁移、**mTLS 完成**、**M3-04 per-route scheme 完成**。M4：RFC 7662/7009 服务原语完成；全 workspace fmt/clippy/test 全绿；HS256 p99 实测 3.9µs、malformed fuzz 通过；**认证层匿名 0 分配达成**（auth 增量 +2.00 == 空中间件 +2.00）；整请求分配税 17.86 另列为独立性能目标；mTLS 握手 e2e 通过；**600s soak 0 失败、chaos 3/3**；OIDC 本地 HTTPS IdP e2e（含 at_hash）通过；**`release/2.5` 分支已创建**；威胁模型见 `auth-threat-model.md`；外部 IdP/WebAuthn 浏览器 e2e 与 ≥1h 浸泡待补（见 `docs/design/auth-conformance.md`、`auth-migration-2.5.md`）。

---

## 7. 明确不做（YAGNI）

1. 不做 IdP/OP、SAML 服务端、LDAP/AD、SCIM 用户同步。
2. 不做用户 CRUD/注册/找回密码/社交登录 UI/验证码；OAuth2 协议客户端见 `rust-webx-oauth2`。
3. 不做自定义密码学（JWT/HMAC/Argon2/WebAuthn 验签均用成熟 crate）。
4. 不做"零配置魔法"：默认值必须可解释、可退出。
5. 不为认证引入第二套 DI/中间件/错误模型。
6. 不把令牌/会话状态放进进程级可变全局。
7. 不做 benchmark 驱动的语义特化。

---

## 8. 风险登记与翻案条件

| 风险 | 触发信号 | 应对 |
|---|---|---|
| 2.5 体量过大导致延期 | M1/M2 超期 ≥2 周 | M1/M2 先行发布 2.5.0，M3/M4 走 2.5.x；不分拆回 2.2–2.4 |
| 与 2.2–2.4 并行改动冲突 | 共享文件（server.rs/config.rs/problem_response.rs）频繁冲突 | 先合 additive 底层；认证实现集中在新文件；每周 rebase |
| 概念膨胀（AuthService/端口过多） | 核心认证概念 >8 | 合并端口；优先复用 `IAuthenticationScheme` |
| JWKS 引入网络依赖与 SSRF 面 | 启动/运行出现外网失败 | 复用 `rust-webx-client` SSRF 守卫 + 超时 + 熔断；静态公钥模式 |
| Cookie/CSRF 误配置导致 CSRF | 渗透测试发现写请求无 token 通过 | 启动守卫 + 默认 SameSite=Lax + Origin 复核 |
| 刷新令牌实现错误导致会话劫持 | 重用未撤销族 | 契约测试 + 状态机 + 审计告警 |
| 口令限流被用于锁定 DoS | 攻击者锁死账号 | 指数退避不永久锁 + IP 维度 + 管理员清除 |
| 默认值变更破坏兼容 | 升级后登录失败 | CHANGELOG + 弃用 ≥2 minor + N-2 测试 |
| 性能预算不达标 | p99 超预算 | 缓存/预分配/降级为 feature；不牺牲语义 |

**翻案条件总则**：任何裁决在触发信号出现时重新评审；本文与宪法冲突时以宪法为准。

---

## 附录 A：标准与参考

- RFC 7515/7519/8725（JWS/JWT/JWT BCP）
- RFC 6749/6750/7636/9700（OAuth 2.0 与安全 BCP、PKCE）
- OpenID Connect Core 1.0
- RFC 7662/7009（内省/撤销）、RFC 8414（授权服务器元数据）、RFC 9449（DPoP）
- W3C WebAuthn Level 3、FIDO2
- RFC 6238（TOTP）、RFC 7617（Basic）、RFC 6265bis（Cookie）
- NIST SP 800-63B（AAL）、OWASP ASVS 4.0 / OWASP Top 10
- 本仓库：`docs/design/framework-constitution.md`、`docs/rust-webx/09-auth-security/`

## 附录 B：ADR 一览（全部目标 2.5 分支）

| ID | 决策 | 状态 |
|---|---|---|
| ADR-AUTH-01 | scheme 链保留 first-match；不做 per-route（M3 复评） | 提议 |
| ADR-AUTH-02 | 无效 token = 匿名保留，新增 `Reject` 严格模式 | 提议 |
| ADR-AUTH-03 | JWT 校验配置化 + `sub` 非空 + 大小/leeway 有界 | 提议 |
| ADR-AUTH-04 | 非对称/JWKS 进 host（feature），OIDC 独立 crate | 提议 |
| ADR-AUTH-05 | Cookie 会话用不透明 id + `ISessionRepository`，不复用 JWT | 提议 |
| ADR-AUTH-06 | CSRF 双提交 + SameSite + Origin；无 CSRF 的 Cookie scheme 启动失败 | 提议 |
| ADR-AUTH-07 | Refresh 不透明令牌 + 哈希存储 + 族轮换 + 重用撤销 | 提议 |
| ADR-AUTH-08 | 口令 NIST 策略 + dummy verify + 双维度限流 + Argon2 闸门 | 提议 |
| ADR-AUTH-09 | 认证指标复用 `IMeter`，不引入第二套遥测 | 提议 |
| ADR-AUTH-10 | `jwt_secret` shim 2.5 弃用、下一 major 删除 | 提议 |
