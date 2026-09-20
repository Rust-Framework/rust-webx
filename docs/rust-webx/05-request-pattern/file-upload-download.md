# 文件上传与下载

本章讲两件事：把浏览器上传的文件接进 Handler，以及把服务端的文件/字节还给浏览器。

两者都走原有的「请求即端点」写法，不需要自定义中间件，也不需要把文件内容塞进 JSON 字段。

| 能力 | 用什么 |
|------|--------|
| 接收 `multipart/form-data` | 请求结构体加 `FormFile` 字段，照常 `#[derive(Deserialize)]` |
| 返回文件（流式） | 响应类型写 `ResponseData`，返回 `ResponseData::file(...)` |
| 返回内存字节 | 响应类型写 `ResponseData`，返回 `ResponseData::bytes(...)` / `::text(...)` |

---

## 一、上传

### 1.1 声明请求

上传端点就是一个普通的 POST 端点。文件字段用 `FormFile`：

```rust
use webx::*;

#[derive(Deserialize)]
pub struct UploadAvatarRequest {
    /// 普通文本字段
    pub user_id: String,
    /// 文件字段
    pub avatar: FormFile,
}

#[post("/api/users/{user_id}/avatar")]
#[authorize]
impl IRequest<AvatarDto> for UploadAvatarRequest {}
```

Handler 和平时完全一样：

```rust
#[derive(Inject)]
pub struct UploadAvatarHandler {
    #[inject]
    storage: Arc<dyn IAvatarStorage>,
}

#[handler(inject)]
#[async_trait]
impl IRequestHandler<UploadAvatarRequest, AvatarDto> for UploadAvatarHandler {
    async fn handle(&mut self, req: UploadAvatarRequest) -> Result<AvatarDto> {
        // `user_id` 来自路由，`avatar` 来自 multipart 文件部分
        let key = format!("{}/{}", req.user_id, req.avatar.file_name());

        // 流式落盘，不在内存里拼一整份文件
        let dest = self.storage.path_for(&key);
        req.avatar.save_as(&dest).await?;

        Ok(AvatarDto {
            key,
            size: req.avatar.size(),
            content_type: req.avatar.content_type().map(str::to_string),
        })
    }
}
```

要点：

* **不需要额外派生**。请求结构体仍然只写 `#[derive(Deserialize)]`，`FormFile` 自己知道怎么从表单里取自己。
* **路由参数和表单字段可以混用**，`{user_id}` 会照常绑定。
* **`Content-Type` 决定绑定方式**：`multipart/form-data` 走表单绑定，其它情况走原有的 JSON / 路径参数绑定。

### 1.2 可选、多值字段

```rust
#[derive(Deserialize)]
pub struct UploadRequest {
    pub title: String,
    /// 可选：客户端没传时为 None
    pub cover: Option<FormFile>,
    /// 多值：同名字段重复出现即可
    pub gallery: Vec<FormFile>,
    /// 布尔值由框架从文本解析
    pub publish: bool,
}
```

文本字段支持 `String`、`bool`、整数与浮点数、`char`、`Option<T>`、`Vec<T>`，以及单元枚举
（例如把 `"disk"` 绑到 `enum Kind { Disk, Ssd }`）。同名文本字段重复出现时，标量取第一个值。

### 1.3 `FormFile` API

| 方法 | 说明 |
|------|------|
| `field_name()` | 表单字段名（如 `"avatar"`） |
| `file_name()` | **已净化**的文件名，可安全用于拼接路径 |
| `original_file_name()` | 客户端原始文件名，仅用于展示 |
| `content_type()` | 客户端声明的 MIME，属于不可信元数据 |
| `size()` / `is_empty()` | 字节数 |
| `extension()` | 小写扩展名（不含点） |
| `path()` | 落盘后的临时文件路径；小文件留在内存时为 `None` |
| `open()` | 返回 `AsyncRead`，流式读取 |
| `read_bytes()` | 一次性读入内存 |
| `copy_to(&mut writer)` | 流式写入任意 `AsyncWrite` |
| `save_as(path)` | 流式写入目标路径（先写同目录临时文件再 rename，不会留下半截文件） |

`file_name()` 会剥掉路径分隔符、控制字符与 Windows 保留字符，并且永远不会是 `.` 或 `..`，
所以 `dir.join(f.file_name())` 是安全的。原始名字请用 `original_file_name()`。

### 1.4 文件存在哪里

上传部分不会整体驻留内存：

```
内存中缓冲，直到超过 Form.MemoryThreshold（默认 1 MiB）
        │
        └──▶ 超过后写入 Form.TempDir 下的临时文件
```

* 无论大小，字节只存一份，**先落盘再拼接字符串**这种反模式不会出现。
* 临时文件由 `FormFile` 自己持有，最后一个引用释放时自动删除——也就是请求结构体被丢弃时。
* 因此：**想长期保存，就在 Handler 里调用 `save_as` / `copy_to`**。这与其他主流框架（ASP.NET Core 的
  `IFormFile`）的语义一致。
* `path()` 返回 `Some` 时可以直接把路径交给下游，做零拷贝移交。

### 1.5 错误语义

| 情况 | 结果 |
|------|------|
| 缺少必填表单字段 / 类型转换失败 | `400`，detail 里点名出错字段 |
| 单个文件超过 `Form.MaxFileSize` | `413` |
| 单个文本字段超过 `Form.MaxFieldSize` | `413` |
| 整个请求超过 `Form.MaxRequestSize` | `413` |
| 请求头里的 `Content-Length` 已超限 | `413`，**在读取上传内容之前**就拒绝 |
| 请求声明 multipart 但缺少 boundary | `400` |
| 用 JSON body 去填 `FormFile` 字段 | `400`（`FormFile` 只能来自真实的 multipart 请求） |

最后一条是安全边界：`FormFile` 无法通过 JSON 构造，所以客户端**不能**让服务端去读任意路径的文件。

### 1.6 OpenAPI 里的上传端点

给请求结构体加 `#[derive(WebxRequestMeta)]`，上传端点就会在 OpenAPI 里被描述成
`multipart/form-data`，而不是错误的 JSON body：

```rust
#[derive(Deserialize, WebxRequestMeta)]
pub struct UploadAvatarRequest {
    pub user_id: String,
    pub avatar: FormFile,
    pub gallery: Vec<FormFile>,
}
```

生成的 `requestBody`：

```json
{
  "content": {
    "multipart/form-data": {
      "schema": {
        "type": "object",
        "properties": {
          "user_id": { "type": "string" },
          "avatar":  { "type": "string", "format": "binary" },
          "gallery": { "type": "array", "items": { "type": "string", "format": "binary" } }
        },
        "required": ["user_id", "avatar", "gallery"]
      }
    }
  }
}
```

规则很简单：**结构体里出现 `FormFile` 字段时，未标注的字段都视为表单字段**
（与 `#[webx_request(query_all)]` 把未标注字段视为 query 参数同理）。
也可以用 `#[from_form]` 显式标注。

---

## 二、下载

### 2.1 `ResponseData`：自己掌控响应

绝大多数 Handler 返回可序列化的 DTO，框架写 JSON。需要自己决定状态码、响应头与响应体时，
把响应类型写成 `ResponseData`：

```rust
use webx::*;

#[derive(Default, Deserialize)]
pub struct DownloadReportRequest {
    pub id: String,
}

#[get("/api/reports/{id}/download")]
impl IRequest<ResponseData> for DownloadReportRequest {}

#[handler]
#[async_trait]
impl IRequestHandler<DownloadReportRequest, ResponseData> for DownloadReportHandler {
    async fn handle(&mut self, req: DownloadReportRequest) -> Result<ResponseData> {
        let path = self.reports.path_for(&req.id)?;
        Ok(ResponseData::file(path)
            .content_type("application/pdf")
            .download_name("季度报表.pdf"))
    }
}
```

浏览器会得到 `Content-Disposition: attachment; filename="..."; filename*=UTF-8''...`，
非 ASCII 文件名通过 RFC 5987 的 `filename*` 原样保留。

### 2.2 `File(...)` 的三种来源

对应 ASP.NET Core 的 `File(...)` / `PhysicalFile(...)` / `Results.File(...)`，
按你手上**已经有什么**来选，都不需要先把内容整体读进内存：

| 你有的东西 | 构造方法 | 对应 ASP.NET Core |
|-----------|----------|-------------------|
| 磁盘路径 | `ResponseData::file(path)` | `PhysicalFile(path, ...)` |
| 未知长度的异步读取器 | `ResponseData::file_stream(reader, ct)` | `File(Stream, ...)` |
| 已知长度且可 seek 的读取器 | `ResponseData::file_seekable_stream(reader, len, ct)` | `File(Stream, ...)` + `enableRangeProcessing` |
| 内存字节 | `ResponseData::bytes(v)` / `::text(s)` | `File(byte[], ...)` / `Results.Bytes` |
| 完全自定义 | `ResponseData::with_file(FileBody::...)` | 手写 `IActionResult` |

流式下载（内容边生成边发送，总量未知）：

```rust
#[handler]
#[async_trait]
impl IRequestHandler<ExportZipRequest, ResponseData> for ExportZipHandler {
    async fn handle(&mut self, req: ExportZipRequest) -> Result<ResponseData> {
        // duplex() 把「写端」交给生产者任务，「读端」直接交给响应。
        let (reader, writer) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move { self.zip_into(writer, req).await });
        Ok(ResponseData::file_stream(reader, "application/zip")
            .download_name("导出.zip"))
    }
}
```

对象存储 / 数据库 LOB 这类**能 seek、也知道长度**的句柄，用
`file_seekable_stream` 就能同时拿到精确的 `Content-Length` 和断点续传：

```rust
let object = store.get(&key).await?;          // 实现 AsyncRead + AsyncSeek
Ok(ResponseData::with_file(
    FileBody::seekable_stream(object, len)
        .content_type("video/mp4")
        .entity_tag(etag)                     // 让客户端能发 If-None-Match / If-Range
        .download_name("录像.mp4"),
))
```

### 2.3 构造方法与文件级开关

| 构造 | 响应体 | 默认 `Content-Type` |
|------|--------|---------------------|
| `ResponseData::json(&value)` | `serde_json` 序列化 | `application/json` |
| `ResponseData::text(s)` | UTF-8 文本 | `text/plain; charset=utf-8` |
| `ResponseData::html(s)` | UTF-8 文本 | `text/html; charset=utf-8` |
| `ResponseData::bytes(v)` | 二进制（内存） | `application/octet-stream` |
| `ResponseData::no_content()` | 空 | —（状态 204） |
| `ResponseData::file(path)` | 流式发送文件 | 按扩展名推断 |
| `ResponseData::file_stream(r, ct)` | 流式转发读取器 | 显式指定 |
| `ResponseData::file_seekable_stream(r, len, ct)` | 流式转发 + 支持 Range | 显式指定 |
| `ResponseData::with_file(FileBody)` | 全部可控 | 按来源推断 |

`ResponseData` 的链式方法：

| 方法 | 作用 |
|------|------|
| `.status(404)` | 覆盖状态码 |
| `.content_type("text/csv; charset=utf-8")` | 覆盖媒体类型 |
| `.header("cache-control", "private, max-age=60")` | 追加响应头 |
| `.download_name("导出.csv")` | 触发「另存为」，并设置文件名 |
| `.inline_name("预览.png")` | 内联展示，并设置文件名 |
| `.enable_range_processing(false)` | 关闭 Range（仅对文件响应有效） |
| `.entity_tag("\"v1\"")` | 显式 `ETag`（文件响应写入校验器，其它写入响应头） |
| `.last_modified(t)` | 显式 `Last-Modified` |

`FileBody` 上还有同样的三个文件级开关，用 `with_file` 时直接写：

```rust
FileBody::path(p)
    .content_type("application/pdf")
    .enable_range_processing(false)   // 不希望客户端切片拉取
    .entity_tag("\"report-v3\"")
    .last_modified(system_time)
    .download_name("报表.pdf")
```

### 2.4 文件响应自带的 HTTP 能力

| 能力 | 路径 / 可 seek 流 | 未知长度流 |
|------|------------------|-----------|
| 发送方式 | 流式，64 KiB 分块 | 流式，64 KiB 分块 |
| `Content-Length` | ✅ 精确 | ❌（使用 chunked 传输编码） |
| `ETag` | ✅ 由大小+mtime 推导，或显式指定 | 仅显式指定时 |
| `Last-Modified` | ✅ | 仅显式指定时 |
| `Accept-Ranges` | `bytes` | `none` |
| `Range`（`206`/`416`，含 `multipart/byteranges`） | ✅ | ❌ |
| `If-Range` / `If-None-Match` / `If-Modified-Since`（`304`） | ✅ | 有校验器时 |
| `HEAD` | ✅ GET 路由自动响应，头部一致、无响应体 | ✅ |
| 文件不存在 | `404`（RFC 7807 problem+json） | — |

区间请求按 RFC 7233 完整实现，而不只是「遇到 `Range` 就切一段」：

| 请求 | 响应 |
|------|------|
| 单个区间 `bytes=0-99` | `206` + `Content-Range` |
| 多个不相交区间 `bytes=0-9,20-29` | `206` + `Content-Type: multipart/byteranges`（精确 `Content-Length`） |
| 重叠/相邻/重复区间 `bytes=0-99,50-149,0-99` | 先合并成不相交集合，再按上面两条处理 |
| 区间过多（合并后仍 > 8 段） | 返回完整响应体（RFC 7233 §3.1 允许） |
| 部分区间不可满足 `bytes=0-3,9000-9001` | 丢弃不可满足的成员，返回可满足的部分 |
| 全部不可满足 | `416` + `Content-Range: bytes */<len>` |
| 语法非法（任一成员） | 忽略整个 `Range` 头（RFC 7233 §2.1） |
| 不可 seek 的来源 | 忽略 `Range`，返回完整响应体，并声明 `Accept-Ranges: none` |

**「先合并」不只是整洁问题，而是安全问题**：不做合并，客户端可以请求 `bytes=0-,0-,0-,…`
八次，让服务端把同一个文件发八遍。合并后各段互不相交，响应体永远不可能大于文件本身。

### 2.5 断点续传

客户端（下载器、`aria2`、`curl -C -`、浏览器视频播放器）遵循这样一个契约：

```bash
# 1. 先探一次，拿到长度与校验器
curl -I https://example.com/video.mp4
# → Content-Length: 734003200
#   ETag: "2bc0e00-18d5a82de3e3ec00"
#   Accept-Ranges: bytes

# 2. 从中断处继续；If-Range 保证文件没变才接受续传
curl -C - -H 'If-Range: "2bc0e0-18d5a82de3e3ec00"' -o video.mp4 https://example.com/video.mp4
```

服务端为此提供：

| 机制 | 作用 |
|------|------|
| `ETag` + `Last-Modified` | 客户端判断手里的副本是否还是同一版本 |
| `Accept-Ranges: bytes` / `none` | 明确告知能否续传，客户端不必试探 |
| `If-Range` | **强比较**：只有校验器逐字节一致才返回 `206`；否则返回完整响应体 |
| `Range` 合并与上限 | 保证范围请求不会放大流量 |
| `416` + `Content-Range: bytes */<len>` | 文件变小（被截断）时让客户端重新开始 |
| `304` | 文件未变时省掉整个响应体 |

关于 `If-Range` 为什么必须严格：客户端是从**某个版本**的中断点继续的。如果文件在这期间
被替换，把新文件的第 N 字节接在旧文件的前 N 字节后面，会得到一个静默损坏的文件。
所以 `If-Range` 用强比较——弱 `ETag`（`W/"…"`）永不匹配，宁可重传整个文件。

服务端自动生成的 `ETag` 由「大小 + 修改时间（纳秒精度）」推导，并作为**强**校验器发出，
因为断点续传需要强校验器。需要逐字节精确的校验器（例如内容哈希）时用
`.entity_tag("\"sha256-…\"")` 显式指定。文件 mtime 落在未来时（归档恢复、时钟偏移）
不会发送 `Last-Modified`，避免 `If-Modified-Since` 与之矛盾。

### 2.6 下载内存里的内容

因为有了 `ETag` 与 `Range`，大文件可以断点续传，视频可以拖动进度条，客户端也能用
`If-None-Match` 省掉重复传输。

> **缓存策略**：普通 API 响应默认 `Cache-Control: no-store`；文件响应默认
> `public, max-age=0, must-revalidate`——否则刚发出的 `ETag` 永远用不上。
> 两者都只在响应没有设过 `cache-control` 时生效，`.header("cache-control", ...)`
> 始终优先。
>
> `ResponseData::bytes` / `::text` 是即时生成的内存内容，框架不会替你计算校验器；
> 需要断点续传或条件请求的内容请落到文件，或用 `file_seekable_stream`。

### 2.5 下载内存里的内容

导出的 CSV、动态生成的图片等，直接给字节即可：

```rust
#[handler]
#[async_trait]
impl IRequestHandler<ExportInventoryRequest, ResponseData> for ExportInventoryHandler {
    async fn handle(&mut self, _: ExportInventoryRequest) -> Result<ResponseData> {
        let csv = self.render_csv().await?;
        Ok(ResponseData::bytes(csv.into_bytes())
            .content_type("text/csv; charset=utf-8")
            .download_name("清单.csv"))
    }
}
```

### 2.7 先鉴权再下载

下载端点和别的端点一样受 `#[authorize]` 保护——这是把文件放在 `wwwroot` 之外、
由 Handler 决定是否放行的主要理由：

```rust
#[get("/api/documents/{id}/download")]
#[authorize(permission = "documents.read")]
impl IRequest<ResponseData> for DownloadDocumentRequest {}
```

> 用 `Authorization` 头传令牌的 SPA 无法用普通 `<a href>` 下载（链接带不上请求头）。
> 这是浏览器的限制，与框架无关：前端应带上令牌 `fetch`，再把响应转成 `Blob` 触发保存。
> 服务端仍然负责媒体类型与文件名，前端从 `Content-Disposition` 里读名字即可。

---

## 三、配置

`multipart/form-data` 的上传限制独立于 `App.MaxBodySize`：JSON 端点应当收得很紧，
上传端点则天然需要更大。声明 multipart 的请求按 `Form` 节度量，其它请求按 `App.MaxBodySize`。

```json
{
  "App": {
    "MaxBodySize": 10485760
  },
  "Form": {
    "MaxRequestSize": 268435456,
    "MaxFileSize": 134217728,
    "MaxFieldSize": 1048576,
    "MemoryThreshold": 1048576,
    "TempDir": "uploads-tmp"
  }
}
```

| 键 | 默认值 | 含义 |
|----|--------|------|
| `MaxRequestSize` | 256 MiB | 整个 multipart 请求的上限 |
| `MaxFileSize` | 128 MiB | 单个文件部分的上限 |
| `MaxFieldSize` | 1 MiB | 单个文本字段的上限 |
| `MemoryThreshold` | 1 MiB | 超过此大小即落盘；设得很大则全部留在内存 |
| `TempDir` | 系统临时目录下的 `webx-uploads` | 落盘目录；相对路径按应用基准目录解析 |

也可以用环境变量覆盖，例如 `APP__Form__MaxFileSize`。

代码里同样可以改：

```rust
Host::builder()
    .configure(|app| {
        app.useOptions(|o| {
            o.form.max_file_size = 32 * 1024 * 1024;
        });
    })
    .build();
```

---

## 四、在中间件里使用

中间件拿到的是 `IHttpContext`，同样可以读表单、写文件响应：

```rust
#[async_trait]
impl IMiddleware for ImportMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        if !ctx.request().is_multipart() {
            return Ok(ControlFlow::Continue(()));
        }

        // 解析结果缓存在请求上，重复调用不会重复解析
        let form = ctx.request_mut().multipart().await?;
        if let Some(file) = form.file("config") {
            tracing::info!("收到 {} ({} 字节)", file.file_name(), file.size());
        }

        // 直接吐一个文件
        // ctx.response_mut().write_body(FileBody::new("report.csv").into()).await?;
        Ok(ControlFlow::Break(()))
    }
}
```

`MultipartForm` 提供 `text(name)` / `texts(name)` / `file(name)` / `files_named(name)` / `contains(name)`。

请求体只能被消费一次：已按 JSON 读过之后再要 `multipart()` 会返回明确错误，反之亦然，
不会静默返回空 body。

---

## 五、流式与性能

整条链路都是流式的，**峰值内存与文件大小无关**。

### 5.1 上传

```
socket ──▶ hyper Incoming ──▶ multer 逐块解析 ──▶ FormFileBuilder
                                                      │
                                      ┌───────────────┴───────────────┐
                                      │                               │
                              ≤ MemoryThreshold                 > MemoryThreshold
                              内存缓冲（一次分配）               64 KiB BufWriter ──▶ 临时文件
```

* 请求体**不预先读入内存**。只有 Handler 真的需要时才消费，而 `multipart()` 是逐块解析。
* 落盘写入包了一层 64 KiB `BufWriter`。`tokio::fs::File` 的每一次写都是一次阻塞线程池调度，
  直接逐块写会让每个传输分片都付一次线程切换；缓冲后每 64 KiB 才调度一次。
* 超过 `MaxFileSize` / `MaxFieldSize` / `MaxRequestSize` 立即中止，不会把整个请求收完再拒绝。
* 请求头里的 `Content-Length` 已经超限时，**在读上传内容之前**就返回 `413`。
* 小文件留在内存里，用引用计数的 `Bytes` 承载：`open()` 不再复制这几十 KB 到几百 KB。

### 5.2 下载

```
文件 / 流 ──▶ ReaderStream(64 KiB) ──▶ hyper ──▶ socket
```

* 统一使用 64 KiB 读缓冲（框架常量 `STREAM_BUF_SIZE`）。`ReaderStream` 的默认值是 4 KiB，
  1 GiB 会变成约 26 万次 `read` + 26 万次 socket 写入；64 KiB 把两者都降到 1/16。
* 路径来源只 `open` 一次，长度与 mtime 从同一个句柄上取（`File::open` + `File::metadata`），
  不再 `metadata()` 之后再 `open()`。
* `Range` 请求用 `seek` 定位后只读需要的字节，不会为了发 4 KiB 而把整个文件过一遍。
* 多区间请求复用同一个文件句柄，逐段 `seek` 读取；每段各自分块，不会把整段读进内存。
* 未知长度的流直接 chunked 转发，不做任何缓冲。

### 5.3 内存开销

| 场景 | 额外内存 |
|------|----------|
| 上传 10 GB 文件，`MemoryThreshold = 1 MiB` | ≈ 1 MiB + 64 KiB 写缓冲 |
| 上传 100 KB 文件 | ≈ 100 KB（留在内存） |
| 下载 10 GB 文件（路径） | 64 KiB 读缓冲 |
| 下载未知长度流 | 64 KiB 读缓冲 |
| `ResponseData::bytes(v)` | 整个 `v`（本来就已在内存里） |

### 5.4 明确的边界

* **没有 `sendfile` 零拷贝。** hyper 1.x 把连接 IO 封装在内部，用户态代码拿不到底层 socket，
  无法调用 `sendfile(2)`。因此「文件 → 响应体 → socket」会有一次用户态拷贝。
  这一条对所有基于 hyper 的框架都成立（Axum/tower-http 同样如此），不是本框架的取舍。
  真正的零拷贝需要自己在 hyper 之下接管连接，代价是失去中间件与路由能力。
* **HTTPS 下 `sendfile` 本来也用不上**，因为需要先加密，必须经过用户态。

### 5.5 实践建议

* 大文件**不要**用 `read_bytes()` 读进内存再 `bytes()` 返回；用 `file()` /
  `file_stream()` / `file_seekable_stream()`。
* 从对象存储回源时，如果 SDK 能返回可 seek 的读取器，用 `file_seekable_stream`——
  断点续传能让客户端少下很多字节。
* 把 `MemoryThreshold` 调到比典型小文件略大一点，可以避免为头像这类请求碰磁盘。
* 反代（nginx）放在前面时，把 `proxy_buffering` 和上游 `Content-Length` 的关系想清楚；
  未知长度的 chunked 响应在反代上默认会被缓冲。

---

## 六、安全清单

1. **永远不要用 `original_file_name()` 拼路径**，用 `file_name()`（已净化）。
2. **不要相信 `content_type()`**——它由客户端声明。要校验就查扩展名白名单，或者嗅探文件头。
3. **按需调低限制**：默认值面向通用场景，图片上传可以把 `MaxFileSize` 收到几 MiB。
4. **落盘目录要可写且定期清理**：未及时 `save_as` 的临时文件在请求结束时删除；
   已 `save_as` 的文件由你的应用负责生命周期。
5. **敏感文件不要放 `wwwroot`**：那里的文件不经过授权检查。用 `ResponseData::file` + `#[authorize]`。

---

## 七、与其它框架对照

| 场景 | ASP.NET Core | FastAPI | rust-webx |
|------|--------------|---------|-----------|
| 接收文件 | `IFormFile` | `UploadFile` | `FormFile` |
| 多文件 | `List<IFormFile>` | `list[UploadFile]` | `Vec<FormFile>` |
| 保存到磁盘 | `CopyToAsync(stream)` | `shutil.copyfileobj` | `save_as(path)` / `copy_to(&mut w)` |
| 流式读取 | `OpenReadStream()` | `UploadFile.file` | `open()` |
| 单文件大小限制 | `FormOptions.MultipartBodyLengthLimit` | 手写 | `Form.MaxFileSize` |
| 内存/落盘阈值 | `FormOptions.MemoryBufferThreshold` | `SpooledTemporaryFile` | `Form.MemoryThreshold` |
| 返回路径文件 | `PhysicalFile(...)` | `FileResponse(...)` | `ResponseData::file(...)` |
| 返回流 | `File(Stream, ...)` | `StreamingResponse(...)` | `ResponseData::file_stream(...)` |
| 返回可 seek 流 | `File(Stream, ...)` + `enableRangeProcessing` | 手写 | `ResponseData::file_seekable_stream(...)` |
| 返回内存字节 | `File(byte[], ...)` / `Results.Bytes` | `Response(content=...)` | `ResponseData::bytes(...)` |
| 下载文件名 | `fileDownloadName` | `filename=` | `.download_name(...)` |
| 范围请求 | `enableRangeProcessing` | 内建 | 内建（含 `multipart/byteranges`） |
| 断点续传 | `If-Range` + `ETag` | `If-Range` | 内建（强校验器 + `If-Range`） |
| 显式校验器 | `lastModified` / `entityTag` | `headers=` | `.last_modified(...)` / `.entity_tag(...)` |

---

## 小结

* 上传：请求结构体里写 `FormFile`，照常 `#[derive(Deserialize)]`，不需要额外派生或中间件。
* 文件流式接收、按阈值落盘、自动清理，内存占用与文件大小无关。
* 下载：响应类型写 `ResponseData`；按手上有的东西选 `file` / `file_stream` /
  `file_seekable_stream`，对应 ASP.NET Core 的 `PhysicalFile` 与 `File(Stream, ...)`。
* 文件响应自带 `ETag` / `Last-Modified` / `Range` / `304` / `HEAD`，无需手工实现。
* 断点续传开箱可用：`Accept-Ranges`、强校验器、`If-Range` 强比较、
  `multipart/byteranges`、区间合并与上限、`416` 语义都已就位。
* 限制集中在 `Form` 配置节，与 JSON 端点的 `App.MaxBodySize` 相互独立。

下一节：[错误处理与 ProblemDetails](error-handling.md)
