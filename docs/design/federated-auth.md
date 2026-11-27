# 联邦认证与机器身份（F1–F4）

> 来源：下游 RXStudio `docs/99-feedback/`（2026-11-26/27）的责任边界结论，经 rust-webx 2.9.3
> 源码逐条核验后重新定级。
>
> 本页是 F1–F4 的**唯一权威页**：核验结论、API 形状、安全要求与落地轮次。使用手册只写
> "怎么用 + 指向本页"，不复制这里的判定与约束。已覆盖的条目只保留一行结论，细节回到
> [upstream-gaps.md](upstream-gaps.md)、[known-boundaries.md](known-boundaries.md) 与代码。

## 1. 核验结论（2026-11-27，基线 2.9.3）

| 反馈条目 | 下游原判定 | 2.9.3 实证 | 结论 |
|---|---|---|---|
| 03 §F1 非对称签发 + JWKS 发布 + 轮换 | 上游缺口 | 验签侧已支持（`auth_jwks.rs`：RS256/ES256/PS256、`kid`、single-flight、SSRF 守卫）；`JwtTokenIssuer` 只签名 HS256，框架不产出 JWKS 文档 | **缺失（签发侧）** |
| 03 §F2 资源服务器多 issuer / 多 audience / scope | 上游缺口 | `Jwt.Issuer`/`Jwt.Audience` 为单值；无 `scope` 消费；`IdentityUser` 无 `client_id` | **部分缺失** |
| 03 §F3 OAuth2 client credentials（服务账号取 token） | 上游缺口 | `rust-webx-oauth2` 只有授权码 + PKCE 社交登录 | **缺失** |
| 03 §F4 外部 PDP 授权客户端端口 | 上游缺口 | 只有进程内 `IDynamicAuthorizer`；无 `IAuthorizationClient` | **缺失** |
| 03 §F5 API key / 服务认证 scheme | 可选 | `ApiKeyScheme` + `IApiKeyValidator` 已发布 | 已覆盖 |
| 01 §1–§21 认证/授权/限流/事务/密钥反馈 | 上游缺口 | 2.9.1–2.9.3 已交付 17 项（`RequestContext` 授权阶段、`RateLimitRoute::Pattern`、`Composite`/`token_bucket`、typed 429/409、`use_transaction_generic`、`upsert_increment`、`ctx.edit`+`detect_changes`、`CursorPage::from_entities`、`add_dbcontext_with_options`、`From<SecretError>`、`from_env_names`、`rotate_outcome`、`consume_matched`、CSPRNG TOTP、`acquire_with_timeout`） | 已覆盖 17 / 部分 4（见下） |
| 01 §10/§16/§20/§21 | 部分 | §10 rust-ef `execute_in_transaction` 与 DML/ChangeTracker 文档仍缺；§16 MFA 的 EF 适配器与参考 schema 仍缺；§20 `revoke_session_and_family` 配对撤销仍缺；§21 durable starter 只有 `publish = false` 参考实现 | 部分（§20 归本仓，见 §7；其余归 rust-ef 或下游） |
| 01 §22 `EFError` 瞬态分类 | rust-ef | `From<EFError> for AppError` 属 rust-ef；残余 `Unknown`/`InvalidArgument → Query` 见 [upstream-gaps.md](upstream-gaps.md) §rust-ef 残留 | 非本仓 |
| 01 §23–§29 分页/排序/API 收敛 | design | 2.9.1 已实施：`PagedResult { items, total }`、`order_by_field`/`then_by_field`、`to_page(&PagedRequest)`、六动词组合根 | 已覆盖；§25 部分（`column!` 仍返回 `&'static str`，`*_raw`/投影别名 `order_by(&str)` 仍在，见 §7） |
| 01 §31–§38 `rust-core` 与依赖收敛 | design | `rust-core` 0.1 已发布；core/umbrella 不链接 rust-ef | 已覆盖 |
| 04 S-01…S-20 表结构/迁移/约束 | — | rust-ef 2.9.1 已有对应原语：`has_foreign_key`/`on_delete`（S-01）、`has_max_length`/`is_fixed_length`（S-02）、`ctx.migrate` + `__ef_migrations_history` + `has_drift`（S-03）、`IndexBuilder::is_unique().has_filter(..)`（S-04）、`has_key`（S-05/06/07）、`has_check`/`has_check_expr`（S-11）、`is_concurrency_token`（S-13）；其余是产品语义与文档决策 | 非本仓（不是 webx 缺失） |

> 只有 F1–F4 落在本仓。它们共享一条判定：**协议标准 + 跨服务复用 + 安全原语 + 无业务语义**。
> 用户/组织/角色/权限目录/SCIM/审计导出仍是下游产品能力，框架不建第二套模型。

## 2. F1 非对称签发 + JWKS 发布

### 目标

一个服务用自己的私钥签发、用同一份公钥集合验签，并把公钥发布成标准 JWKS：所有资源服务器
只持公钥即可验签，HS256 共享密钥不再是跨服务必需品。

### 配置（`Jwt.Signing`，PascalCase，加性）

| 键 | 必填 | 语义 |
|---|---|---|
| `Algorithm` | 是 | `RS256`/`ES256`/`PS256`；`HS256` 留给 `Jwt.Secret`，在此显式拒绝 |
| `KeyId` | 是 | JWT header `kid`，同时是 JWKS 条目的 `kid`；空白即启动失败 |
| `PrivateKeyFile` | 是 | PKCS#8/PKCS#1/SEC1 PEM 私钥路径；Unix 上必须 `chmod 600`（与 `Jwt.SecretFile` 同一守卫） |
| `PublicKeyFile` | 否 | 仅用于核对；缺省时从私钥派生公钥 |

### API（`rust-webx-host`）

```rust
pub struct JwtSigningKey { /* kid + algorithm + EncodingKey + 公开 JWK */ }

impl JwtSigningKey {
    pub fn from_pem(algorithm, kid: impl Into<String>, private_pem: &[u8]) -> Result<Self>;
    pub fn key_id(&self) -> &str;
    pub fn algorithm(&self) -> JwtAlgorithm;
    pub fn public_jwk(&self) -> Jwk;
}

pub struct JwtKeySet { /* active + retired（仅公钥） */ }

impl JwtKeySet {
    pub fn from_section(
        signing: &JwtSigningSection,
        retired: &[JwtRetiredSigningKeySection],
    ) -> Result<Self>;
    pub fn active(&self) -> &JwtSigningKey;
    pub fn jwks(&self) -> JwkSet;                       // RFC 7517，含 active + retired
    pub fn verification_resolver(&self) -> Arc<dyn IJwtKeyResolver>;
}

impl JwtTokenIssuer {
    pub fn from_key_set(keys: Arc<JwtKeySet>, default_lifetime: Duration) -> Self;
}
```

- `JwkSet`/`Jwk` 增加 `Serialize` 与 `use = "sig"` 输出，保证 `/​.well-known/jwks.json` 只是
  `serde_json::to_string(&key_set.jwks())`。
- 装配规则（`use_authentication()`）：
  1. 有 `Jwt.Signing` → 启动期加载私钥（失败即启动失败），注册 `Arc<JwtKeySet>` 单例与
     非对称 `ITokenIssuer`；
  2. `Jwt.Algorithms` 必须包含签名算法，否则启动失败（不做静默加宽）；
  3. 验签解析器按组合构建：只有 `Jwt.Signing` → 本地 key set 闭环；只有 `Jwt.Jwks`/
     `Jwt.JwksSources` → 远端信任锚；两者都有 → `LocalFirstKeyResolver`（本地 `kid` 优先，
     未知 `kid` 才落到远端），因此合作方 IdP 故障不会连带拒绝本机签发 token，而远端多源仍保持
     "同一 `kid` 命中多源 → 503" 的 fail-closed 语义；
  4. 多 issuer 部署下本机自签需要 `Jwt.Signing.Issuer` 显式指定"我是哪一个 issuer"
     （必须出现在 `Jwt.Issuers`）；缺省时启动告警，本机 token 不带 `iss`、会被自己的多 issuer
     校验器拒绝；
  5. `Jwt.Signing` 缺省时行为与 2.9.3 完全一致（HS256 默认路径零变化）。
- 轮换 = 滚动部署：新 key 作为 active、旧 key 作为 `RetiredSigningKeys` 公钥继续验签；
  JWKS 同时发布两者，旧 token 在 `exp` 前自然退场，之后删除 retired 条目。不做运行期
  热轮换（无锁、无缓存失效面）。

### 安全要求

- 私钥只从文件读、不落日志、不进 `Debug`（`JwtSigningKey` 手写 `Debug` 打 `<redacted>`）。
- RSA 最小 2048 位、EC 只接受 P-256；算法与 `kid` 不符、公私钥不匹配 → 启动失败。
- 启动期自检：用私钥签一份短命 token，再用 `verification_resolver()` 验回；失败即启动失败。
- 发布文档只含公钥参数（RSA `n/e`、EC `x/y`），永不含 `d`。

## 3. F2 资源服务器多 issuer / 多 audience / scope

### 目标

同一个服务同时信任多个 IdP（自有 + 企业 IdP），并执行 OAuth 2.0 资源服务器语义：
`iss`/`aud` 白名单、`scope` → 权限映射。

### 形状（加性，单值仍有效）

- `Jwt.Issuers: Vec<String>` / `Jwt.Audiences: Vec<String>`：与现有 `Issuer`/`Audience` 互斥
  合并（单值等价于单元素表）；`Jwt.JwksSources: Vec<{Issuer,Url,…}>`：每个来源一个 JWKS，并按
  `Issuer` 绑定（与单份 `Jwt.Jwks` 互斥）。
- `Jwt.Signing.Issuer: Option<String>`：多 issuer 部署下本机自签所用的 `iss`，必须 ∈ `Jwt.Issuers`。
- `Jwt.RequireScope: bool`（默认 false）+ `ScopeClaim`（默认 `scope`）+ `ScopeSeparator`
  （默认空格）：开启后每个受保护请求必须携带至少一个 scope（缺失 → 403，不降级为匿名）。
- `Jwt.ScopeAsPermissions: bool`（默认 **false**）：默认只解析 scope 做 `RequireScope` 判定，
  **不**并入 `permissions()`。开启后才与 `permissions` claim 取并集、去重。理由：外部 IdP 的
  `scope` 不能被静默当成应用声明的权限，否则换一个 IdP 就可能放大授权面。
- `IdentityUser` 增加 `client_id: String`（来自 `client_id`/`azp`，缺省空），供下游按客户端审计。

### 边界

- 框架只做**集合校验与映射**，不做角色/组织/权限目录；claim 名字可配置但没有业务语义。
- 多 issuer 的 `kid` 空间按 issuer 隔离：同一 `kid` 在不同 issuer 下是不同 key，解析不得跨 issuer 串用
  （同一 `kid` 命中多个来源 → 503）。
- **来源绑定（运行期）**：配置 `Jwt.JwksSources` 时，每个来源发布的 key 在验签后绑定到该来源的
  `Issuer` —— token 的 `iss` 必须等于实际提供 key 的来源 issuer，否则按无效 token 处理
  （拒绝或匿名，取决于 `InvalidTokenBehavior`）。绑定随解析结果原子返回（`resolve_bound`），
  不依赖跨请求共享状态，因此并发不能让它被跳过。
- **多 issuer 只允许非对称 + 每来源一份 JWKS**：`Jwt.Jwks`（单份共享 JWKS）与 `Jwt.Issuers`
  多值同配 → 启动失败；`Jwt.Algorithms` 含 `HS256` 且 `Issuers` 多值 → 启动失败（共享 HMAC
  secret 无法表达 per-issuer 信任）。需要过渡期"本机 HS256 + 合作方 asym"时，先用单一 issuer
  或迁移到 `Jwt.Signing`。

## 4. F3 OAuth2 client credentials

### 目标

服务到服务调用不再借用户 token 或手写 HTTP：一个 `IClientCredentials` 端口取机器令牌，
框架处理缓存、提前刷新、失效重取与并发单飞。

```rust
#[async_trait]
pub trait IClientCredentials: Send + Sync {
    /// 取一个可用于 `audience` 的机器令牌（缓存命中不发请求）。
    async fn token(&self, scope: &[&str]) -> Result<ServiceToken>;
}
pub struct ServiceToken { pub access_token: String, pub expires_at_unix_secs: u64, pub scope: String }
```

- 实现：`OAuth2ClientCredentials`（`client_secret_basic`/`client_secret_post` 二选一、
  `scope` 空格拼接、`audience` 可选参数），底层走 `rust-webx-client` 的 `IHttpClient`。
- 缓存：按 `(scope 排序串, audience)` 分桶；`expires_in - skew` 到期；**并发单飞**（一个
  在途请求，其余等待同一结果）；失败不缓存；缓存有界（`DEFAULT_MAX_CACHE_ENTRIES = 256`，FIFO
  淘汰，诊断用 `cached_entries()`），避免调用方传入的任意 scope 造成无界增长。
- 每个 token 请求有端到端 deadline（`execute` + 读 body + 解析整体超时），即使注入的
  `IHttpClient` 忽略 `OutboundRequest::timeout` 也不会挂死单飞等待者。
- secret 只从调用方传入（推荐 `ISecretStore`），不读环境变量；`Debug` 打 `<redacted>`。
- 失败语义：token 端点 4xx → `Error::di`（配置/凭据错误，不重试）；5xx/超时 → 可重试 503；
  绝不降级为无令牌请求。

## 5. F4 外部 PDP 授权客户端

```rust
#[async_trait]
pub trait IAuthorizationClient: Send + Sync {
    async fn check(&self, check: &AccessCheck) -> Result<AccessDecision>;
    async fn check_batch(&self, checks: &[AccessCheck]) -> Result<Vec<AccessDecision>>;
}
```

- `AccessCheck { subject, tenant, permission, resource, context }`；`AccessDecision::{Allow, Deny}`。
- 适配器：HTTP PDP（POST JSON，超时、批量上限、响应形状校验），注册进 DI；
  `HostBuilder` 只暴露 `use_authorization_client(Arc<dyn IAuthorizationClient>)`。
- 决策缓存：进程内有界 TTL 缓存（键含 tenant/subject/permission/resource，`max_cache_entries`
  上界，默认关闭）；只缓存 `Allow`，`Deny` 与错误从不缓存。
  **边界**：命中缓存的 `Allow` 在 TTL 内不会再询问 PDP —— 撤权/禁用最迟 `ttl`（≤300s）后生效，
  需要立即生效时调用 `invalidate()` 或关闭缓存；PDP 故障期间，已缓存的 `Allow` 仍会放行（这是
  TTL 缓存的固有语义，不是 fail-closed 的例外）。未命中时的超时/不可达仍 **fail-closed**
  （503，不降级为允许）。
- 不定义权限模型/角色/组织；PDP 决策实现是下游产品。

## 6. 落地轮次与验收

| 轮次 | 并行工作流 | 验收 |
|---|---|---|
| 第 1 轮（已完成） | F1 非对称签发 + JWKS 发布（24 测试）‖ F3 client credentials（20 测试 + DI 端口）‖ 反馈逐条独立核验（38 项 + S-01…S-20 归属）‖ `IHttpClient` 可伪造响应（6 测试） | `cargo test -p rust-webx-host --test auth_signing_test`、`-p rust-webx-oauth2`、`-p rust-webx-client`；启动负例（错算法/短 RSA/权限 600/公私钥不匹配/重复 retired kid） |
| 第 2 轮（已完成） | F2 多 issuer/audience/scope/client_id（21 测试）‖ F4 PDP 端口 + HTTP 适配器 + 决策缓存（10 集成 + 8 单元测试）‖ `OutboundRequest` 读访问器（3 测试） | 多 issuer 正/负例、跨来源同 `kid` → 503、scope 403、PDP fail-closed/缓存/批量上限 |
| 第 3 轮（已完成） | 手册 4 页 + 新页 `client-credentials.md` + README/CHANGELOG(Unreleased) + 索引生成 ‖ 伞 crate 重导出 + 模块文档纠偏 | `check-docs: ok`、`check-docs-drift: ok`、`clippy -D warnings`、`test --workspace`、`doc` |
| 第 4 轮（已完成） | §20 `revoke_session_and_family` + `ISessionStore::revoke_by_refresh_family`（session cache 透传）‖ F3/F4 删除私有传输端口，收敛回唯一 `IHttpClient` 缝 | `refresh_token_test` 14、`authorization_client_test` 12、`-p rust-webx-oauth2` 32、`-p rust-webx-core` 全套；全量 fmt/clippy/test(all-features)/doc 全绿 |
| 第 5 轮（已完成） | 对抗性安全复核（Critical 0 / High 1）→ 修 H-1 原子 `resolve_bound`（删除 provenance 共享 map）‖ 修 M-1/M-2/M-4/M-5/M-6/M-7/M-8 + L-1/L-3/L-4 ‖ F1 可运行示例 `asymmetric_jwt.rs` | `auth_multi_issuer_test` 27、`auth_signing_test` 27、`auth_jwks` 单测、`authorization_client_test` 13、`refresh_token_test` 15、`-p rust-webx-oauth2` 34；示例 `cargo check/clippy` 干净 |

每轮都必须：`unsafe_code` 禁令不变、每请求零新增分配（启动期一次性）、失败全部 fail-closed、
公开 API 只增不减（2.x 政策见 [compatibility.md](compatibility.md)）。

## 7. 合并门禁、已知不做与部分覆盖账

### 7.1 合并门禁（防"配置键可解析但没有消费者"）

任何新增配置键，必须在**同一变更**里满足三条，否则视为伪覆盖：

1. 有消费者：例如 `Jwt.Signing` 一旦能被 `JwtSection` 反序列化，`JwtValidation` 与
   `use_authentication()` 必须在同一批改动里接受它；只加字段不加消费者 = 静默降级路径。
2. 有正例 + 负例：`Jwt.Signing` + `Algorithms:["RS256"]` **不配** `Jwt.Jwks` 必须启动成功并
   自签自验闭环；配错算法/短 RSA/公私钥不匹配必须启动失败。
3. 有手册指针：使用页写"怎么用 + 指向本页"，不复制判定表。

### 7.2 本仓内的部分覆盖（后续轮次处理）

| 项 | 现状 | 归本仓的收尾动作 |
|---|---|---|
| §20 会话 ↔ refresh family 配对撤销 | `RotateOutcome::Reused { family_id }` 已给；第 4 轮补齐 `ISessionStore::revoke_by_refresh_family` + 显式能力 `supports_refresh_family_revocation()`（第 5 轮修正：不再用 501 猜能力）与 `revoke_session_and_family` helper（先撤 family、再撤会话；`CachedSessionStore` 透传并清负缓存） | 已完成（第 4/5 轮） |
| §25 排序强类型化收尾 | `order_by_field`/`then_by_field` 已强类型；`column!` 仍返回 `&'static str`，`*_raw` 与投影别名 `order_by(&str)` 仍在 | rust-ef 侧（不在本仓）；本仓手册只写强类型入口，raw 入口标注"仅白名单" |
| §23/§34 keyset 契约 | `PagedRequest` 越界 400、无归一状态；`KeysetRequest` 仍 `from_parts`/静默 clamp | rust-core 侧（不在本仓）；本仓手册明确两种分页的越界语义差异，不承诺统一 |
| §21/§35 durable starter | `examples/adapters/*/ef` 全部 `publish = false`，下游只能复制 | 本仓只需保证参考适配器的列宽/单位有书面契约（见 `integrations.md` 端口表）；发布新 starter 属路线决策 |

### 7.3 已知不做（避免下一轮重复上报）

| 不做项 | 理由 | 替代路径 |
|---|---|---|
| F1 运行期热轮换 | 无锁、无缓存失效面的静态 key set 足够覆盖滚动发布；热轮换会把配置漂移引入运行期 | 新 key 作 active、旧 key 作 `RetiredSigningKeys` 公钥，滚动部署 |
| F2 角色/组织/权限目录 | 产品数据模型，框架不建第二套权限库 | `#[authorize]` 声明 + `IDynamicAuthorizer` / F4 PDP 客户端 |
| F3 自动注入 `Authorization: Bearer` 到任意出站请求 | 隐式注入会把"用谁的令牌"变成 ambient 状态，违背显式依赖 | 调用方显式 `token()` 后自行加头；`rust-webx-client` 保持无全局凭据 |
| F4 PDP 决策实现与角色模型 | 决策逻辑属于下游 | `IAuthorizationClient` 端口 + 缓存/fail-closed；实现由 Arc 等产品提供 |
| 多 issuer 下的 key 归属 | JWT 验签必须先解 header 选 key，`iss` 只有在验签后才可信 | `JwksSources` 运行期：同一 `kid` 命中多源 → 503；验签后把 key 原子绑定到来源 issuer（`resolve_bound`），`iss` 不符即拒绝；启动期不抓取 JWKS（避免网络依赖） |
| 共享 `Jwt.Jwks` 或多 issuer + `HS256` | 单份 JWKS / 共享 HMAC secret 无法表达 per-issuer 信任 | 启动期拒绝，指引改用 `Jwt.JwksSources`（每 issuer 一份 JWKS）与非对称签名 |

### 7.4 安全硬化（已落地）

- `JwtSection` 手写 `Debug`，`secret` 打 `<redacted>`（不再随 `#[derive(Debug)]` 输出明文）。
- `JwtSigningKey`/`JwtKeySet` 的 `Debug` 只输出 `kid`/算法，密钥材料永不打印。
- `Jwt.Signing` 来源绑定原子化（`resolve_bound`）：不再有跨请求共享的 provenance 状态，
  并发无法让绑定被跳过；每 token 零额外分配。
- `iss`/`aud`/`nbf` 是 issuer 保留 claim，`TokenClaims::extra` 无法注入。
- `Jwt.RetiredSigningKeys` 无 `Jwt.Signing` → 启动失败；retired 公钥同样受 RSA ≥2048 约束；
  JWKS 文档内重复 `(kid, alg)` → 503，多 key 文档的空 `kid` 条目被跳过。
- F3 token 缓存有界；F3/F4 请求端到端 deadline；§20 用显式能力声明而不是状态码猜能力；
  `CachedSessionStore` 的负缓存插入带 revision 守卫，本地 revoke 不会被并发读掩盖。

### 7.5 复核遗留（Low，已评估未修）

| 项 | 现状/理由 |
|---|---|
| 私钥文件权限检查的 TOCTOU 与 Windows 无 ACL 检查 | Unix 上 `metadata` + `read` 两次打开之间可被本机有写权限者换文件；Windows 分支不检查（与 `Jwt.SecretFile` 同型）。威胁模型是"已能在部署目录写文件的人"，属运维边界；文档已声明。 |
| F3 `expires_in` 缺失/负数按 300s | token 端点受信；文档已写明保守假设。 |
| `use_authorization_client` 重复注册 last-wins | 属应用装配错误；rust-dix 的 `instance` 语义与其它 `use_*` 一致，未额外加检测。 |
| 多来源未知 `kid` 的最坏耗时 ≈ N × `TimeoutMs` | 顺序查询是有意设计（确定性失败、避免并行扇出）；每来源的未知 `kid` 刷新有 5s 门控，来源数量应保持个位数（手册已写）。 |
| 注入的 `IHttpClient` 若自行跟随重定向 | F3/F4 只接受注入 client，不控制其重定向策略；原生 `HttpClient` 不跟随重定向。手册要求注入实现继承 SSRF/不跟随策略。 |

