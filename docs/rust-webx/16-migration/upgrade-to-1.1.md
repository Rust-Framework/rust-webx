# 升级到 1.1（破坏性变更）

> 目标读者：正在使用 rust-webx 1.0.2 的项目。1.1 把响应模型对齐 ASP.NET Core 的
> `IActionResult` 族：**`ResponseData` 被删除**，需要自行掌控 HTTP 响应的端点改为
> 「响应类型写 `Response`，返回一个 action result」。
>
> 版本说明：这批破坏性变更由项目决定并入 **1.x 发布线**（发布为 1.1.0），不另开 2.0 大版本；
> 历史上 2.0 开发分支的提交已全部进入 `main`。

## 1. 依赖升级

```toml
[dependencies]
rust-webx = "1.1"
```

DI / ORM 基线不变：rust-dix 0.10、rust-ef 2.1，MSRV 1.95（见
[升级到 1.0](upgrade-to-1.0.md)）。**普通 DTO 端点无需改动**：`IRequest<UserDto>` +
`ActionResult<UserDto>` 仍是默认写法，框架内部用 `JsonResult` 写 200 JSON。

## 2. `ResponseData` → `IActionResult` 族（主要破坏点）

| 1.0.2 | 1.1.0 |
|-------|-------|
| `ResponseData::file(p)` | `FileResult::path(p)` |
| `ResponseData::file_stream(r, ct)` | `FileResult::stream(r).content_type(ct)` |
| `ResponseData::file_seekable_stream(r, len, ct)` | `FileResult::seekable_stream(r, len).content_type(ct)` |
| `ResponseData::with_file(b)` | `FileResult::with_body(b)` |
| `ResponseData::bytes(v)` / `::text(s)` / `::html(s)` | `ContentResult::bytes(v, ct)` / `::text(s)` / `::html(s)` |
| `ResponseData::no_content()` | `NoContentResult` |
| `ResponseData::json(v)` | `JsonResult::new(v)`（普通 DTO 端点自动使用） |
| `IRequest<ResponseData>` | `IRequest<Response>` |

改法示例（文件下载端点）：

```rust
use webx::*;

// 1.0.2
#[get("/api/reports/{id}/download")]
impl IRequest<ResponseData> for DownloadReportRequest {}

#[handler]
#[async_trait]
impl IRequestHandler<DownloadReportRequest, ResponseData> for DownloadReportHandler {
    async fn handle(&mut self, req: DownloadReportRequest) -> ActionResult<ResponseData> {
        Ok(ResponseData::file(self.reports.path_for(&req.id)?)
            .content_type("application/pdf")
            .download_name("报表.pdf"))
    }
}

// 1.1.0
#[get("/api/reports/{id}/download")]
impl IRequest<Response> for DownloadReportRequest {}

#[handler]
#[async_trait]
impl IRequestHandler<DownloadReportRequest, Response> for DownloadReportHandler {
    async fn handle(&mut self, req: DownloadReportRequest) -> ActionResult<Response> {
        Ok(Box::new(
            FileResult::path(self.reports.path_for(&req.id)?)
                .content_type("application/pdf")
                .download_name("报表.pdf"),
        ))
    }
}
```

`Response` 是 `Box<dyn IActionResult>` 的别名：trait object 必须装箱（Rust 没有隐式转换），
所以 handler 里固定写成 `Ok(Box::new(<result>))`。链式方法名保留在结果类型上：
`.status(u16)`、`.header(k, v)`、`.download_name(..)`、`.inline_name(..)`、
`.enable_range_processing(bool)`、`.entity_tag(..)`、`.last_modified(..)`
（后三个文件级开关只在 `FileResult` 上）。

## 3. 其它返回写法

```rust
// 201 + 带类型 body（跨轨道：声明类型变成 Response）
Ok(Box::new(JsonResult::new(dto).status(201)))

// 204
Ok(Box::new(NoContentResult))

// 仅状态码
Ok(Box::new(StatusCodeResult(202)))

// 文本 / HTML / 内存字节
Ok(Box::new(ContentResult::text("ok")))
Ok(Box::new(ContentResult::html("<h1>hi</h1>")))
Ok(Box::new(
    ContentResult::bytes(csv.into_bytes(), "text/csv; charset=utf-8").download_name("清单.csv"),
))
```

## 4. 删除的别名

- `webx::Json<T>`（无引用别名）删除，用 `JsonResult<T>`。
- `ResponseData` 类型本身连同构造器全部删除，**不保留 deprecated 别名**。

## 5. OpenAPI 行为

- 不再产生悬空 `$ref`：每个被引用的响应 DTO 名称都会生成一个占位 `components.schemas`
  条目（Rust 无运行时反射，字段需查 Rust 类型）。
- `IRequest<Response>` 端点标记为 opaque：文档只声明响应存在，状态码/头/体由 handler 运行时决定。

## 6. 升级检查清单（grep）

```bash
# 1) 旧响应类型（期望：无输出）
rg 'ResponseData' src/

# 2) 需要自控响应的端点
rg 'IRequest<Response>' src/
rg 'Box::new\((File|Json|Content|NoContent|StatusCode)Result' src/

# 3) 依赖未对齐
cargo tree -d | rg 'rust-dix'
# 期望：只有 rust-dix v0.10.x / rust-dix-macros v0.10.x
```

## 7. 验证建议

1. `cargo test` 全量通过；
2. 文件下载 / 上传端点回归：`Range`、`If-None-Match`、`Content-Disposition` 行为应与 1.0.2 一致；
3. 生成一次 OpenAPI 文档，确认 `components.schemas` 中的引用都能解析。

---

## English summary

rust-webx 1.1 aligns the response model with ASP.NET Core's `IActionResult` family and removes
`ResponseData`:

- **Action results**: `IActionResult`, `Response` (`Box<dyn IActionResult>`), `JsonResult<T>`,
  `ContentResult`, `FileResult`, `NoContentResult`, `StatusCodeResult`. Endpoints that control the
  response declare `IRequest<Response>` and return `ActionResult<Response>`; ordinary DTO endpoints
  (`IRequest<T>` + `ActionResult<T>`) are unchanged.
- **Migrations**: `ResponseData::file(p)` → `FileResult::path(p)`,
  `file_stream(r, ct)` → `FileResult::stream(r).content_type(ct)`,
  `file_seekable_stream(r, len, ct)` → `FileResult::seekable_stream(r, len).content_type(ct)`,
  `with_file(b)` → `FileResult::with_body(b)`,
  `bytes/text/html` → `ContentResult::bytes/text/html`,
  `no_content()` → `NoContentResult`.
- **Removed aliases**: the unused `webx::Json<T>` alias; no deprecated shims remain.
- **OpenAPI**: referenced response types produce placeholder `components.schemas` entries (no
  dangling `$ref`); `Response` endpoints are marked opaque.
- Version note: the breaking changes ship as **1.1.0** on the 1.x line by project decision.
