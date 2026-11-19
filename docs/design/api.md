# rust-webx 公共 API 设计（1.1）

> 状态：**冻结契约**（1.1 已发布）。1.1 的公共 API 以本文为准；任何新增/改名都要先改本文，再改代码。
> 评估对象：`crates/core`、`crates/macros`、`crates/host`、`crates/webx` 的公开面。

## 1. 目标与非目标

**目标**

1. 让 ASP.NET Core 开发者能用同一套词汇读懂端点：`ActionResult`、`IActionResult`、`FileResult`、
   `ContentResult`、`NoContentResult`、`StatusCodeResult`、`ProblemDetails`。
2. 保持 webx 自己的架构选择：**Mediator（一操作 = 一 Request + 一 Handler + 编译期注册）**。
3. 错误可预期：handler 的失败走 `Err(webx::Error)`，由框架写 RFC 7807 `problem+json`。
4. 概念可数：返回模型只有"值 / 自行掌控的响应 / 错误"三种结局，其余都是糖。

**非目标（明确不做）**

- 不做 Controller/Action 类（Mediator 是选择，不是过渡态）。
- 不实现 C# 式 `ActionResult<T>` 隐式转换（Rust 没有隐式转换，强行用枚举会毁掉 `Ok`/`?`）。
- 不做运行时反射式 OpenAPI schema（Rust 无反射；当前只保证路径/参数/响应码，schema 为占位）。
- 不引入第二套 Web/ORM/DI 抽象。

## 2. 第一性原理：Rust 的硬约束

| 约束 | 后果 |
|------|------|
| 没有隐式转换 | `return user;` / `return File(...)` 在 Rust 里必须写成 `Ok(user)` / `Ok(Box::new(FileResult::…))` |
| trait object 需要 `Box` | `IActionResult` 不能直接当返回类型；`IRequest<R>` 的 `R` 必须是 sized 类型，因此有 `Response = Box<dyn IActionResult>` |
| 没有运行时反射 | OpenAPI 无法从 `Serialize` 类型推导 schema；只能靠宏/注解 |
| Mediator 需要请求类型 | 每个端点必须先有 Request 类型；`IRequest<R>` 把"响应类型"编码进类型系统，换取编译期校验 |

这些约束决定了 webx 不可能在"书写行数"上等于 C#；能对齐的是**概念与语义**，不是语法糖的量。

## 3. 返回模型（唯一契约）

> **一个 action 返回 `ActionResult<T>`：`Ok(T)` 被序列化为 `200` JSON；`Err(Error)` 映射为
> `problem+json`。需要控制 HTTP 的 action 把响应类型写成 `Response`，返回一个 `IActionResult`。**

三种结局，没有第四种（唯一特例是单元响应：`IRequest<()>` 由宏直接写成 `204`，不属于"值"序列化）：

```rust
// ① 值（最常见）
#[get("/users/{id}")]
impl IRequest<UserDto> for GetUserRequest {}

impl IRequestHandler<GetUserRequest, UserDto> for GetUserHandler {
    async fn handle(&mut self, req: GetUserRequest) -> ActionResult<UserDto> {
        Ok(self.repo.find(&req.id).await?)
    }
}

// ② 自行掌控响应（文件 / 非 JSON / 自定义状态码与头）
#[get("/reports/{id}")]
impl IRequest<Response> for DownloadReportRequest {}

impl IRequestHandler<DownloadReportRequest, Response> for DownloadReportHandler {
    async fn handle(&mut self, req: DownloadReportRequest) -> ActionResult<Response> {
        Ok(Box::new(FileResult::path(self.reports.path(&req.id)?).download_name("report.csv")))
    }
}

// ③ 失败（框架写 problem+json）
return Err(Error::NotFound(format!("user {} not found", req.id)));
```

**单元响应**：`IRequest<()>` 的 handler 返回 `Ok(())`，宏把它转成 `NoContentResult`（204 + 空 body）。
这是 typed action 的唯一特例；OpenAPI 把 `()` 记为 `{ "type": "object" }`。

**响应类型必须按字面写**：宏在编译期按类型名判定"自控响应"（末段为 `Response`，或类型名含
`IActionResult`），因此请写 `IRequest<Response>` 或 `IRequest<Box<dyn IActionResult>>`。
`type Resp = Response;` 之类的别名、或把业务 DTO 命名为 `Response`，都会被当成普通序列化类型
并在编译期报错（这是宏的边界，不是运行时不变量）。

**双轨边界（刻意保留）**

| 轨道 | 声明 | 能表达 | 不能表达 |
|------|------|--------|----------|
| typed action | `IRequest<T>` | 200 + 任意 `Serialize` 值（`()` 特例为 204）；编译期知道 body 类型 | 非 200 状态码、非 JSON body、文件 |
| response-control action | `IRequest<Response>` | 任意状态码/头/body、文件流 | 声明式 body 类型（OpenAPI 只能标记为 opaque） |

"201 + 带类型 body"属于跨轨道需求：用 `IRequest<Response>` + `JsonResult::new(dto).status(201)`，
代价是路由的声明类型变成 `Response`。这是 Rust 版本下**唯一**的取舍，写进文档而不是用枚举掩盖。

## 4. 概念表（公共面）

| 概念 | 所属 | 职责 | 一句话 |
|------|------|------|--------|
| `IRequest<T>` | core | 标记 + 响应类型 | 请求即契约 |
| `IRequestHandler<T, R>` | core | 唯一执行者 | 一操作一 Handler |
| `ActionResult<T>` | core | handler 返回类型 | `= Result<T, Error>`，ASP.NET 的 `ActionResult<T>` 位 |
| `Result<T>` | core | 通用错误联合 | 基础设施代码用这个名字 |
| `IActionResult` | core | 响应写入扩展点 | ASP.NET 的 `IActionResult` |
| `Response` | core | `Box<dyn IActionResult>` | Rust 里 `IActionResult` 的 sized 形态 |
| `JsonResult<T>` | core | 带状态码的 JSON | ASP.NET `JsonResult` |
| `ContentResult` | core | 文本/HTML/字节 | ASP.NET `ContentResult` |
| `FileResult` | core | 文件/流 | ASP.NET `PhysicalFile` / `File` |
| `SseResult` | core | 服务端事件流 | SSE 响应（chunked、`no-store`、断连钩子） |
| `NoContentResult` | core | 204 | ASP.NET `NoContentResult` |
| `StatusCodeResult` | core | 仅状态码 | ASP.NET `StatusCodeResult` |
| `OperationResult<T>` | core | 业务结论 DTO | `success` + 业务 `status` + `message` + `result` |
| `PagedResult<T>` | core | 列表 DTO | 分页信封 |
| `Error` / `ProblemDetails` | core | 失败模型 | RFC 7807 |

**删除记录**：`ResponseData`（1.1 删除，由 `IActionResult` 族替代）、`PagedResponse`（1.0.2 改名
`PagedResult`）、`Json<T> = T`（无引用别名）、各类 deprecated 别名（不保留）。

## 5. 与 ASP.NET Core / axum 的映射

ASP.NET Core 官方文档给出四种 action 返回类型：具体类型、`IActionResult`、`ActionResult<T>`、
`HttpResults`。其中 `ActionResult<T>` 的价值是：元数据可从 `T` 推断，且 `T` 与 `ActionResult`
可隐式转换（`return product;` / `return NotFound();`）。

| ASP.NET Core | rust-webx | 说明 |
|---|---|---|
| `Task<ActionResult<UserDto>>` | `ActionResult<UserDto>` | 值 / 错误 |
| `Task<IActionResult>` | `ActionResult<Response>` | 自行掌控响应 |
| `return user;` | `Ok(user)` | Rust 无隐式转换 |
| `return NotFound();` | `Err(Error::NotFound(..))` | 统一的 problem+json |
| `File(...)` / `PhysicalFile(...)` | `FileResult::path/stream/seekable_stream` | 文件与流 |
| `Content(...)` | `ContentResult::text/html/bytes` | 文本/字节 |
| `Json(value, status)` | `JsonResult::new(value).status(..)` | 带状态码 JSON |
| `NoContent()` | `NoContentResult` | 204 |
| `StatusCode(202)` | `StatusCodeResult(202)` | 仅状态码 |
| `[ProducesResponseType]` | 路由宏参数 | 当前只输出固定响应码 |

axum 的对照：axum 用 `IntoResponse` 统一"值/响应/错误"，端点返回 `impl IntoResponse` 或
`Result<T, E>`。webx 的 `IRequest<R>` 需要在编译期知道 `R`（序列化与 OpenAPI），所以不能照搬
`impl Trait`；`ActionResult<T>` + `Response` 是同一模型在"有类型声明"约束下的等价拆分。

## 6. 易用性评估（开发者视角）

测量口径：一个端点从声明到可运行所需的**独立声明项**（type/impl/attribute）与**类型重复次数**。

| 场景 | ASP.NET Core | rust-webx 1.1 | 差异原因 |
|------|--------------|---------------|----------|
| 普通 GET | 1 方法 | 1 请求结构体 + 1 路由 impl + 1 Handler 结构体 + 1 处理器 impl | Mediator 需要请求类型；`#[get]` 挂在空 impl 上是宏的语法载体 |
| 文件下载 | 1 方法 | 同上 + `Response` 类型 | Rust 需要 `Box<dyn IActionResult>` |
| 201 + body | 1 方法 | 2 轨道二选一 | 无隐式转换，typed 与 response-control 无法合并 |

**结论**：webx 的"重"来自 **Request 类型 + 标记 impl**（架构选择），不是返回类型；返回类型与 C# 基本同构。

已知摩擦点与处置：

| 摩擦 | 处置 |
|------|------|
| `#[async_trait]` 每个 impl 一行 | 1.1 保留（trait 仍是 async-trait 模型）；后续版本评估原生 AFIT |
| `Ok(Box::new(FileResult…))` | 保留（Box 是 trait object 的硬约束）；文档给最短写法 |
| `IRequest<R>` 与 handler 的 `R` 重复 | 编译器强制一致（`T: IRequest<R>`），是安全网不是冗余 |
| 双轨 | 文档化边界 + OpenAPI 对 `Response` 标记 opaque |
| 概念多 | 概念表 + 每个类型一句职责；删除死别名 |

## 7. 不变量（编译器保证 + 评审检查）

1. handler 的返回类型只能是 `ActionResult<T>`；基础设施返回 `Result<T>`。
2. typed action 的 `IRequest<T>` 与 handler 的 `R` 必须一致（编译器保证）。
3. 业务错误用 `Err(Error)`；HTTP 控制用 `IActionResult`；业务结论用 `OperationResult`，三者不得混用。
4. 文档/示例中 handler 上下文只出现 `ActionResult`，不出现裸 `Result`。
5. 公共 API 变更必须先改本文，并按 semver 判定版本号（1.1.0 的破坏性变更按项目决定并入
   1.x 发布线，不走 2.0 大版本，见 `CHANGELOG.md`）。

其中第 1、2 条由 trait 签名与编译器强制；第 3–5 条是评审约定，CI 的 `scripts/check-docs.sh`
只守版本/链接/索引一致性，不做语义检查。

## 7.1 追加：SSE 流式响应（`SseResult`）

**状态**：已实现（additive，随下一个 minor 发布）。`SseResult` 是 `IActionResult` 的一个实现，用于
`text/event-stream`：端点仍声明 `IRequest<Response>`，handler 返回
`Ok(Box::new(SseResult::new(stream)))`。

契约：

```rust
pub struct SseEvent {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: bytes::Bytes,
    pub retry: Option<u32>,
}

impl SseResult {
    pub fn new<S>(events: S) -> Self
    where S: futures_util::Stream<Item = Result<SseEvent>> + Send + 'static;
    pub fn keep_alive(self, every: Duration) -> Self;
    pub fn max_lifetime(self, lifetime: Duration) -> Self;
    pub fn on_close(self, hook: impl FnOnce(StreamEnd) + Send + 'static) -> Self;
    pub fn header(self, key: impl Into<String>, value: impl Into<String>) -> Self;
}

pub enum StreamEnd { Completed, ClientDisconnected, Failed(Error), Deadline }
```

不变量：

1. 响应固定 `200`，`content-type: text/event-stream; charset=utf-8`，
   `cache-control: no-store`，`x-accel-buffering: no`；调用方可用 `header()` 覆盖。
2. 传输走 chunked（无 `Content-Length`），每帧写完即交给 hyper；禁止先 collect 再发。
3. `on_close` 恰好触发一次；客户端断开时由 body 的 `Drop` 触发
   `ClientDisconnected`，回调必须非阻塞（异步结算自行 `spawn`）。
4. 源流 `Err` 结束流并记 `Failed`（不向客户端写半截错误）；需要客户端可见的错误
   由上层转成 `SseEvent`。
5. `keep_alive` 发注释帧 `: ping\n\n`，不占用事件序号；`max_lifetime` 到点记
   `Deadline` 并正常结束。
6. 压缩中间件必须跳过 `text/event-stream`。

## 7.2 追加：出站 HTTP（`rust-webx-client`）

**状态**：已实现（独立 crate，additive）。服务端需要调用上游 HTTP 时使用它，不再引入第二套
HTTP 栈。公共面：

```rust
pub trait IHttpClient: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, request: OutboundRequest)
        -> Result<OutboundResponse, HttpClientError>;
}

pub struct OutboundResponse { pub status: u16, pub headers: HeaderMap, pub body: ResponseStream }

impl ResponseStream {
    pub async fn collect(self, limit: usize) -> Result<Bytes, HttpClientError>;
    pub fn bytes(self) -> impl Stream<Item = Result<Bytes, HttpClientError>>;
    pub fn sse(self) -> SseStream;
}
```

不变量：

1. 每个上游一个 `HttpClient`（base URL / 连接池 / 超时独立）；`HttpClientRegistry` 按名字解析。
2. 非 2xx 是 `OutboundResponse`，不是 `HttpClientError`；调用方决定透传/切候选/映射。
3. `request_timeout` 只约束响应头；流式 body 由 `read_timeout` 约束帧间空闲。
4. 响应体默认流式；`collect` 必须给上限；事件流只能走 `sse()`/`bytes()`，禁止整段 collect。
5. 流式首字节之后禁止重试/切候选；`HttpClientError::is_connect()` 只用于首字节前的判定。
6. 组件不复用 `host` 的生命周期；请求取消依赖 future 被 drop（上游连接随之关闭/重置）。

## 7.3 追加：认证 scheme 链（`IAuthenticationScheme`）

**状态**：已实现（additive）。JWT 从"唯一认证中间件"变成 scheme 链中的一项，`add_authentication()`
的既有行为保持不变。

```rust
pub trait IAuthenticationScheme: Send + Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, ctx: &dyn IHttpContext) -> bool;
    async fn authenticate(&self, ctx: &mut dyn IHttpContext)
        -> Result<Option<Box<dyn IClaims>>>;
}
```

不变量：

1. 注册顺序即尝试顺序；首个 `matches` 命中的 scheme 处理请求，不再尝试后续 scheme。
2. `add_authentication_scheme` 的 scheme 排在 JWT 之前；JWT 由 `add_authentication()` 追加。
3. `matches` 必须无 IO、无副作用；凭证形状判断（`Bearer a.b.c`、`Bearer sk-…`、`x-api-key`）。
4. JWT 验证失败按匿名继续（既有语义）；API key 命中但校验失败返回 `401`，且不回显 key。
5. 无 scheme 命中 = 匿名继续；受保护端点由 `#[authorize]` 或 `#[fromclaim]` 身份绑定写 401。
6. scheme 只做凭证校验与 claims 产出；存储、撤销 TTL、配额属于 validator/store。

**API Key 存储/校验（已实现）**：

```rust
pub trait IApiKeyStore: Send + Sync {
    async fn get(&self, key_id: &str) -> Result<Option<ApiKeyRecord>, ApiKeyStoreError>;
}

let store = Arc::new(MemoryApiKeyStore::new());
store.insert(
    ApiKeyRecord::new("key-1", "tenant-1", hash_secret(pepper, secret).to_vec(), "v1")
        .with_roles(["chat"]),
);
let validator = StoreApiKeyValidator::new(store, "v1", pepper.to_vec());

Host::builder()
    .add_authentication_scheme(Arc::new(ApiKeyScheme::new(Arc::new(validator))))
```

不变量：

1. 只存 `HMAC-SHA256(pepper, secret)`，不存明文；`pepper_id` 支持轮换。
2. 每次请求检查 `status`、`revoked_at`、`expires_at`；撤销后下一请求即失效。
3. 未知 key、错误 secret、错误 pepper、过期/吊销都按匿名返回 `None`；store 故障返回 500。
4. claims：`sub=key_id`、`tenant_id`、`roles`、`permissions`、`pepper_id`。
5. 当前 `MemoryApiKeyStore` 用于测试/小部署；EF-backed store 与缓存 TTL 待补。

## 7.4 追加：配额预扣-结算（`IQuotaStore`）

**状态**：已实现于 `rust-webx-ai`（`MemoryQuotaStore`；分布式实现待补）。用于按
key/模型/tenant 计量请求、token、并发与预算。

```rust
pub trait IQuotaStore: Send + Sync {
    async fn reserve(&self, request: ReserveRequest) -> Result<Reservation, QuotaError>;
    async fn settle(&self, reservation: Reservation, usage: Usage, cost_micros: u64)
        -> Result<(), QuotaError>;
    async fn release(&self, reservation: Reservation) -> Result<(), QuotaError>;
}
```

不变量：

1. `reserve` 原子地检查并占用所有维度（RPM/TPM/TPD/并发/预算）；失败返回
   `QuotaError::Exceeded { dimension, limit, retry_after }`。
2. `settle` 用真实 `Usage`/成本替换估算（多退少补），并释放并发槽；`release` 全额退回估算。
3. `settle`/`release` 按 `lease_id` 幂等；重复调用是无操作。
4. 结算可以超过限额（请求已经发生），超卖由下一次 `reserve` 拒绝；不得在 `settle` 里丢账。
5. 租约到期只释放并发槽；估算的 token/成本保持计费（保守，不低估）。
6. 错误映射建议：`Exceeded` → `429`（`retry_after` 进 `Retry-After`），`Unavailable` → `503`，
   `Invalid` → `400`；OpenAI 表面在 G8 再转 `insufficient_quota`。
7. 限额由 `IQuotaPolicy` 解析（可缓存）；热路径不得每请求查库。

## 7.5 追加：用量计量（`IUsageSink` / `rollup_events`）

**状态**：已实现于 `rust-webx-ai`（内存 sink + 纯函数 rollup）与 `rust-webx-ai-ef`
（幂等 EF 账本 + outbox + rollup + `PeriodicBackgroundService` workers）。

```rust
pub trait IUsageSink: Send + Sync {
    async fn record(&self, event: UsageEvent) -> Result<(), UsageError>;
}
pub trait ITokenizer: Send + Sync {
    fn count(&self, model: &str, text: &str) -> usize;
}
pub fn rollup_events(events, window_secs: u64) -> Vec<UsageRollup>;
```

EF 账本：

```rust
use webx_ai_ef::EfUsageSink;

let sink = EfUsageSink::from_options(Arc::clone(&db_options));
let executor = GatewayExecutor::new(catalog, router).with_usage(Arc::new(sink));
```

Outbox 与 rollup：

```rust
let outbox = EfUsageOutbox::from_options(Arc::clone(&db_options));
let pending = outbox.pending(100).await?;
outbox.mark_delivered("req-1").await?;
outbox.mark_failed("req-1", "billing 503").await?;

let rollup = EfUsageRollup::from_options(Arc::clone(&db_options));
let rows = rollup.rollup_window(window_start, 60).await?;
```

Workers：

```rust
let outbox_worker = UsageOutboxWorker::new(outbox, handler, 100, Duration::from_secs(5));
let rollup_worker = UsageRollupWorker::new(rollup, Duration::from_secs(60), Duration::from_secs(30));
```

不变量：

1. `UsageEvent.request_id` 是幂等键；`record` 必须是 upsert-ignore（首写优先）。
2. 每个结束的请求（完成/断连/上游失败）恰好写一条事件；流式结算在 `GatewayStream`
   完成/断连/失败时触发。
3. `created_at_unix_secs == 0` 时由 sink 盖章；`EfUsageSink` 在转换时盖章，账本列也应有默认值。
4. `Usage` 各桶求和用 saturating；金额用整数 micro-USD。
5. `rollup_events` 是纯函数：固定窗口、按 tenant+model 分组；不在写路径做聚合。
6. 错误映射建议：`UsageError::Unavailable` → 503，`Invalid` → 400；写失败不得让已完成的请求回滚。
7. tokenizer 的估算只用于预扣/上游缺 usage 的兜底；权威值以上游返回为准。
8. `EfUsageSink` 每次写创建新 `DbContext`；账本与 outbox 同一 `save_changes` 写入；先查
   `request_id`，唯一冲突按首写优先吞掉。
9. outbox 的 `pending` 只返回 `delivered=false` 且 `next_attempt_at<=now` 的行；失败按指数退避，
   `mark_delivered` 幂等；rollup 按 `(window_start, tenant, model)` 幂等插入。
10. `UsageOutboxWorker`/`UsageRollupWorker` 实现 `PeriodicBackgroundService`：每 tick 处理有界批次，
    失败记录后退避，rollup worker 只处理最近一个已完成窗口。

## 7.6 追加：provider 端口、统一 IR 与路由

**状态**：IR/provider 端口/目录/路由/执行器/OpenAI 兼容适配器已实现于 `rust-webx-ai`；
Anthropic/Gemini 转译 lane 待补。

```rust
pub trait ChatProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn complete(&self, req: CanonicalRequest) -> Result<CanonicalResponse, ProviderError>;
    async fn stream(&self, req: CanonicalRequest) -> Result<CanonicalStream, ProviderError>;
}
pub trait IRoutePlanner: Send + Sync {
    async fn plan(&self, req: &RouteRequest) -> Result<RoutePlan, RouteError>;
}
```

不变量：

1. 数据面只认 `CanonicalRequest`/`CanonicalResponse`/`StreamEvent`；厂商字段走 `extensions`。
2. `ChatProvider` 声明能力；路由器按 `required` 能力过滤，不把能力不足的候选交给执行器。
3. `ModelCatalog` 是不可变快照；`plan` 零 DB/零锁竞争（健康表是进程内小锁）。
4. 别名策略只决定**排序**：Priority/Fallback 目录序、Weighted 权重序、LeastLatency EWMA、
   Capability 先过滤；执行器按序尝试并遵守首字节规则。
5. `HealthRegistry` 连续失败打开冷却；成功清零并更新 EWMA；冷却是本进程视角。
6. `RouteError::UnknownModel` → 404、`NoCandidate` → 503、`Invalid` → 400。
7. `OpenAiProvider` 实现 `ChatProvider`：请求编码/非流解码/SSE 事件解码；`request.model` 是上游
   模型名，别名解析与执行器负责在调用前替换。

## 7.7 追加：数据面执行器（`GatewayExecutor`）

**状态**：已实现于 `rust-webx-ai`；OpenAI 兼容 HTTP 表面已落在 `rust-webx-ai-gateway`（见 §7.9）。

```rust
pub struct GatewayContext { request_id, tenant_id, key_id, model_alias }
pub struct GatewayExecutor { /* catalog + router + quota + usage + tokenizer + health */ }

impl GatewayExecutor {
    pub async fn complete(&self, ctx: &GatewayContext, req: CanonicalRequest)
        -> Result<CanonicalResponse, GatewayError>;
    pub async fn stream(&self, ctx: &GatewayContext, req: CanonicalRequest)
        -> Result<GatewayStream, GatewayError>;
}
```

不变量：

1. **先预扣后调用**：`reserve` 失败不得调用上游；成功/失败/断连后 `settle`/`release` 恰好一次。
2. **首字节规则**：非流调用可对可重试错误换候选；`stream()` 一旦返回流就不再重试。
3. 流结束时按真实 usage 结算；断连时按 tokenizer 估算 `ClientGone`，并发槽必须释放。
4. 每个结束的流/调用写一条 `UsageEvent`；写失败不影响已返回的响应。
5. 健康回写：成功清零/更新 EWMA，上游错误记失败；客户端断开不算 provider 失败。
6. `GatewayError` 映射：route 404/503、quota 429/503、provider 502/503/504、internal 500。

## 7.8 追加：错误响应写入器（`IErrorResponseWriter`）

**状态**：已实现（`webx-core` 端口 + `HostBuilder::add_error_writer`；`webx-ai` 提供
`OpenAiErrorWriter`）。

```rust
pub trait IErrorResponseWriter: Send + Sync {
    fn matches(&self, route_pattern: Option<&str>, path: &str, accept: Option<&str>) -> bool;
    async fn write(&self, ctx: &mut dyn IHttpContext, error: &Error);
}
```

不变量：

1. 写入器按注册顺序取首个匹配；没有匹配时保持 RFC 7807 `problem+json`。
2. `matches` 同时拿到路由模式与原始 path：`/v1/` 的 404 没有路由模式，仍可被接管。
3. 写入器拥有状态码、header 与 body；5xx 不得把内部 detail 写给客户端。
4. 写入器只处理**框架错误路径**；业务成功/流式数据不走这里。
5. 写入器不能 panic；`write` 无返回值，写响应失败由 transport 兜底。
6. `OpenAiErrorWriter` 输出 `{"error":{message,type,param,code}}`，默认前缀 `/v1/`。
7. 配额限额拒绝在端点层映射为 `insufficient_quota`（type/code），保留 `Retry-After`；
   其他 429 仍为 `rate_limit_error`。

## 7.9 追加：OpenAI 兼容 HTTP 表面（`rust-webx-ai-gateway`）

**状态**：已实现；`POST /v1/chat/completions`（JSON + SSE）。

```rust
webx_ai_gateway::install(); // 把路由 inventory 链入二进制

Host::builder()
    .add_authentication()
    .add_error_writer(Arc::new(OpenAiErrorWriter::new()))
    .register(move |svc| svc.instance(Arc::new(executor)))
    .build()
    .run()
    .await?;
```

不变量：

1. 鉴权由 `#[fromclaim] user: IdentityUser` 提供；tenant 缺失返回 403，未认证由框架返回 401。
2. 请求体 `model` 是公共别名；响应 `model` 也回显公共别名，不泄露上游模型名。
3. 非流返回 `object: "chat.completion"`；流返回 `object: "chat.completion.chunk"`，以 `[DONE]` 结束。
4. 流式 usage 作为 `choices: []` 的 chunk 下发（若上游提供）；失败在已开始流后以 error chunk 结束。
5. Handler 业务失败返回 OpenAI 信封（含 `Retry-After`）；绑定/鉴权/路由失败由
   `OpenAiErrorWriter` 统一接管。
6. 本 crate 只做 HTTP 适配；路由、provider、配额、用量、健康全部在 `rust-webx-ai`。
7. `GET /v1/models` 返回当前 catalog 的公共别名列表（OpenAI `{"object":"list"}` 形状）；
   需要鉴权，不泄露上游模型名。

## 7.10 追加：韧性（`RetryPolicy` / `CircuitBreaker` / `Bulkhead`）

**状态**：已实现于 `rust-webx-ai`；执行器已接线。

```rust
let executor = GatewayExecutor::new(catalog, router)
    .with_retry_policy(
        RetryPolicy::default()
            .with_max_attempts(3)
            .with_base_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_secs(2)),
    )
    .with_circuit_breaker(Arc::new(CircuitBreaker::new(5, Duration::from_secs(10))))
    .with_bulkhead(Arc::new(Bulkhead::new(64, Duration::from_millis(50))));
```

不变量：

1. 只对 `ProviderError::is_retryable()` 重试；`InvalidRequest`/`Unauthorized` 等不重试。
2. 退避 = `min(base * 2^(n-1), max)` + 50–100% jitter；`Retry-After` 优先；sleep 前检查总 deadline。
3. 熔断按 `ModelKey` 计：连续失败达到阈值 → Open(cooldown)；冷却后只放一个 HalfOpen 探针；
   探针成功 → Closed，失败 → 再次 Open。
4. 舱壁按 provider 计：`Semaphore` + 最大等待；full 返回 503 类错误；流式请求的 permit 持有到
   流结束/客户端断开。
5. 熔断/舱壁拒绝不得调用 provider，也不得产生 usage。

## 7.11 追加：指标（`IMeter`）

**状态**：已实现于 `webx-core`；`rust-webx-ai` 执行器已接线。

```rust
pub trait IMeter: Send + Sync {
    fn counter(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Counter>;
    fn histogram(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Histogram>;
    fn gauge(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Gauge>;
}
```

不变量：

1. 指标名是静态字符串；标签只放低基数维度（model/provider/status/kind），**禁止** key_id、
   tenant_id、request_id、prompt 文本。
2. `counter` 只增；`histogram` 记录观测；`gauge` 可正负。
3. 执行器发出：`ai_gateway_requests_total`、`ai_gateway_request_duration_ms`、
   `ai_gateway_ttft_ms`、`ai_gateway_tokens_total`、`ai_gateway_cost_micros_total`、
   `ai_gateway_active_streams`、`ai_gateway_upstream_errors_total`、
   `ai_gateway_quota_rejections_total`。
4. `active_streams` 在流创建时 +1，在完成/断连/失败时恰好 -1。
5. 指标写入失败不得影响请求；实现应非阻塞（`NoopMeter` 丢弃，`MemoryMeter` 仅测试用）。
6. exporter/后端（Prometheus/OTel）由应用实现 `IMeter`，框架不绑定。
7. `ICostCalculator`/`StaticCostTable` 用整数 micro-USD；cached/reasoning 桶单独计价，
   不重复计入 base token。成本同时写入配额结算、`UsageEvent.cost_micros` 与
   `ai_gateway_cost_micros_total`。
8. 执行器用 `tracing` 输出结构化完成/失败/流日志：request_id、tenant_id、model、provider、
   mode、status、latency、tokens、cost、trace_id；**不含** prompt/response 文本。
9. `RequestTracing` 优先 `x-request-id`，否则采纳合法 W3C `traceparent` 的 32-hex trace id；
   非法/全零则生成新 id。`x-trace-id` 与 `RequestContext::trace_id()` 始终一致。
10. `rust-webx-client` 在请求未显式设置 `traceparent` 时，从 `RequestContext::trace_id()` 注入
    `00-{trace_id}-{span}-01`；显式 header 优先。`tracestate` 透传待补。

## 7.12 追加：目录热替换（`CatalogHandle`）

**状态**：已实现于 `rust-webx-ai`；router 与 executor 可共享同一 handle。

```rust
let handle = CatalogHandle::new(initial_catalog);
let router = Arc::new(CatalogRouter::from_handle(handle.clone()));
let executor = GatewayExecutor::new_with_handle(handle.clone(), router);

handle.reload(new_catalog); // false when version is older
```

不变量：

1. `current()` 无锁返回 `Arc<ModelCatalog>`；热路径不查库、不持锁。
2. `reload` 只接受 `version >= 当前` 的快照；旧版本返回 `false` 且不改变当前值。
3. `CatalogRouter::plan` 与 `GatewayExecutor` 的 provider 解析读同一 handle；配置变更无需重启。
4. 正在执行的请求使用它开始时读到的快照；新请求看到新快照。
5. 健康状态（`HealthRegistry`）不随目录替换重置。
6. 文件/DB watcher、配额/价格/key 轮换、多实例版本轮询由应用或后续版本提供。

## 7.13 追加：精确响应缓存（`IGatewayCache`）

**状态**：已实现于 `rust-webx-ai`（内存实现；Redis/单飞待补）。

```rust
pub trait IGatewayCache: Send + Sync {
    async fn get(&self, key: &str) -> Option<CanonicalResponse>;
    async fn put(&self, key: String, response: CanonicalResponse, ttl: Duration);
}

let executor = GatewayExecutor::new(catalog, router)
    .with_cache(Arc::new(MemoryGatewayCache::new(1024)))
    .with_cache_ttl(Duration::from_secs(60))
    .with_single_flight();
```

不变量：

1. 仅 `!stream && temperature == Some(0.0)` 可缓存；其他请求不生成 key。
2. key = SHA256(tenant + NUL + 完整请求序列化)；tenant 隔离，绝不跨租户复用。
3. 命中返回缓存响应，不调用 provider、不预扣配额、不写 usage；请求指标 status=`cache_hit`。
4. TTL 到期即 miss；内存实现有容量上限并清理过期项。
5. `put` 在成功响应后执行；失败响应不缓存。
6. 缓存指标：`ai_gateway_cache_hits_total` / `ai_gateway_cache_misses_total`。
7. `with_single_flight()`：同 key 并发请求共享一次上游调用（成功/失败都广播）；
   flight 完成后移除 map 条目；流式不参与。
8. 单飞与分布式缓存由后续实现补充，不改变 key 契约。

## 7.14 追加：长流停机预算（`Server.*DrainTimeoutSeconds`）

**状态**：已实现于 host；连接级 SSE 活动跟踪。

```json
{ "Server": { "DrainTimeoutSeconds": 30, "StreamDrainTimeoutSeconds": 300 } }
```

不变量：

1. 普通连接使用 `DrainTimeoutSeconds`；只要该连接产生过 SSE 响应，就使用
   `StreamDrainTimeoutSeconds`。
2. SSE 活动按连接标记：连接内首个 `text/event-stream` 响应 +1，连接结束时 -1。
3. 停机先停止 accept，再等待连接 join；超时后 abort 剩余连接。
4. 预算最小 1s；0 会被钳制，避免立即 abort。
5. `SseResult::max_lifetime` 仍用于兜底僵尸流。
6. 待补：停机 SSE 事件、按流精确预算、后端 drain 协调。

## 7.15 追加：入站请求体流式（`body_stream`）

**状态**：已实现（core trait + host 实时流入站；client `body_stream` 出站）。

```rust
pub type RequestBodyStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

async fn body_stream(&mut self) -> Result<RequestBodyStream>;

OutboundRequest::post("/path")
    .body_stream(chunks); // Stream<Item = Result<Bytes, HttpClientError>>
```

不变量：

1. 默认实现回退 `body_bytes` 并作为单 chunk 返回，测试/替代 host 无需改。
2. host 覆盖为实时连接流；body 只能消费一次，之后 `body_bytes` 报错。
3. 流式读取仍按 `App.MaxBodySize` 计数；超出返回 413。
4. 与 `multipart()` 互斥；不能同时以表单和裸流读取。
5. 普通 DTO 绑定仍走缓冲；裸流由 middleware / `IRequest<Response>` 适配器使用。
6. 出站 `body_stream` 用 `UnsyncBoxBody` + `StreamBody` 逐 chunk 发送；`OutboundRequest` 不再
   `Clone`（仍 `Debug`）。
7. 按路由 MaxBodySize、出站 content-length/chunked 策略、背压调优待补。

## 7.16 追加：成本预演（`GatewayExecutor::estimate` / `dry_run`）

**状态**：已实现于 `rust-webx-ai` 与 OpenAI 表面。

```rust
pub struct GatewayEstimate {
    pub model: String,
    pub prompt_tokens: u64,
    pub max_completion_tokens: u64,
    pub estimated_cost_micros: u64,
}

let estimate = executor.estimate(&ctx, &canonical_request);
```

HTTP：

```json
{ "model": "fast", "messages": [...], "max_tokens": 10, "temperature": 0, "dry_run": true }
```

返回：

```json
{
  "object": "gateway.estimate",
  "model": "fast",
  "prompt_tokens": 1,
  "max_completion_tokens": 10,
  "estimated_cost_micros": 1500
}
```

不变量：

1. 只做 tokenizer + 价格表计算；**不**调用 provider、不预扣配额、不写 usage。
2. completion 取 `max_tokens`，缺省用执行器 `default_max_tokens`。
3. 成本由 `ICostCalculator` 计算；未配置价格时为 0。
4. 仍需鉴权/tenant；`dry_run` 可与 `stream` 同时出现，但只返回估算。
5. 估算不是承诺；真实结算以 usage 为准。

## 7.17 追加：Mock 上游（`webx_client::testing::MockUpstream`）

**状态**：已实现于 `rust-webx-client`；用于集成测试与适配器 conformance。

```rust
let mock = MockUpstream::start().await?;
mock.json(200, &body);
mock.sse([r#"data: {"choices":[{"delta":{"content":"hel"}}]}"#, "data: [DONE]"]);
mock.status(429);
mock.delay_before_next(Duration::from_millis(50));
let requests = mock.requests();
```

不变量：

1. 只绑定 `127.0.0.1:0`，测试结束 drop 即停止；不用于生产。
2. 请求按入队顺序消费脚本；无脚本返回 503。
3. 记录 method/path/headers/body，供断言与脱敏后 replay。
4. `fixture()`/`save_fixture`/`load_fixture`/`replay` 支持文件式 record/replay；fixture body 为
   UTF-8（二进制会 lossy），敏感 header 需在保存前脱敏。
5. SSE 脚本是原始 `data:` 行；调用方负责完整帧（`\n\n` 会补齐）。
6. 延迟/状态码用于超时、429、5xx 回归。
7. 官方 SDK conformance、真实 fuzz/压测待补；SSE 解码器已有“任意切分点 + 伪随机多 chunk”回归。

## 8. 迁移（1.0.2 → 1.1）

| 1.0.2 | 1.1 |
|-------|-------|
| `ResponseData::file(p)` | `FileResult::path(p)` |
| `ResponseData::file_stream(r, ct)` | `FileResult::stream(r).content_type(ct)` |
| `ResponseData::file_seekable_stream(r, len, ct)` | `FileResult::seekable_stream(r, len).content_type(ct)` |
| `ResponseData::with_file(b)` | `FileResult::with_body(b)` |
| `ResponseData::bytes/text/html` | `ContentResult::bytes(v, ct)` / `::text(s)` / `::html(s)` |
| `ResponseData::no_content()` | `NoContentResult` |
| `IRequest<ResponseData>` | `IRequest<Response>` |
| `webx::Json<T>`（别名） | 已删除，用 `JsonResult<T>` |

## 9. 已知债务与处置

- **OpenAPI schema**：不生成业务字段（Rust 无反射）。已改为"占位 component"：`$ref` 不再悬空，
  文档明确标注字段需查 Rust 类型。
- **发布脚本**：`publish.sh` 与 `.github/workflows/publish.yml` 的索引轮询逻辑已抽到
  `scripts/crates-index.sh`，两份路径共享同一实现（2026-11-12 收口）。
- **`IRequest<T>` 的空 impl**：宏需要承载属性；后续版本评估 `#[request(GET, "/path")]` 派生式写法。
- **`INDEX.md` / `INDEX.json`**：手工双维护，尚无生成器；下一步用脚本从 `INDEX.md` 生成 JSON。
- **两套文档树**：`docs/rust-webx`（使用者手册）与 `docs/design`（维护者设计），属有意拆分。
- **`docs/reviews/`**：内部计划/复盘记录已从对外文档树删除（历史保留在 git 中）。
