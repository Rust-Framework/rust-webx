# HTTP 错误码注册表

> rust-webx 的 `application/problem+json` 中 `code` 扩展成员的权威契约：
> 有哪些稳定码、HTTP 状态与 `retryable` 的对应关系、problem `type` URI 的约定，
> 以及新增码必须同步哪些位置。前前基线 2.9.0；长连接关闭码见
> [close-codes.md](close-codes.md)。

---

## 1. 契约

RFC 9457 problem 响应的扩展成员（不是“另一种信封”，客户端按 `code` 分支）：

| 成员 | 类型 | 语义 |
|---|---|---|
| `code` | string | **稳定**机器可读码，小写 snake_case；同一 HTTP 状态可有多个码，由它区分原因 |
| `status` | number | HTTP 状态（响应行同值） |
| `retryable` | bool | 重试同一请求是否可能成功（过载/上游瞬时故障，以及“等待冲突方完成后可重试”的 `conflict_retryable` 为 `true`） |
| `traceId` | string | 与 `x-trace-id` 响应头同源，用于日志关联 |
| `detail` / `title` | string | 面向人；**不是**契约，客户端不得解析 |

`code`、`retryable` 由 `ProblemDetails::complete` 保证存在：应用未显式给值时，
由 [`Error::code()`](../../crates/core/src/error.rs) /
[`Error::retryable()`](../../crates/core/src/error.rs) 或
[`default_code_for_status()`](../../crates/core/src/error.rs) /
`status_is_retryable()` 填充。

## 2. problem `type` URI 约定

- 框架自动生成的问题（引擎错误 writer、绑定/校验失败、404/405 等）统一使用
  `type = https://httpstatuses.com/{status}`：`type` 只表达 **HTTP 状态族**。
- 同一个 `type` 下可能有多个 `code`（例如 `404` 的 `not_found` 与
  `route_not_found`，`500` 的 `internal_error`/`dependency_injection`/…），
  所以 **`type` 不是稳定分支键**。
- 客户端必须按 `code`（必要时叠加 `status`）分支；`type` 仅用于把响应归类到
  HTTP 语义。框架不按 `code` 生成 per-code 的 `type` URI——否则每个新增码都会
  冻结一个 URI。
- 应用可用 `ProblemDetails::typ` 表达自己的领域类型（例如内部错误目录），
  但这属于应用契约；框架不读取 `type`，host 原样写出。
- 响应 `content-type` 固定为 `application/problem+json`（见
  [`ProblemDetails`](../../crates/core/src/problem.rs)）。

## 3. 稳定码表（`ERROR_CODES`，23 项）

`status` 为 `FALLBACK_STATUS = 0` 表示状态由错误自身携带（仅通用兜底码 `error`）。

| `code` | `status` | `retryable` | 含义 | 产生路径 |
|---|---|---|---|---|
| `bad_request` | 400 | false | 通用客户端请求错误 | `Error::Http`；`Status(400)` |
| `invalid_json` | 400 | false | 请求 JSON 无法解析 | `Error::Serialization` |
| `validation_failed` | 400 | false | 字段/跨字段校验失败 | `Error::Validation`；`ProblemDetails::validation` |
| `unauthorized` | 401 | false | 未认证或凭证无效 | `Error::Unauthorized` |
| `forbidden` | 403 | false | 已认证但无权限 | `Error::Forbidden` |
| `not_found` | 404 | false | 业务资源不存在 | `Error::NotFound`；`ProblemDetails::not_found` |
| `route_not_found` | 404 | false | 没有匹配的路由 | `Error::Routing` |
| `method_not_allowed` | 405 | false | 路径存在但方法不允许 | `Status(405)` 兜底 |
| `conflict` | 409 | false | 并发/状态冲突（永久冲突） | `Error::Conflict` |
| `conflict_retryable` | 409 | **true** | 可等待的冲突：冲突方完成后重试可能成功 | `Error::conflict_retryable` |
| `payload_too_large` | 413 | false | 请求体或单个上传文件超限 | `Error::PayloadTooLarge` |
| `unsupported_media_type` | 415 | false | 端点不支持该 content-type | `Error::UnsupportedMediaType` |
| `unprocessable_entity` | 422 | false | 语义不可处理 | `Status(422)` 兜底 |
| `too_many_requests` | 429 | **true** | 被限流，可退避重试；typed 变体同时写 `Retry-After` | `Error::too_many_requests`；`Status(429)` 兜底 |
| `dependency_injection` | 500 | false | DI 解析/装配失败（服务端缺陷） | `Error::Di` |
| `internal_error` | 500 | false | 未分类服务端错误 | `Error::Internal`；`Error::Message`；`Status(500)` |
| `invalid_state` | 500 | false | 状态机/编程错误（如响应已写出） | `Error::State` |
| `response_serialization` | 500 | false | 响应 DTO 序列化失败（服务端缺陷） | `Error::ResponseSerialization` |
| `not_implemented` | 501 | false | 功能未实现 | `Status(501)` 兜底 |
| `bad_gateway` | 502 | **true** | 上游返回非法响应 | `Status(502)` 兜底 |
| `service_unavailable` | 503 | **true** | 依赖不可用/过载，可重试 | `Status(503)` 兜底 |
| `gateway_timeout` | 504 | **true** | 上游超时 | `Status(504)` 兜底 |
| `error` | `FALLBACK_STATUS` | false | 通用兜底：无专用码的状态 | 任意未登记状态的 `Error::Status` / `ProblemDetails` |

不变量（由 `crates/core/tests/error_codes_test.rs` 断言）：

1. 每个 `Error` 变体的 `code()` 都在表中且 `status`/`retryable` 与
   `status_code()`/`retryable()` 一致；
2. `default_code_for_status` 的每个专用状态都在表中且与
   `status_is_retryable` 一致；
3. 表中无重复、均为 snake_case；`retryable` 只对 `429/502/503/504` 与
   `conflict_retryable`（409，按条件而非状态类重试）为 `true`；
4. 表的码集合**恰好**等于运行时可达码集合（不允许陈旧条目）；
5. 未登记状态回落到 `error`，且 `error` 永远不可重试。

`conflict_retryable` 是唯一的“按条件可重试”条码：它保留 409（请求与前前状态冲突），
但框架知道该冲突是瞬时的（另一请求正在处理同一幂等键/同一计划变更），因此
`retryable: true`。它不是“自动重试”指令：客户端应等待冲突方完成、重读状态或重放
同一幂等键；永久冲突仍用 `conflict`（`retryable: false`）。typed 429 的
`retry_after_secs` 由引擎错误 writer 写成 RFC 6585 `Retry-After` 响应头（只写一次）。

## 4. 覆盖范围与边界

- **覆盖**：`Error` 的每个变体、`default_code_for_status` 的全部稳定输出。
  这些是框架自身会写出的码。
- **不覆盖**：应用自定义码。应用可以通过
  `ProblemDetails::with_code("...")` 定义自己的 `code`（例如
  `rate_limited`），host 原样写出；它属于应用契约，**不要**登记到
  `ERROR_CODES`。
- **不覆盖**：WebSocket 帧级错误 `{"error":{"code","message","retryable"}}`
  （如 `rate_limited`、`frame_too_large`、`invalid_message`）。那是帧协议而非
  HTTP problem；关闭原因与 wire code 见 [close-codes.md](close-codes.md)。
- 审计/日志侧：`code` 与 `traceId` 一起足以在 5 分钟内定位一次失败的因果链，
  无需解析 `detail`。

## 5. 新增一个稳定码

同一批必须完成（缺一项 CI 或评审即拒绝）：

1. `crates/core/src/error.rs`：新增变体或状态映射，码字符串确定后**永不变更**；
2. `crates/core/src/error_codes.rs`：在 `ERROR_CODES` 登记（`code`/`status`/
   `retryable`），保持按状态排序；
3. `crates/core/tests/error_codes_test.rs`：把新变体加入 `representative_errors`
   （对其 `Error` 变体的穷举 match 会先把编译挡住），新状态加入
   `DEFAULT_STATUSES`；
4. 本文表格追加一行；若同时引入新的关闭原因，还要更新
   [close-codes.md](close-codes.md)；
5. 发布流程追加 `CHANGELOG.md`（由发布负责人维护）。

2.x 只做加性变更：不重命名、不删除、不复用已发布码；语义变更等同于新码。

## 6. 相关

- [close-codes.md](close-codes.md)：WebSocket/SSE 关闭原因与 wire code。
- [`crates/core/src/error_codes.rs`](../../crates/core/src/error_codes.rs)：注册表。
- [`crates/core/src/error.rs`](../../crates/core/src/error.rs)：`Error` 与映射函数。
- [`crates/core/src/problem.rs`](../../crates/core/src/problem.rs)：RFC 9457 结构与扩展成员。
