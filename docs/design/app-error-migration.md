# 错误载体：`webx::Error = rust_core::AppError`

> 状态：2.9.1 起生效（破坏性补丁，semver 例外）。本页是这条契约的**设计权威**：为什么统一、
> 字段契约、分类矩阵、清洗与边界转换规则。逐条源码迁移动作见
> `docs/rust-webx/16-migration/upgrade-to-2.9.1.md` §1；机器码表的唯一权威是
> `rust_core::error_codes::ERROR_CODES`（`error-codes.md` 只是它的文档投影）。

## 1. 问题与决策

2.9 及以前，`webx_core::Error` 是一个 19 变体的 enum，`rust-dix` 有 `RdiError`、`rust-ef` 有
`EFError`。三个框架各自定义"失败"，于是每个应用都要写一层映射（`map_ef_error`、
`map_di_error`、`map_protector_error`），而且映射漏掉的状态会把可重试的基础设施故障
误报成不可重试的客户端错误（典型：SQLite `busy` 经 `EFError::Query` 变成 400）。

决策：**一个应用错误载体，跨框架共享**——`rust_core::AppError`（`crates/core`，无 I/O、无 async、
只依赖 serde）。各框架保留自己的诊断类型，在**边界**用 `From` 收敛：

| 边界 | 来源类型 | 收敛方式 |
|---|---|---|
| DI | `rust_dix::RdiError` | `From<RdiError> for AppError`（rust-dix 1.1） |
| 持久层 | `rust_ef::EFError` | `From<EFError> for AppError`（rust-ef 2.9.1） |
| 密钥 | `webx::SecretError` | `From<SecretError> for Error`（webx 2.9.1） |
| 预构造 problem | `ProblemDetails` | `impl From<ProblemDetails> for AppError` + 反向 `from_app_error` |

`Rust` 没有异常，所以 `?` 是唯一需要被满足的语法：只要边界实现 `From`，handler 里
`self.ctx.save_changes().await?` 就能直接返回框架结果类型。这也是**禁止应用侧再写映射表**的
原因——第二套映射必然与框架的分类规则漂移。

## 2. 字段契约

字段私有，只读通过访问器（新增字段不算破坏），构造走构造器：

| 成员 | 访问器 | 语义 |
|---|---|---|
| kind | `kind() -> ErrorKind` | 粗分类，决定默认 status/code/retryable（见 §3） |
| code | `code() -> &str` | 稳定机器码（snake_case）；应用自定义码允许存在，但**不注册**进框架表 |
| detail | `detail() -> Option<&str>` | **客户端可见**文案；`None` = 不显示，渲染层回退到状态标题 |
| operator | `operator() -> Option<&str>` | **仅日志**；原始缺陷文本（SQL、列名、容器消息、密钥名） |
| retryable | `retryable() -> bool` | 同一请求重试是否可能成功 |
| status | `status_code() -> u16` | 生效的 HTTP 状态；无专用 kind 的状态（418/422/501）用显式覆盖 |
| fields | `fields() -> &[FieldProblem]` | 绑定/校验的逐字段问题 |
| extensions | `extensions() -> &Map` | 附加成员；渲染层拥有 wire 格式，本 crate 只承载 |

`public_detail()` = 4xx 的 `detail`，5xx 恒为 `None`（服务端缺陷不回显）；`Display` 依次打印
operator / detail / code，**禁止**把它当客户端文案（用 `public_detail()` 或状态标题）。

构造器一览：`validation`、`bad_request`、`unauthorized`、`forbidden`、`not_found`、
`route_not_found`、`conflict`、`conflict_retryable`、`payload_too_large`、
`unsupported_media_type`、`too_many_requests(secs, detail)`、`unavailable`、`internal(operator)`、
`di(operator)`、`state(operator)`、`invalid_json`、`response_serialization`、`status(code, detail)`；
链式 `with_detail` / `with_operator` / `with_status` / `with_retryable` / `with_field` /
`with_extension`。

## 3. 分类矩阵

`kind` 决定默认值；构造器可以收紧（见备注）。

| kind | 默认 status | 默认 code | retryable | 备注 |
|---|---|---|---|---|
| `InvalidInput` | 400 | `bad_request` | 否 | `validation` 用 `validation_failed`；`invalid_json` 用 `invalid_json` |
| `Unauthorized` | 401 | `unauthorized` | 否 | 认证缺失/非法 |
| `Forbidden` | 403 | `forbidden` | 否 | 已认证但无权限 |
| `NotFound` | 404 | `not_found` | 否 | `route_not_found` 用于路由未命中 |
| `Conflict` | 409 | `conflict` | 否 | `conflict_retryable` 覆盖为可重试 409 |
| `PayloadTooLarge` | 413 | `payload_too_large` | 否 | |
| `UnsupportedMediaType` | 415 | `unsupported_media_type` | 否 | |
| `RateLimited` | 429 | `too_many_requests` | **是** | 携带 `x-webx-retry-after` 秒数 |
| `Unavailable` | 503 | `service_unavailable` | **是** | 上游不可用/超时（含 502/504 覆盖） |
| `Internal` | 500 | `internal_error` | 否 | `public_detail() = None` |
| `Di` | 500 | `dependency_injection` | 否 | 组合根/容器缺陷，与 handler 缺陷分开过滤 |
| `Other` | 500（覆盖） | `error`（覆盖） | 否 | 无专用分类的状态，必须显式给 status |

一致性规则（`ERROR_CODES` 的文档测试与运行期函数互为校验）：

- 一个 code 只对应一个状态；`retryable` 必须与 `status_is_retryable(status)` 一致，
  唯一例外是 `conflict_retryable`（可重试的 409，条件特定）。
- code 一经发布只增不删、不改名；新增必须同时进 `ERROR_CODES` 与 `default_code_for_status`。
- 应用自定义 code 走 `AppError::new(kind, code)`，**不**进框架表。

## 4. Wire 与清洗

- 渲染：`application/problem+json`；`title` 由状态给出，`detail` 只取 `public_detail()`，
  5xx 的 `operator` 只进日志。
- 保留扩展键：`x-webx-title` / `x-webx-type` / `x-webx-instance` / `x-webx-retry-after`
  （前缀 `x-webx-`）。它们只用于 **verbatim 还原**预构造 problem、承载 429 等待值，
  **永远不得**出现在响应体或日志里；自定义 `IErrorResponseWriter` 必须用
  `webx_core::problem::extensions_for_wire`（过滤保留键），`Retry-After` 用
  `retry_after_of(error)`。
- `ProblemDetails → AppError` 把 `title`/`type`/`instance` 移入保留键，反方向
  `ProblemDetails::from_app_error(&err, path, trace_id)` 在 verbatim 模式下逐字节还原
  （含缺失的 `type` 与未清洗的 5xx detail），普通模式才派生标题并清洗 5xx。
- 429：`too_many_requests(secs, detail)` 写 `x-webx-retry-after`，默认 writer 只发一次
  `Retry-After` 头；裸 `status(429, ..)` 不写该键。

## 5. 边界转换的语义

- **DI**：所有容器失败（缺服务、keyed 未注册、循环依赖、scope 违规、工厂 panic、async/owned
  限制、provider 已释放）→ `Di` / `dependency_injection` / 500 / `public_detail() = None`，
  容器原文进 operator。
- **持久层**：业务错误（`NotFound`/`ModelValidation`/`ConcurrencyConflict`）保留调用方文案；
  驱动派生（`Query`/`TypeConversion`/`UniqueViolation`）用固定客户端文案、SQL/列/约束名只进
  operator；瞬态（`Connection`/`Transaction`、`Database` 的 SQLSTATE 08/40/57P0x、MySQL
  1205/1213、SQLite busy/locked）→ 可重试 503。唯一分类源是
  `rust_ef::database_error_is_transient`。
- **密钥**：`SecretError::NotFound` → 隐藏 500（缺密钥是部署缺陷，名字只进日志）；
  `Unavailable` → 可重试 503。

## 6. 不变量与验收

1. 代码里不再有 `Error::Variant(..)` 构造与 match（迁移表见 `upgrade-to-2.9.1.md` §1）；
2. 每个公开构造器的 `kind/code/status/retryable` 与 §3 一致（`crates/core` 契约测试）；
3. `ERROR_CODES` 与 `default_code_for_status`/`status_is_retryable` 无分歧（文档测试）；
4. 5xx 的 `public_detail() == None`；`operator` 不出现在响应体；
5. `x-webx-*` 不出现在响应体（`problem_details_test` 的 wire 等价用例）。
