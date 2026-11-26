# 观测契约：span 树与连接生命周期事件

> 维护者文档。定义 rust-webx 2.8.0 起**稳定的** tracing span / 事件名字、字段与关联规则；
> 面向接入 OpenTelemetry、日志聚合与告警的团队。名字与字段是公开契约，改法见 §6。
> 观测只经 `tracing`（下游可接 OTel / JSON 日志），框架不另造一套 JSON 事件格式。

---

## 1. 定位

请求可观测性由三部分协作，职责不重叠：

| 信号 | 归谁 | 回答的问题 |
|---|---|---|
| `x-request-id` / `x-trace-id` / `RequestContext::trace_id()` | RequestId、RequestTracing 中间件 | 这是**哪一次**请求（关联键） |
| span 树（本文） | engine pipeline、host 端点、认证中间件、engine context | 请求**在每一层花了多久、落在哪条路由、失败在哪一阶段** |
| RED 指标（`http_requests_total` 等） | MetricsMiddleware / `HttpMetrics` | 聚合成功率、延迟分布、inflight、连接 gauge |

原则：

- **不重复日志**：`RequestTracing::after` 的每请求访问日志、WS/SSE 的连接指标与业务
  `on_close` 钩子保持原样；span/事件是 trace 侧的补充，不复制同一条自由格式日志。
- **默认热路径零分配**：请求 span 以 **DEBUG** 级别发布；默认 `Host` 安装的是 INFO 级
  subscriber，因此默认部署只做 callsite 兴趣检查（静态/原子读），不分配、不做字符串格式化，
  不改变 [alloc-budget.md](alloc-budget.md) 的框架税与 [stability-gates.md](stability-gates.md)
  的 wire p99 语义。需要 span 时用 `RUST_LOG=...webx.http=debug`（或 OTel filter）显式开启，
  代价见 §5。
- **每请求 span 预算 ≤ 6**（根 + 最多 5 个子 span），见 §2 清单。

---

## 2. span 树

```text
http.server.request                   根：整个请求（中间件 + 路由 + handler + after）
├── auth.authenticate                 认证决策（配置了认证中间件时）
└── http.route_match                  路由终点：匹配 + 端点分发 + 响应写
    ├── http.handler                  端点适配器分发（DirectRouteTarget / StubEndpoint / ...）
    │   └── http.binding              请求体读取/解析（绑定中的 I/O 阶段）
    └── http.response_write           响应写（直接分发路径时挂在 route_match 下）
```

根 span 每个请求**恰好一个**：`Host::run` 走 engine pipeline（`crates/engine/src/pipeline.rs`）；
直接驱动公开的 `MiddlewarePipeline::execute`（嵌入/测试）时由 `crates/host/src/pipeline.rs`
创建同名根 span。两者互斥，不会重复。

| span 名 | target | 级别 | 父 span | 必填字段 | 落点 | 出现条件 |
|---|---|---|---|---|---|---|
| `http.server.request` | `webx.http` | DEBUG | —（根） | `method`、`path`、`trace_id`、`status` | `crates/engine/src/pipeline.rs:49,242,278`；`crates/host/src/pipeline.rs:22,81` | 每个经 pipeline 执行的请求 |
| `auth.authenticate` | `webx.http` | DEBUG | `http.server.request` | `scheme`、`outcome` | `crates/host/src/auth.rs:614` | 主机配置了 `AuthenticationMiddleware` |
| `http.route_match` | `webx.http` | DEBUG | `http.server.request` | `method`、`path`、`pattern`、`params` | `crates/engine/src/pipeline.rs:64,136,362,434` | 每个到达路由终点的请求；未匹配时 `pattern` 不出现 |
| `http.handler` | `webx.http` | DEBUG | `http.route_match` | `status` +（`handler` 或 `kind`） | `crates/host/src/endpoint.rs:186`（StubEndpoint）、`:299`（DirectRouteTarget）、`:33`/`:261`（Request/Controller） | 请求经 host 端点适配器分发；compile-time direct fast path 不经过适配器 |
| `http.binding` | `webx.http` | DEBUG | `http.handler`（直接分发时 `http.route_match`） | `kind`、`limit`、`bytes`（bytes 仅成功读取时） | `crates/engine/src/context.rs:650,744` | Handler 读取 body 或解析 multipart |
| `http.response_write` | `webx.http` | DEBUG | 当前 `http.handler` 或 `http.route_match` | `kind`、`status` | `crates/engine/src/context.rs:164` | 端点返回 `EndpointResult`（direct dispatch 与 `DirectRouteTarget`） |

span 一律 DEBUG：默认 `Host` 的 INFO subscriber 不创建它们，默认请求路径零分配；开启
`webx.http=debug`（或 OTel filter）才产生 span。级别也是契约（§6），不要按环境改成别的级别。

字段约定：

| 字段 | 类型 | 语义 |
|---|---|---|
| `method` / `path` | string | 请求行原值。`path` 含高基数 id，按路由聚合请用 `http.route_match.pattern` |
| `trace_id` | string | §4 的关联键；**始终存在**，空串表示该请求没有 trace id（未安装 RequestTracing） |
| `status` | integer | HTTP 状态码（u64） |
| `pattern` | string | 路由模板（如 `/api/users/{id}`） |
| `params` | integer | 路由参数个数 |
| `scheme` | string | 认证方案名；没有方案匹配时为 `"none"` |
| `outcome` | enum | `ok` / `anonymous` / `unauthorized` / `forbidden` / `rate_limited` / `error` |
| `handler` | string | 端点类型名（宏生成的 `request type`），静态字符串 |
| `kind` | enum | `http.handler`：`request` / `controller`；`http.binding`：`bytes` / `multipart`；`http.response_write`：`empty` / `json` / `action` |
| `limit` | integer | body 上限（字节） |
| `bytes` | integer | 实际读取的 body 字节数 |

---

## 3. 连接生命周期事件

长连接（WebSocket upgrade、SSE 流）在 `target = "webx.connection"` 上输出结构化事件，事件名放在
字段 `event` 中（`message` 与 `event` 同名，便于纯日志场景阅读）。每个连接 `opened`/`closed`
各**恰好一次**。

| 事件 | 级别 | 字段 | 出现时机 |
|---|---|---|---|
| `connection.opened` | INFO | `event`、`transport`、`connection_id`、`trace_id` + SSE 的 `path` | WS：upgrade 完成、session 开跑前；SSE：响应组装完成、开始推流前 |
| `connection.closed` | INFO（异常时 WARN） | `event`、`transport`、`connection_id`、`trace_id`、`reason`、`duration_ms`（+ 异常时 `error`） | WS：session 结束时；SSE：流正常结束 / 写失败 / body 被丢弃时 |

字段约定：

| 字段 | 类型 | 语义 |
|---|---|---|
| `event` | string | 稳定事件名：`connection.opened` / `connection.closed` |
| `transport` | string | `websocket` 或 `sse`（`TransportKind::as_str()`） |
| `connection_id` | string | `c<u64>` 形式的进程内单调 id（复用 `ConnectionId::next`）。SSE 在响应 finalize 时分配，WS 沿用 upgrade 请求的 connection meta |
| `trace_id` | string | upgrade/请求的 trace id；无则为空串（规则同 §4） |
| `path` | string | 仅 SSE：产生流的请求路径 |
| `reason` | enum | WS：`completed`（session future 正常返回）/ `error`；SSE：`completed`（源流结束）/ `client_disconnected`（body 被丢弃）/ `failed`（body 写错误） |
| `duration_ms` | integer | 连接存活时长（毫秒） |
| `error` | string | 仅 WARN 的 `connection.closed`：终止 session 的错误 |

**close_code 的边界**：engine 只持有会话 future 的返回值，拿不到 WebSocket 线上 close code；
SSE 也没有 close code。需要 wire code / `CloseReason` / `StreamEnd` 的细分（空闲超时、滚动重启、
token 过期等）时读 session 层信号（`IConnection::close_reason()`、`SseResult::on_close`、
`RealtimeMetrics`），本契约不伪造不存在的字段。`reason` 只表达 engine 能观察到的结束类别。

落点：

- WS：host 的 `WebSocketMiddleware` 安装 `IUpgradeSession`（`crates/host/src/websocket.rs`），
  engine 在 upgrade 完成后执行它并输出两侧事件（`crates/engine/src/context.rs:337`，opened
  `:358`，closed `:370`/`:380`）。
- SSE：`EngineContext::finalize` 检测 `text/event-stream` 后输出 `opened`，并把响应 body 包一个
  只触发一次的 drop/end 观察器输出 `closed`（`crates/engine/src/context.rs:210,242,272`）。
  普通响应不包装、不分配。

---

## 4. trace_id 关联规则

1. **来源优先级**（RequestTracing）：合法 `x-request-id`（≤128 个 `[A-Za-z0-9._-]`）→
   W3C `traceparent`（version `00`，32 hex trace id）→ 新生成 32 位小写 hex。
2. **一个值三处同源**：`RequestContext::trace_id()`、`x-trace-id` 响应头、`problem+json` 的
   `traceId`；根 span 的 `trace_id` 字段与连接事件的 `trace_id` 也是同一个值。
3. **记录时机**：根 span 在 trace id 分配（认证/追踪中间件）之前创建，因此 `trace_id` 声明为
   `tracing::field::Empty`，在 pipeline 返回后 `record`。该字段**始终存在**，空串表示本次请求没有
   trace id —— 与手册一致：未启用 `RequestTracing` 的主机不生成 trace id
   （见 [user-identity.md](../rust-webx/05-request-pattern/user-identity.md)）。
4. **后台任务 / 手工 span**：需要关联时用 `RequestContext::run(Some(identity), ..)` /
   `run_with_trace(..)` 进入请求上下文，或把已有 span 作为父 span；不要另造 request id。
5. **跨服务**：入站 `traceparent` 被复用时继续向出站传递同一 trace id；框架内不生成第二套 id。

---

## 5. 采样与开销

- **默认路径**：请求 span 是 DEBUG，默认 INFO subscriber 不创建它们，因此每个请求只做一次
  调用点兴趣检查；不分配、不 clone request 数据；body 只记录字节数，不记录内容。本机
  `alloc-probe compare 500 13`（release，默认配置）为 **Windows: framework 16.43 / baseline
  6.00 / tax 10.42**，**Linux CI: framework 18.43 / baseline 6.00 / tax 12.42**；两者都在
  [alloc-budget.md](alloc-budget.md) 的平台门禁内（Windows ≤12 / Linux ≤13）。
- **开启 span 的代价**：订阅者会为每个 enabled span 保存格式化字段。本机 alloc-probe 把
  `webx.http` 开到 DEBUG 后（`/hello` 该路由 3 个 span：root、route_match、response_write），
  framework 升到 70.43 allocs/request（+54）。这个代价只在显式开启 span 时发生；生产开启前请按
  自己的 exporter 复测（OTel batch exporter 往往比 fmt 便宜，但仍应按请求采样）。
- **采样位置**：在 subscriber / exporter 层做（OTel sampler、`tracing_subscriber` 的 filter）。
  不建议在框架代码里随机跳过 span：父 span 被跳过而子 span 保留会破坏 trace 树。
- **建议策略**：`status >= 500` 的请求、所有 `connection.closed` 全保留；成功请求按比例头采样。
  trace id 无论采样与否都随 `x-trace-id` 返回，日志/审计仍可关联。
- **过滤示例**：只开框架 span 与连接事件 ——
  `RUST_LOG=info,webx.http=debug,webx.connection=info`（target 与 span/事件名都是稳定契约，
  可安全用于过滤）。
- **SSE 成本**：每连接一个 boxed body 包装（长连接生命周期，非每消息路径）；非 SSE 响应零额外分配。

---

## 6. 命名即契约

以下都属于**公开契约**，与导出到 OTel / 日志索引的名字一一对应：

- span 名、`target`、级别、父/子关系；
- 字段名、字段类型、枚举取值；
- 事件名（`event` 字段值）、事件字段与级别。

规则：

- **只加不删**：新字段/新 span 是加性变更；不删字段、不改字段语义/类型。级别也是契约
  （请求 span DEBUG、连接事件 INFO/WARN）：调级别按改名流程处理，不得按环境静默改变。
- **改名必须弃用**：重命名 span/字段/事件需先新旧并写并标注，保留 ≥2 个 minor 后再删旧名，
  流程见 [compatibility.md](compatibility.md) 与 [deprecations.md](deprecations.md)。
- **不造第二套**：不得为同一信息新增平行 JSON 事件或第二套指标名字；扩展请加到本文表格并同批更新
  契约测试。
- **验收门禁**：`crates/host/tests/observability_test.rs` 用真实 tracing subscriber 捕获 span/事件，
  断言根 span 字段、trace id 与响应头一致、子 span 层级与连接事件字段；改名字必须同批改本文档 + 该测试。

---

## 7. 与既有信号的关系

| 既有信号 | 仍然负责 | 与本契约的关系 |
|---|---|---|
| `RequestIdMiddleware`（`x-request-id`） | 入口关联 id | RequestTracing 优先复用它作为 trace id |
| `RequestTracing`（`x-trace-id`、日志、`traceparent`） | trace id 分配 + 每请求访问日志 | 根 span 读同一个 trace id；日志保留，不重复 |
| `problem+json.traceId` | 错误响应的关联键 | 与根 span / 响应头同值 |
| `TimingMiddleware` / `RequestTracing::after` | 每请求耗时与结果日志 | 短期保留；span 提供分层分解，不替代日志 |
| RED 指标（`http_requests_total` 等） | 聚合成功率/延迟/inflight/连接 gauge | span 是单请求因果视图；采样后指标仍是全量，见 [metrics.md](../rust-webx/11-production/metrics.md) |
| WS/SSE 连接指标与 `on_close` 钩子 | 连接计数、业务收尾 | `connection.*` 事件只做生命周期轨迹，二者互补 |

设计准则（不可观测系数、故障必须可解释）见
[framework-constitution.md](framework-constitution.md) §1、§3。

---

## 8. 验收

```text
cargo test -p rust-webx-host --test observability_test
cargo test -p rust-webx-host --lib
cargo clippy -p rust-webx-host --all-targets -- -D warnings
```

`observability_test` 覆盖：根 span 的 `method`/`path`/`trace_id`/`status` 与 `x-trace-id`、
`RequestContext::trace_id()` 一致；`auth.authenticate` → `http.route_match` →
`http.handler` → `http.binding` 的父子层级与字段；SSE 与 WebSocket 的
`connection.opened`/`connection.closed` 各一次且 `connection_id`/`trace_id` 一致。
