# 公共 API 收敛清单（3.0 提案）

> 目标：一个概念一个入口；同义入口删除；附加维度（顺序、超时、容量、容量上限）用 spec/Options
> 承载，而不是复制方法；语义确实不同的近邻保留，但必须有**一句可背诵的判别规则**。
>
> 固定不改：`PagedRequest` / `PagedResult`（对外契约，见 `pagination-model.md`）。

## 判定口径

| 判定 | 含义 | 动作 |
|---|---|---|
| 同义 | 两个入口产生同一种结果，靠命名/参数区分 | 删一个，保留覆盖面更大的 |
| 复制 | `X()` 与 `X_with(...)` 是同一方法 + 一个参数 | 合并为 `X(Options)`（`Default` + builder） |
| 维度 | 顺序、超时、条件等附加维度 | 进 spec/Options，方法只留一个 |
| 保留 | 语义不同（typed/erased、panic/Result、可跳页/续读） | 不动代码，写死判别规则并禁止第三套 |

## 清单

| # | 现状 | 判定 | 收敛后 | 批次 |
|---|---|---|---|---|
| 1 | `PagedRequest` / `PagedResult` | 固定 | 不动；协议见 `pagination-model.md` | — |
| 2 | `KeysetRequest` / `CursorPage` | 保留（独立契约） | 仅用于追加流/深翻页；`from_entities` 已补胶水；判别规则一句话 | — |
| 3 | `OperationResult::{ok, ok_with_status, success, success_with_status}` | 同义 | `success(message)` / `failed(status, message)` + `with_status` / `with_message` / `with_result` | A |
| 4 | `Error::Message`（500，裸文本，13 处） | 同义 | 删除，统一 `Error::Internal`（500、`public_detail()=None`）；`State` 仅用于状态机 | A |
| 5 | `Error::Http(String)`（400） | 同义 | 删除，统一 `Error::Status(400, _)` / `Validation` | A |
| 6 | `use_session_store` + `use_session_store_cached` | 复制 | **已完成（3.0）**：只留 `use_session_store`；缓存用 `CachedSessionStore::new(store, ttl)` 显式装饰 | A |
| 7 | `add_localization` + `add_localization_with(Options)` | 复制 | **已完成（3.0）**：`use_localization(LocalizationOptions)` | A |
| 8 | `add_memory_cache` + `add_memory_cache_with(max_entries)` | 复制 | **已完成（3.0）**：`add_memory_cache(CacheOptions)`（`Default` + `.max_entries(n)`） | A |
| 9 | `add_options` / `use_options` / `add_options_monitor` | 复制 + 命名混 | `add_options::<T>(section)` 同时注册 `IOptions<T>`/`IOptionsMonitor<T>`；`use_options(value)` 只用于预构造实例 | B |
| 10 | `add_health_check` / `_async` / `_async_with_timeout`（+ registry `register`/`register_async`） | 复制 + 维度 | `add_health_check(HealthCheckSpec::new(name, check).timeout(d))`；sync 闭包经 `IntoHealthCheck` 适配 | B |
| 11 | `use_middleware` / `_at` / `_with` / `_with_at` | 维度 | `use_middleware(MiddlewareSpec::<M>::default().at(order).factory(f))` | B |
| 12 | `add_background_service` / `_with` / `add_hosted_service_with` / `_ordered` / `_if` | 维度 + 命名混 | `add_background_service(spec)`（spec 承载 order/if/构造）；`PeriodicBackgroundService` 仍是 trait 变体，不是注册变体 | B |
| 13 | `use_alt_svc(value)` / `use_h3_alt_svc(port)` | 同义 | **已完成（3.0）**：只留 `use_alt_svc(impl Into<String>)` | A |
| 14 | `build` / `try_build` | 保留（panic vs Result） | 生产装配只用 `try_build`；`build` 仅测试/示例 | 规则 |
| 15 | `Mediator::send` / `send_boxed` | 保留（typed vs erased） | `send_boxed` 只给 `Arc<dyn IMediator>` 类型擦除调用 | 规则 |
| 16 | `IRequest<Response>` 自控响应 vs `ActionResult<T>` vs `OperationResult<T>` | 保留（语义不同） | 写死三选一：DTO → `ActionResult<T>`；命令型业务信封 → `OperationResult`；文件/非 JSON/自定义状态 → `IRequest<Response>` | 规则 |
| 17 | `use_rate_limiter` / `use_rate_limit_policy` | 保留（全局桶 vs 策略） | 全局桶用 limiter；按用户/租户/API key 用 policy | 规则 |
| 18 | `add_authentication` / `add_authentication_scheme` / `use_auth_scheme_for` | 命名混 | 动词规则：`use_*` = 替换端口/启用；`add_*` = 叠加中间件或基础设施；逐个改名对齐 | C |
| 19 | `#[fromquery]` 与 `#[request(queryall)]` 两种查询绑定 | 同义（两种写法） | 合并为一种：字段级 `#[fromquery]` 为默认；`queryall` 作为结构级开关而不是第二套绑定 | C |
| 20 | `#[validate]` 与 `ProblemDetails::validation(...)` | 维度（声明式 vs 手工） | 字段级/格式用 `#[validate]`；跨字段/查库/业务规则用 `ProblemDetails`；不新增第三套 | 规则 |

## 批次日与验收

- **A（同义删除，低风险）**：#3–#8、#13。只删入口与调用点改写，**能力零丢失**；
  验收：各 crate 单测 + `cargo semver-checks` 记录移除例外 + 文档/示例/下游编译。
- **B（spec 化注册）**：#9–#12。新增 `HealthCheckSpec` / `MiddlewareSpec` / `CacheOptions` /
  `BackgroundServiceSpec`，旧方法在内移除；验收：现有注册写法在迁移文档里一一对应，
  顺序/超时/条件/容量行为不变（host 单测 + 集成测试）。
- **C（宏与命名）**：#18、#19。绑定合并涉及宏展开与 OpenAPI 元数据，先写迁移样例再动代码；
  验收：`api_stability`、绑定测试、OpenAPI 生成一致。
- **D**：#14–#17、#20 只写规则（不删代码），进 dev-guide 与插件规则，避免"第二套"再长出来。

## 与插件/文档的联动

rust-xazgo 插件的 `skills/*.md`、`lib/catalog/*.js`、`lib/audit.js`（如 `EF-WEBX-FEATURE`、
`X-SHARED-MODULES`）、`lib/prompt.js` 与本清单同批更新：一个概念只留一个入口，规则表只写一处。

## 风险

- 破坏性面大：A 批虽小，但 `OperationResult` 与 `Error` 变体被下游大量引用，必须给迁移表。
- `KeysetRequest`/`CursorPage` 若被下游用于"可跳页"场景，保留期需在文档里明确判别规则，
  避免同一端点两种契约混用。
- B 批 spec 类型会与 rust-dix 的 `instance`/Options 模式重复，必须复用而不是再造注册语义。
