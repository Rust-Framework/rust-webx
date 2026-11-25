# 公共 API 收敛清单（2.9.1 提案）

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
| 3 | `OperationResult::{ok, ok_with_status, success, success_with_status}` | 同义 | **已完成（2.9.1）**：`success(message)` / `failed(status, message)` + `with_status` / `with_message` / `with_result`（rust-core 0.1） | A |
| 4 | `Error::Message`（500，裸文本，13 处） | 同义 | **已完成（2.9.1）**：enum 删除，统一 `Error::internal(operator)`（500、`public_detail()=None`）；`Error::state` 仅用于状态机 | A |
| 5 | `Error::Http(String)`（400） | 同义 | **已完成（2.9.1）**：enum 删除，统一 `Error::status(400, _)` / `Error::validation(..)`（`app-error-migration.md`） | A |
| 6 | `use_session_store` + `use_session_store_cached` | 复制 | **已完成（2.9.1）**：只留 `use_session_store`；缓存用 `CachedSessionStore::new(store, ttl)` 显式装饰 | A |
| 7 | `add_localization` + `add_localization_with(Options)` | 复制 | **已完成（2.9.1）**：`use_localization(LocalizationOptions)` | A |
| 8 | `add_memory_cache` + `add_memory_cache_with(max_entries)` | 复制 | **已完成（2.9.1）**：`add_memory_cache(CacheOptions)`（`Default` + `.max_entries(n)`） | A |
| 9 | `add_options` / `use_options` / `add_options_monitor` | 复制 + 命名混 | **已完成（2.9.1）**：`add_options::<T>(section)` / `add_options_with::<T>(section, f)` 同时注册 `Arc<T>`、`Arc<OptionsMonitor<T>>` 与 `Arc<dyn IOptionsMonitor<T>>`，`add_options_monitor` 已删除；`HostAppBuilder::use_options` 改名为 `options`，驼峰 `useOptions` 删除（见下文「组合根动词与槽位」） | B |
| 10 | `add_health_check` / `_async` / `_async_with_timeout`（+ registry `register`/`register_async`） | 复制 + 维度 | **已完成（2.9.1）**：`add_health_check(HealthCheckSpec::sync/async_check(..).timeout(d))`；registry 保留底层原语 | B |
| 11 | `use_middleware` / `_at` / `_with` / `_with_at` | 维度 | **已完成（2.9.1）**：`use_middleware(MiddlewareSpec::new::<M>().at(slot))` / `MiddlewareSpec::factory(f)` | B |
| 12 | `add_background_service` / `_with` / `add_hosted_service_with` / `_ordered` / `_if` | 维度 + 命名混 | **已完成（2.9.1）**：`add_background_service(BackgroundServiceSpec::new/with_factory(..).enabled(b))`、`add_hosted_service(HostedServiceSpec::new/with_factory(..).ordered(i).enabled(b))`；`PeriodicBackgroundService` 仍是 trait 变体，不是注册变体 | B |
| 13 | `use_alt_svc(value)` / `use_h3_alt_svc(port)` | 同义 | **已完成（2.9.1）**：只留 `use_alt_svc(impl Into<String>)` | A |
| 14 | `build` / `try_build` | 保留（panic vs Result） | 生产装配只用 `try_build`；`build` 仅测试/示例 | 规则 |
| 15 | `Mediator::send` / `send_boxed` | 保留（typed vs erased） | `send_boxed` 只给 `Arc<dyn IMediator>` 类型擦除调用 | 规则 |
| 16 | `IRequest<Response>` 自控响应 vs `ActionResult<T>` vs `OperationResult<T>` | 保留（语义不同） | 写死三选一：DTO → `ActionResult<T>`；命令型业务信封 → `OperationResult`；文件/非 JSON/自定义状态 → `IRequest<Response>` | 规则 |
| 17 | `use_rate_limiter` / `use_rate_limit_policy` | 保留（全局桶 vs 策略） | 全局桶用 limiter；按用户/租户/API key 用 policy | 规则 |
| 18 | `add_authentication` / `add_authentication_scheme` / `use_auth_scheme_for` | 命名混 | **已完成（2.9.1）**：`add_authentication` → `use_authentication()`；`add_authentication_scheme`（叠加显式 scheme）与 `use_auth_scheme_for`（前缀选择）保留 | C |
| 19 | `#[fromquery]` 与 `#[request(queryall)]` 两种查询绑定 | 同义（两种写法） | **已完成（2.9.1 规则）**：字段级 `#[fromquery]` 默认；`queryall` 是结构级开关，不是第二套绑定 | C |
| 20 | `#[validate]` 与 `ProblemDetails::validation(...)` | 维度（声明式 vs 手工） | 字段级/格式用 `#[validate]`；跨字段/查库/业务规则用 `ProblemDetails`；不新增第三套 | 规则 |

## 组合根动词与槽位

收敛后只有六个动词，每个概念一个入口：

| 动词 | 语义 | 份数 / 顺序 |
|---|---|---|
| `add_*` | 叠加：向同一槽追加一项（中间件、健康检查、后台任务、HTTP 客户端等） | 多份；顺序/槽位敏感 |
| `use_*` | 替换/启用端口或全局策略（`use_clock`、`use_meter`、`use_authentication`…） | 单份；后设置胜 |
| `register` / `register_with_options` | 直接编辑 DI 容器（逃生舱；后者是读 `AppOptions` 的桥） | 按调用顺序执行 |
| `configure(\|app\| app.options(..))` | 仅 `AppOptions`；`options` 是字段级唯一入口 | 按链顺序；后写覆盖先写 |
| `add_options` / `add_options_with` | 配置节绑定；后者在绑定后应用代码级覆盖，再注册三种注入形态 | 一个节一个 `T` |
| `watch_options_file` | 为已注册的 options 节加热更新（轮询文件 → 发布新快照） | 单份；替换整个快照 |

约束：

- 新增 builder 方法**必须**在 rustdoc 标注所属槽（`**Slot:**` 后接 `add_*` / `use_*` /
  `register` / `configure` / `add_options*` / `watch_options_file`），否则评审不通过。
- 同一能力不允许再长出第二套动词；`use_*` 与 `add_*` 的区别是「替换单份」与「叠加多份」，
  不是「新/旧」。
- `use_*` 的「单份」由 `instance` 落实：同一端口后一次调用覆盖前一次（`use_meter` /
  `use_session_store` 等把实例注册进 DI 的端口同样按调用顺序执行，没有「先注册者胜」的例外）；
  `build()` 尾部的框架默认实现一律用 `try_add_*`，只在应用没有提供时兜底，所以默认实现永远
  不会覆盖 `use_*` 实例。
- 历史遗留的叠加型 `use_middleware` / `use_http_transport` 见 #11 与传输后端清单；不要以它们为
  模板新增叠加语义的 `use_*`（叠加项一律用 `add_*`）。
- 2.9.1：`HostAppBuilder::use_options` 改名为 `options`；驼峰 `useOptions` 删除，不保留别名。
  唯一 `AppOptions` 入口是 `HostBuilder::configure(|app| app.options(|o| …))`。

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
