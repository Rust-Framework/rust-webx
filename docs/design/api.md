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
