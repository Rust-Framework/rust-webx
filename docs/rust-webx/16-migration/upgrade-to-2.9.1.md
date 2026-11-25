# 从 2.9.0 迁移到 2.9.1（破坏性补丁）

> 2.9.1 是**破坏性补丁版（semver 例外）**：原计划的 3.0 契约迁移整体并入本补丁，不再有 3.0。
> 错误载体、分页契约、嵌套绑定、强类型排序、组合根动词与部分 HostBuilder 入口都变了，
> 依赖分层也从 `starters` 收敛为「框架 crate + 下游适配」。
> 本文只列**源码迁移动作**；逐条证据与设计见 `docs/design/app-error-migration.md`、
> `pagination-model.md`、`api-convergence.md`、`integrations.md`。

## 1. 错误载体：`webx::Error = rust_core::AppError`

`webx_core::Error` 现在是 `rust_core::AppError` 的别名，2.9 的 19 变体 enum 已删除。

| 2.9 | 2.9.1 |
|---|---|
| `Error::Internal(msg)` | `Error::internal(msg)`（operator-only detail） |
| `Error::Validation(msg)` | `Error::validation(msg)` |
| `Error::Status(code, msg)` | `Error::status(code, msg)` |
| `Error::Http(msg)` | `Error::bad_request(msg)` |
| `Error::Routing(msg)` | `Error::route_not_found(msg)` |
| `Error::Serialization(e)` | `Error::invalid_json(e)` |
| `Error::ResponseSerialization(e)` | `Error::response_serialization(e)` |
| `Error::TooManyRequests { retry_after_secs, detail }` | `Error::too_many_requests(retry_after_secs, detail)` |
| `Error::ConflictRetryable(msg)` | `Error::conflict_retryable(msg)` |
| `Error::Message(msg)` | `Error::internal(msg)` |
| `Error::Problem(p)` | `p.into()` / `ProblemDetails::to_error()` |

模式匹配改为访问器：

```rust
// before
match error {
    Error::Internal(detail) => log(detail),
    Error::Status(503, _) => retry(),
    Error::Problem(p) => write(p),
    _ => {}
}

// after
tracing::error!(operator = error.operator(), "internal failure");
if error.status_code() == 503 { retry(); }
let problem = ProblemDetails::from_app_error(&error, path, trace_id);
```

- `Result<T>` / `ActionResult<T>` 仍是 `rust_core::Result` 别名；
- `RdiError` / `EFError` 在各自 crate 边界实现 `From`，handler 直接 `?`；
- `Display` 只打印 operator/detail/code，不能用于客户端文案；用 `public_detail()`；
- `x-webx-*` 是保留扩展键：自定义 `IErrorResponseWriter` 必须用
  `webx_core::problem::extensions_for_wire`，`Retry-After` 用 `retry_after_of`。

## 2. 分页契约

```rust
// before (2.9)
let paging = PagedRequest::from_parts(page, page_size);
let page = PagedResult::from_request(items, total, &paging);

// after (2.9.1)
let paging = PagedRequest { page, page_size }; // binding 负责默认/400 校验
let page = query.to_page(&paging).await?;     // 直接返回 PagedResult<T>
```

- `PagedRequest` 只有 `page` / `page_size` 字段；无 `new/from_parts/clamped/Default`；
  `offset() -> u64`；
- `PagedResult<T>` 是扁平信封 `{success,status,message,items,total}`；
  `new(items, total)`；无 `page/page_size/total_pages` 回声，无 `from_request`；
- 列表请求推荐嵌套绑定：

```rust
#[derive(WebxRequestMeta)]
struct ListOrdersRequest {
    #[fromclaim] user: IdentityUser,
    #[fromquery] keyword: Option<String>,
    #[fromquery(nested)] paging: PagedRequest,
}
```

`page < 1` / `page_size ∉ 1..=100` 是 400 字段错误（不再静默夹紧），且
`page/page_size` 会进入 OpenAPI。

## 3. 强类型排序与 raw 入口

```rust
// before
query.order_by(Order::COLUMN_CREATED_AT).order_by_desc("id");

// after
query
    .order_by_field(Order::created_at)
    .then_by_field_desc(Order::id);
```

- `order_by_field*` 要求字段类型实现 `Sortable`；`Vec<u8>`、JSON、任意 struct 编译期拒绝；
- **字符串排序已删除**：`order_by(&str)` / `order_by_desc(&str)` 在 2.9.1 移除（无兼容别名）。
  确实需要原始列名拼写时，用显式 raw 入口 `order_by_raw(..)` / `order_by_desc_raw(..)`；
  `SelectQueryBuilder` 的别名排序保留，并同样新增 raw 拼写；
- 判别规则：**强类型字段优先**（`order_by_field*`）；只有列名来自运行期白名单映射时才用
  `*_raw`，并且列名必须来自白名单，不得直接拼接用户输入。

## 4. 组合根动词：`use_options` → `options`

`AppOptions` 字段级入口改名，驼峰别名删除（两者都在 2.9.1 移除，无兼容别名）：

| 2.9 | 2.9.1 |
|---|---|
| `.configure(\|app\| app.use_options(\|o\| …))` | `.configure(\|app\| app.options(\|o\| …))` |
| `.configure(\|app\| app.useOptions(\|o\| …))`（2.9.0 驼峰别名） | 删除；只有 `options` |

`options` 仍然只能通过 `HostBuilder::configure` 触达，是 `AppOptions` 的**唯一**入口：

```rust
Host::builder()
    .configure(|app| app.options(|o| o.metrics.enabled = false))
```

配置节绑定新增了带代码级覆盖的入口——绑定之后、创建三种注入实例之前执行闭包：

```rust
Host::builder()
    .add_options::<SiteConfig>("Site")
    // 绑定后覆盖（生产弱密钥 / 通配 CORS 仍 fail-closed）
    .add_options_with::<SiteConfig>("Site", |site| {
        site.public_url = std::env::var("SITE_URL").unwrap_or_default();
    })
```

六个组合根动词（`add_*` / `use_*` / `register` / `configure` / `add_options*` /
`watch_options_file`）与槽位表见 [`api-convergence.md`](../../design/api-convergence.md)。

## 5. HostBuilder 入口收敛（A 批）

| 2.9 | 2.9.1 |
|---|---|
| `use_session_store_cached(store, ttl)` | `use_session_store(Arc::new(CachedSessionStore::new(store, ttl)))` |
| `use_h3_alt_svc(port)` | `use_alt_svc("h3=\":443\"; ma=86400")` |
| `add_localization()` / `add_localization_with(opts)` | `use_localization(LocalizationOptions)` |
| `add_memory_cache()` / `add_memory_cache_with(n)` | `add_memory_cache(CacheOptions::default())` / `...max_entries(n)` |
| `add_options_monitor::<T>(section)` | `add_options::<T>(section)`（已同时注册 `Arc<OptionsMonitor<T>>` 与 `Arc<dyn IOptionsMonitor<T>>`） |
| `OperationResult::ok(payload, msg)` | `OperationResult::success(msg).with_result(payload)` |
| `OperationResult::ok_with_status(s, p, m)` | `OperationResult::success(m).with_status(s).with_result(p)` |
| `OperationResult::success_with_status(s, m)` | `OperationResult::success(m).with_status(s)` |

`CacheOptions::max_entries(0)` 在注册期拒绝。

### 5.1 2.x 兼容层删除（无别名）

四个 2.x 兼容层整体删除，没有 `#[deprecated]` 过渡，也没有别名：

| 2.9.0 及之前 | 2.9.1 |
|---|---|
| `DeclaredRules` + `IDynamicAuthorizer::authorize_with_rules` | `IDynamicAuthorizer::authorize_with_declared_rules(claims, route_pattern, method, &'static [AuthorizeRule])`；只做路由级判断时覆写 `authorize`（默认被转调） |
| `ResourceAuthorization` / `IAuthorizationPolicy` / `authz::resource_auth_middleware` | `#[authorize]` 静态规则，或 `#[authorize(dynamic)]` + `IDynamicAuthorizer`；实例归属在 Handler 用 `RequestUser::require_owner` / `require_same_tenant` |
| `init_jwt_secret` / `jwt_secret()`（全局签名密钥 shim） | 注入 `Arc<dyn ITokenIssuer>`；密钥来自 `Jwt.SecretFile`（解析顺序 env > `Jwt.Secret` > 文件）或自定义 issuer 用 `register_with_options` 读 `options.jwt.secret` / 解析 `JwtSecret` |
| `RateLimit.TrustProxy` / `RateLimit.TrustedProxies` 与 `RateLimiter::with_trust_proxy` | `ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`（转发头信任的唯一入口）；`RateLimitSection` 已无这两个字段，旧键被 serde 忽略、不再生效 |

- 授权侧迁移：静态角色/权限直接写 `#[authorize(role = ...)]` / `#[authorize(permission = ...)]`；
  “DB 是权限真相”写 `#[authorize(dynamic, permission = ...)]` 并实现
  `authorize_with_declared_rules`，见 [authorize 宏与声明式授权](../09-auth-security/authorize-macro.md) §动态授权；
- 归属侧迁移：`resource_auth_middleware` 原本就不看具体路径参数，删除后把
  `require_owner` / `require_same_tenant` 放进 Handler，见
  [资源授权与实例级归属](../09-auth-security/resource-authorization.md)；
- 注册登记表（含每一项的替代与出处）见 [`deprecations.md`](../../design/deprecations.md)。

## 6. 依赖分层：`starters` → 框架 crate + 下游适配

2.9.1 的根 workspace 只包含框架 crate 与协议/传输适配器；与 ORM/数据库耦合的持久化适配器
**移出发布序列**（全部 `publish = false`）。

> **迁移说明**：原 `integrations/` 已改名为 `examples/adapters/`（独立 workspace、自带 lockfile），
> 例如 `integrations/outbox/ef` → `examples/adapters/outbox/ef`。构建/测试命令里的 `--manifest-path`
> 相应换成 `examples/adapters/Cargo.toml`。

参考应用 docbit/dmbit/imbit 各自是 `examples/<app>/` 下的**独立 workspace**（各自 lockfile），
不再属于根 workspace。

```bash
cargo test --workspace                                              # 框架 crate
cargo test --workspace --manifest-path examples/adapters/Cargo.toml # 参考适配器（不发布）
cargo test --workspace --manifest-path examples/docbit/Cargo.toml
```

### 6.1 `rust-webx-ef`（`webx_ef`）已删除

兼容壳整个删除（不是"再保留一版"）：

- `map_ef_error` / `EfResultExt` 早在 2.9.1 移除，持久层错误直接用 `?`
  （`impl From<EFError> for AppError` 在 rust-ef 边界）；
- 持久化 `Idempotency-Key` store 改为**端口实现**：框架只有 `IIdempotencyStore` 与内存默认，
  数据库实现由下游提供（参考适配器 `examples/adapters/idempotency/ef`，非发布）；
- 历史 `webx_ef::*` 与 `webx_ef::idempotency::*` 路径**没有别名**：`use webx_ef::idempotency::…`
  改成实现/复制端口实现后使用自己的导入名（`EfIdempotencyStore`、`IdempotencyHousekeeping`、
  `register_idempotency_model`、`IdempotencyRecord` 都在参考适配器 crate 根）。

### 6.2 `*-ef` 移出发布序列

| 2.9.1 之前 | 2.9.1 起 |
|---|---|
| 五个 `rust-webx-*-ef` 适配 crate 随框架发布，用精确版本互相钉住 | 全部移入 `examples/adapters/`，`publish = false`，**不再随框架发布**；crates.io 上没有制品，也没有可钉的版本对 |
| 下游"加一个依赖 + 选 feature" | 下游**依赖 `rust-webx`**，在业务 crate 实现端口，或复制 `examples/adapters/*` 参考实现，跑 `testing/contract-tests` 的端口契约，再在组合根装配 |

- 业务代码里出现 `use webx_*_ef` / `rust_webx_*_ef` 就是错误用法：参考实现不是依赖项；
- 复制后包名、实体、表、迁移都归下游自己的仓库，可裁剪、可改名，不需要等框架发版；
- 三种接入方式与端口清单见 [端口与适配器（下游接入）](../13-extensibility/integrations.md)。

### 6.3 outbox 拆成「框架 crate + 适配器存储」

outbox 在 2.9.1 一分为二：

- **框架**：新发布的 `rust-webx-outbox`（导入名 `webx_outbox`）承载 `IOutbox`、`OutboxEntry`、
  `OutboxOptions`、`OutboxDispatcher`（租约/退避/死信/保留期）与 `InMemoryOutbox`；注册入口是
  `add_outbox(store, OutboxOptions)`（`store` 传 `Arc<dyn IOutbox>`，需要容器构建存储时用
  `add_outbox_with`）；该 crate **不链接 rust-ef**；
- **适配器**：EF 存储（`webx_outbox` 表、同事务 `enqueue_outbox`）留在 `examples/adapters/outbox/ef`，
  复制后以 `add_outbox_ef(OutboxOptions)` 装配（存储由适配器自己构建）。

原代码 `use webx_outbox_ef::{OutboxOptions, ...}` + `.add_outbox(..)` 的迁移动作：

| 2.9.0 | 2.9.1 |
|---|---|
| `use webx_outbox_ef::add_outbox`（EF 存储 + 派发器） | 单进程：`use webx_outbox::*` + `add_outbox(Arc::new(InMemoryOutbox::new(..)), options)`；要持久化：复制 `examples/adapters/outbox/ef`，改用 `add_outbox_ef(options)` |
| `register_outbox_model(&mut ctx)` | 保留，但函数来自复制后的适配器 crate |
| 事件由本进程 `#[handler] IEventHandler<T>` 接收 | 不变（派发语义仍在框架侧） |

### 6.4 参考应用迁到 `examples/`

- 三个参考应用在 `examples/{docbit,dmbit,imbit}`，各自独立 workspace；
- docbit / dmbit / imbit 都已是**四层**（`contracts` / `domain` / `handlers` / `host`），
  操作文件按 `<context>/<operation>.rs` 拆分；
- imbit 的 host 包名是 **`imbit-host`**（bin 名仍是 `imbit`）：

```bash
cargo run  --manifest-path examples/imbit/Cargo.toml -p imbit-host
cargo test --manifest-path examples/imbit/Cargo.toml
```

### 6.5 示例/约定层：`ToModel` / `ToEntity` / `ApplyTo` → 标准转换

2.7 的参考应用在 `examples/*/domain/src/mappers/mod.rs` 自定义了三个映射 trait
（`ToModel` 实体 → 读模型、`ToEntity` 创建种子 → 实体、`ApplyTo` 请求 → 实体变更），
handler 侧还有把实体标脏的辅助函数（docbit 的 `stage_update`、dmbit 的 `track_row`）。
它们**从来不是框架 API**，只是示例约定；2.9.1 的示例（迁移后）改用标准库转换与框架的
局部更新契约：

| 2.7 示例约定 | 2.9.1 示例 |
|---|---|
| `ToModel` + `to_model()` | `impl From<&Entity> for Model`，调用 `Model::from(&entity)` |
| `ToEntity` + `fn to_entity(id, now)` | `impl TryFrom<CreateXRequest> for Entity`（可失败，`type Error = webx::Error`；domain 只依赖 core 时是 `webx_core::Error`，同一类型） |
| `ApplyTo` + `fn apply_to(self, &mut Entity, now)` | `impl IPatchable<UpdateXRequest> for Entity { fn patch(&mut self, patch) }` |
| `stage_update(&mut ctx, entity)`（包装 `DbContext::update`，全列写） | `ctx.edit(entity, \|e\| e.patch(req.patch))` + `ctx.detect_changes()`（只写脏列） |

```rust
// before（2.7 示例约定）
let new_row = req.to_entity(id, now); // ToEntity：创建
req.apply_to(&mut loaded, now);       // ApplyTo：更新（loaded 为已加载实体）
stage_update(&mut ctx, loaded);       // DbContext::update：所有列都写
let model = new_row.to_model();       // ToModel：读模型

// after（2.9.1 示例）
// 创建：可失败转换
let new_row = Order::try_from(create_req)?;
ctx.add(new_row);

// 部分更新：只覆盖请求携带的字段
ctx.edit(loaded, |o| o.patch(update_req.patch));
ctx.detect_changes(); // 只把 patch 改过的列标脏
ctx.save_changes().await?;

// 读模型：OrderModel::from(&saved)，saved 为重新加载的实体
```

- `IPatchable<TPatch>` 是 2.9.1 新增的框架契约（`rust-core` 的 `persistence` 模块，
  `rust_core::IPatchable`）；`From` / `TryFrom` 属于 `core::convert`，由下游实现。
  示例里被换掉的 `ToModel` / `ToEntity` / `ApplyTo` 与辅助函数都不是框架公开 API，
  所以这**不是一次框架 API 移除**；但下游若复制过这套约定，应与示例同步迁移，
  否则会保留私有映射方言并继续用全列写。
- `TryFrom` 负责可失败的创建校验，`From<&Entity>` 只做无损投影（第二投影用具名方法）；
  `patch` 不返回 `Result`，可失败校验放进请求 `#[validate]` / Handler / 领域规则。
- 迁移收益：不再维护私有的三件套；`edit` 先拍快照、`detect_changes` 后只 UPDATE 脏列
  （`update` 是全列写）；`id`、租户与审计列仍由 Handler、身份与审计拦截器决定。
- 完整职责表与代码见
  [映射职责与最佳实践](../12-project-structure/contracts-handlers-domain.md)。

## 7. 发布顺序

只发布框架 crate（`examples/adapters/*` 全部 `publish = false`，不在顺序里）：

```text
rust-core 0.1 → rust-dix 1.1 → rust-ef 2.9.1
  → rust-webx 家族 2.9.1：core / host / engine / macros / spa / openapi / client / webx、
    领域组各 crate、rust-webx-outbox
```

- 家族内部用精确 pin（`=2.9.1`）互相约束，`rust-webx-outbox` 与家族同版本；
- `rust-ef` 在应用侧保持 semver 范围（`"2.6"`），不要收紧成 `=2.9.1`；
- `examples/adapters/*` 不发布：它们是参考实现，随示例仓演进，不参与发布顺序与 semver 门禁。

2.9.1 工作区里的 `[patch.crates-io]` 指向本地兄弟仓，发布后移除。
