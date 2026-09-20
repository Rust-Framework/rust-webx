# Changelog

All notable changes to **rust-webx** are documented in this file.


## [0.5.0] — 2026-09-20

> Upgrading from 0.4 or earlier: see
> [`docs/rust-webx/16-migration/upgrade-to-0.5.md`](docs/rust-webx/16-migration/upgrade-to-0.5.md),
> which walks through every breaking change below with verification steps.

### Breaking

- **Import path**: the crate **library names** are now `webx` / `webx_core` /
  `webx_host` / `webx_macros` / `webx_spa` / `webx_openapi`. Package names are
  unchanged (`rust-webx`, `rust-webx-core`, …), so `Cargo.toml` needs no edit —
  only `use` paths and macro paths do.

  ```bash
  # Only the underscore form is a code path; dashed package names are unaffected.
  grep -rl 'rust_webx' --include='*.rs' . | xargs sed -i 's/rust_webx_build/webx/g; s/rust_webx/webx/g'
  grep -rl 'RUST_WEBX' --include='*.rs' . | xargs sed -i 's/RUST_WEBX/WEBX/g'
  ```

  | Item | Before | After |
  |------|--------|-------|
  | Umbrella import | `use rust_webx::*;` | `use webx::*;` |
  | Sub-crate import | `rust_webx_core::…` | `webx_core::…` |
  | Entry macro | `#[rust_webx::main]` | `#[webx::main]` |
  | Embed macro | `rust_webx::spa::embed_assets!()` | `webx::spa::embed_assets!()` |
  | Build-time crate | `rust_webx_build::embed_assets("wwwroot")` | `webx::builder().web_root("wwwroot").build()` |
  | Generated file | `rust_webx_embedded_assets.rs` | `webx_embedded_assets.rs` |

- **Environment variables**: `RUST_WEBX_APP_BASE` → `WEBX_APP_BASE`;
  `RUST_WEBX_EMBED` → `WEBX_EMBED`.
- **SPA entry**: `HostBuilder::embed()` removed. `build.rs` writes the table with
  `webx::builder().web_root(...).build()` and the entry point links it with
  `#[webx::main(embed)]`; plain `#[webx::main]` never bakes assets. Integration
  tests that never run `fn main` register the same table with
  `#[webx::embed_assets]`.
- **Embedded assets are stored compressed** (see Added). Code that reads
  `EmbeddedAsset` fields directly must account for `raw_len` and `encoding`:
  `bytes` is the stored payload, which is only the file itself when `encoding`
  is `Identity`.

### Added

- **Compressed embedded assets**: `build.rs` brotli-encodes every compressible
  file (quality 11) and the SPA middleware hands the stored stream to clients
  that accept `Content-Encoding: br`, decoding once and caching it for clients
  that do not. Media that is already compressed (PNG, WOFF2, ZIP, …) and files
  under 256 bytes stay verbatim. On docbit's 27 MiB `wwwroot` this removes ~80%
  of the embedded footprint — and because static files were never compressed on
  the wire, they are now smaller in transit too.
- **Per-representation `ETag`s**: an embedded file served as brotli carries
  `<etag>-br`, and only responses that can vary by encoding send
  `Vary: Accept-Encoding`.
- **Migration guide**: [`docs/rust-webx/16-migration/upgrade-to-0.5.md`](docs/rust-webx/16-migration/upgrade-to-0.5.md)
  documents the import rename, the environment variables, the SPA embed model and
  the new payload encoding, with a verification checklist and a troubleshooting
  table.

- **Docbit — sixth work (`rust-agent-flow`)**: the `rust-flow` repository is now
  part of the portfolio. Its documentation slug is `rust-agent-flow`, so the
  wiring names it explicitly everywhere it appears: `DocService::WORK_SLUGS`,
  `sibling_doc_relative` (`rust-flow/docs/rust-agent-flow`), the seed exhibition
  row, and both `scripts/copy-ecosystem-docs` variants — the publish bundle now
  carries six doc trees instead of five.
- **Docbit — documentation bundle upload**: `POST /api/works/{slug}/docs`
  (admin-only) installs an uploaded zip as `<app_base>/docs/{slug}` and re-syncs
  the catalog in the same request. Extraction is sandboxed against path
  traversal, symlink entries and zip bombs; the install is an atomic swap with
  rollback, so the served documentation is never left half-replaced. Works with
  no seed template are created from their `INDEX.json`.
- **Docbit — blog media**: `POST /api/media` (authenticated) stores images and
  attachments under `<app_base>/uploads/`, served back by `GET /api/media/{*path}`.
  Raster images are inline; everything else — notably SVG — is sent as a download
  so it cannot execute on this origin. Vditor's `upload`/`insert` toolbar entries
  are wired to it.
- **Docbit — admin UI**: a "上传文档" action on each 作品管理 row, with progress
  and a post-upload summary.

### Changed

- **SPA embed**: `Host::build` layers the compiled-in table under `.use_spa(...)`
  automatically once the table is linked; `.use_spa` is the runtime disk overlay.
- **Build script API**: `webx::builder().web_root(...).build()` (the
  `rust-webx-build` package exposes the `webx` library name).
- **`EmbeddedAsset`** gained `raw_len` and `encoding`
  (`ContentEncoding::Identity` / `Brotli`). `EmbeddedAssets::total_bytes()` is now
  the stored payload size and `total_raw_bytes()` the uncompressed size; the host
  startup log reports both.

### Fixed

- **Docbit admin password**: the built-in `admin@docbit.local` / `admin123`
  account was created on every boot with no environment gate, and the password
  was written to the log. Production now provisions the admin only from
  `DOCBIT_ADMIN_PASSWORD` and never falls back to a built-in password.
- **`rust-webx-build` is now published.** The documented
  `[build-dependencies] rust-webx-build = "…"` dependency resolves; the crate was
  missing from the release lists (`publish.sh` and `.github/workflows/publish.yml`).
- **`cargo doc --workspace` no longer collides.** `rust-webx-build` shares the
  library name `webx` with the umbrella crate, so it is marked `doc = false`
  (it is a build-script helper with no runtime API to browse).
- **Crate-level rustdoc**: the crate roots now use `//!` docs (previously `//`
  comments, which left every docs.rs landing page without a description).
- **`publish.sh --check`** reports a dependent crate whose internal dependency is
  not yet on crates.io as "packages OK; dry-run skipped" instead of a hard
  failure, and its pre-flight now mirrors the CI gates (`fmt`, `clippy -D warnings`,
  workspace tests).


## [0.4.0] — 2026-09-16 — 文件上传与下载基础设施

> **English** · **简体中文**

### English

#### Added

- **Uploads**: `multipart/form-data` binding into typed request structs. A request
  field declared as `FormFile` is filled from the matching file part; text fields,
  `bool`/numeric/`Option`/`Vec` and unit enums bind as usual. No extra derive and no
  custom middleware — `#[derive(Deserialize)]` is all a request struct needs.
- **Uploads**: file parts stream into a `FormFileBuilder` that buffers in memory up to
  `Form.MemoryThreshold` and then spools to `Form.TempDir`. Peak memory is independent
  of upload size, and spooled files are deleted when the request struct is dropped.
- **Uploads**: `FormFile` API — `file_name()` (sanitised), `original_file_name()`,
  `content_type()`, `size()`, `extension()`, `path()`, `open()`, `read_bytes()`,
  `copy_to()`, `save_as()` (atomic write).
- **Downloads**: `ResponseData` as a response type gives a handler full control of
  status, headers and body: `json` / `text` / `html` / `bytes` / `file` / `no_content`,
  plus `status()` / `content_type()` / `header()` / `download_name()` / `inline_name()`.
- **Downloads**: the `File(...)` family, mirroring ASP.NET Core. A response can come
  from a path (`ResponseData::file`), an async reader of unknown length
  (`ResponseData::file_stream`, sent chunked), or a seekable reader of known length
  (`ResponseData::file_seekable_stream`, which keeps exact `Content-Length` and
  `Range`). `ResponseData::with_file(FileBody)` exposes the file-level knobs:
  `enable_range_processing`, `entity_tag`, `last_modified`.
- **Downloads**: `ResponseData::file` is streamed and automatically carries
  `Content-Length`, `ETag`, `Last-Modified`, `Accept-Ranges` and a RFC 6266/5987
  `Content-Disposition`, and honours `Range` (`206`/`416`), `If-Range`,
  `If-None-Match`/`If-Modified-Since` (`304`) and `HEAD`.
- **Resumable downloads**: `Range` handling is RFC 7233-correct end to end. Ranges are
  coalesced — overlapping, adjacent and duplicate members merge — so a range request
  can never amplify traffic beyond the file size; up to eight disjoint parts are
  answered as `multipart/byteranges` with an exactly computed `Content-Length`; more
  than that falls back to the full representation. Unsatisfiable members are dropped,
  an entirely unsatisfiable set is `416` with `Content-Range: bytes */<len>`, and a
  syntactically invalid member invalidates the whole header (§2.1).
- **Resumable downloads**: correct validator semantics. `If-Range` uses a **strong**
  comparison as RFC 7233 §3.2 requires (a weak tag never matches), while
  `If-None-Match` uses the weak comparison RFC 7232 §3.2 requires. `ETag`s carry full
  mtime precision, so an in-place edit that preserves the file size is still detected;
  `Last-Modified` is suppressed when the mtime is in the future (RFC 7232 §2.2.2); and
  `Accept-Ranges: none` is sent when ranges are unavailable, so download managers stop
  probing instead of guessing.
- **Performance**: one shared 64 KiB `STREAM_BUF_SIZE` for streamed responses and
  spooled uploads (the previous `ReaderStream` default of 4 KiB cost ~16x more
  reads and socket writes per gigabyte); file responses `open()` once and take
  length/mtime from the same handle instead of `metadata()` + `open()`; spooled
  uploads write through a 64 KiB `BufWriter` so `tokio::fs` no longer dispatches
  to the blocking pool once per transport chunk; memory-backed uploads are held in
  a reference-counted `Bytes` so `FormFile::open` no longer copies them;
  `FormFile::copy_to` uses the 64 KiB buffer instead of `tokio::io::copy`'s 8 KiB.
- **Routing**: `HEAD` now falls back to the `GET` route for the same path, so download
  endpoints answer `HEAD` with the same headers and no body.
- **Static files**: `SpaMiddleware` hands files to the host as file bodies, so
  `wwwroot` assets are streamed with the same validators and range support, get MIME
  types from a full extension table, and no longer buffer into memory. It also
  overrides the default `cache-control: no-store` with a caching policy.
- **Config**: new `Form` section (`MaxRequestSize`, `MaxFileSize`, `MaxFieldSize`,
  `MemoryThreshold`, `TempDir`). Multipart requests are measured against it; other
  requests keep using `App.MaxBodySize`. A declared `Content-Length` above the limit
  is refused with `413` before the upload is read.
- **Errors**: `Error::PayloadTooLarge` → `413` and `Error::UnsupportedMediaType` → `415`.
- **Docs**: new chapter [文件上传与下载](docs/rust-webx/05-request-pattern/file-upload-download.md).

#### Changed

- **(breaking)** `IHttpRequest::body_bytes` / `body_text` take `&mut self`: reading the
  body consumes it, and reading it twice now reports an error instead of returning an
  empty body. `read_json_body` takes `&mut dyn IHttpRequest`.
- **(breaking)** `IHttpRequest` gained `content_type()`, `is_multipart()` and
  `multipart()`; `IHttpResponse` gained `write_body(ResponseBody)`. Both have defaults,
  so existing implementations keep compiling.
- **(breaking)** `ResponseData.body` is now a `ResponseBody` (`Bytes` or `File`) and
  `ResponseData` gained a `headers` field; use the constructors instead of a struct
  literal.
- **(breaking)** `IRequest<R>` / `IRequestHandler<T, R>` / `IMediator::send` no longer
  require `R: Serialize`; the bound is enforced where the value is serialized. This is
  what allows `IRequest<ResponseData>`.
- The route dispatch function now receives `&mut dyn IHttpContext` instead of the
  pre-read body, route/query maps and claims, so endpoints can stream the body.
- Request bodies are no longer read eagerly; they stay a live stream until the endpoint
  asks for them.
- `IHttpResponse` no longer defaults to `Content-Type: application/json`; writers set it
  explicitly (all built-in endpoints already did).
- `Cache-Control` defaulting moved from `SecurityHeadersMiddleware` into response
  finalisation, so it can depend on what the response is: `no-store` for ordinary
  responses, `public, max-age=0, must-revalidate` for file responses (an `ETag` paired
  with `no-store` could never be used). Either way an explicit `cache-control` set by
  the application always wins.
- Streamed responses are boxed as `UnsyncBoxBody`, so a body only has to be `Send` —
  requiring `Sync` would rule out most streaming readers.
- `HttpStatus` gained `PARTIAL_CONTENT`, `NOT_MODIFIED`, `PAYLOAD_TOO_LARGE`,
  `UNSUPPORTED_MEDIA_TYPE` and `RANGE_NOT_SATISFIABLE`.

#### Fixed

- **security**: `rustls` bumped to `0.23.45` and `rustls-webpki` to `0.103.15` for
  RUSTSEC-2026-0285 (TLS 1.3 handshake messages could be accepted across encryption
  level boundaries).
- `crates/spa` MIME lookup replaced with the full extension table (`.csv`, `.md`,
  `.webp`, `.mp4`, … no longer fell through to `application/octet-stream`).
- Static assets are no longer forced to `cache-control: no-store`.
- `docs/rust-webx/13-extensibility/custom-endpoints.md` no longer claims a custom
  `IEndpoint` can be registered through `register()`; it documents the two supported
  escape hatches.

#### Reference apps

- **dmbit**: inventory export is a real `text/csv` download with a
  `Content-Disposition` filename; inventory import is a real `multipart/form-data`
  upload (`file` + `confirm_update`) instead of a JSON-encoded CSV string.

### 简体中文

#### 新增

- **上传**：`multipart/form-data` 绑定到类型化请求结构体。字段声明为 `FormFile`
  即可接收对应文件部分；文本、布尔、数值、`Option`、`Vec` 与单元枚举照常绑定。
  请求结构体只需 `#[derive(Deserialize)]`，无需额外派生或中间件。
- **上传**：文件部分以流式写入 `FormFileBuilder`，超过 `Form.MemoryThreshold`
  即落盘到 `Form.TempDir`。峰值内存与文件大小无关，临时文件随请求结构体释放自动删除。
- **上传**：`FormFile` 提供 `file_name()`（已净化）、`original_file_name()`、
  `content_type()`、`size()`、`extension()`、`path()`、`open()`、`read_bytes()`、
  `copy_to()`、`save_as()`（原子写入）。
- **下载**：响应类型写 `ResponseData` 即可完全掌控状态码、响应头与响应体：
  `json` / `text` / `html` / `bytes` / `file` / `no_content`，配合
  `status()` / `content_type()` / `header()` / `download_name()` / `inline_name()`。
- **下载**：对齐 ASP.NET Core 的 `File(...)` 家族。响应内容可以来自磁盘路径
  （`ResponseData::file`）、未知长度的异步读取器（`ResponseData::file_stream`，
  以 chunked 发送）、或已知长度且可 seek 的读取器
  （`ResponseData::file_seekable_stream`，保留精确 `Content-Length` 与 `Range`）。
  `ResponseData::with_file(FileBody)` 暴露文件级开关：
  `enable_range_processing`、`entity_tag`、`last_modified`。
- **下载**：`ResponseData::file` 流式发送，自动带 `Content-Length`、`ETag`、
  `Last-Modified`、`Accept-Ranges` 与符合 RFC 6266/5987 的 `Content-Disposition`，
  并支持 `Range`（`206`/`416`）、`If-Range`、`If-None-Match`/`If-Modified-Since`
  （`304`）与 `HEAD`。
- **断点续传**：`Range` 处理端到端符合 RFC 7233。区间先做合并——重叠、相邻、
  重复的成员都会合并——因此范围请求不可能放大流量到超过文件本身；最多 8 个不相交
  区间会以 `multipart/byteranges` 应答，`Content-Length` 精确计算；超过上限则退回
  完整响应。不可满足的成员被丢弃，全部不可满足时返回 `416` 与
  `Content-Range: bytes */<len>`；语法非法的成员会使整个头失效（§2.1）。
- **断点续传**：校验器语义正确。`If-Range` 按 RFC 7233 §3.2 使用**强**比较
  （弱标签永不匹配），`If-None-Match` 按 RFC 7232 §3.2 使用弱比较。`ETag` 保留完整
  mtime 精度，因此原地修改但大小不变也能被识别；mtime 在未来时不发送
  `Last-Modified`（RFC 7232 §2.2.2）；不支持范围请求时显式发送
  `Accept-Ranges: none`，下载工具不必再试探。
- **性能**：流式响应与落盘上传共用 64 KiB 的 `STREAM_BUF_SIZE`（`ReaderStream`
  原先默认 4 KiB，每 GiB 会多出约 16 倍的 read 与 socket 写入）；文件响应只
  `open` 一次并从同一句柄取长度与 mtime，不再 `metadata()` 后再 `open()`；
  落盘上传走 64 KiB `BufWriter`，`tokio::fs` 不再为每个传输分片调度一次阻塞线程池；
  内存态上传改用引用计数的 `Bytes` 承载，`FormFile::open` 不再复制；
  `FormFile::copy_to` 改用 64 KiB 缓冲，而非 `tokio::io::copy` 的 8 KiB。
- **路由**：`HEAD` 自动回退到同路径的 `GET` 路由，下载端点的 HEAD 头部与 GET 一致、无响应体。
- **静态文件**：`SpaMiddleware` 改为交给宿主流式发送文件，`wwwroot` 资源同样获得
  校验器与范围请求支持，MIME 改用完整扩展名表，不再整份读入内存；并覆盖了默认的
  `cache-control: no-store`。
- **配置**：新增 `Form` 配置节（`MaxRequestSize`、`MaxFileSize`、`MaxFieldSize`、
  `MemoryThreshold`、`TempDir`）。multipart 请求按此度量，其余请求仍用
  `App.MaxBodySize`；声明超限的 `Content-Length` 会在读取上传内容前直接返回 `413`。
- **错误**：新增 `Error::PayloadTooLarge` → `413`、`Error::UnsupportedMediaType` → `415`。
- **文档**：新增[文件上传与下载](docs/rust-webx/05-request-pattern/file-upload-download.md)章节。

#### 变更

- **（破坏性）** `IHttpRequest::body_bytes` / `body_text` 改为 `&mut self`：读取即消费，
  重复读取会返回明确错误而不是空 body；`read_json_body` 改为接收 `&mut dyn IHttpRequest`。
- **（破坏性）** `IHttpRequest` 新增 `content_type()`、`is_multipart()`、`multipart()`；
  `IHttpResponse` 新增 `write_body(ResponseBody)`。两者都有默认实现，现有实现仍可编译。
- **（破坏性）** `ResponseData.body` 改为 `ResponseBody`（`Bytes` 或 `File`）并新增
  `headers` 字段；请改用构造方法而不是结构体字面量。
- **（破坏性）** `IRequest<R>` / `IRequestHandler<T, R>` / `IMediator::send` 不再要求
  `R: Serialize`，约束移到真正序列化的位置。这是 `IRequest<ResponseData>` 能成立的前提。
- 路由 dispatch 函数改为接收 `&mut dyn IHttpContext`，端点因此可以流式读取请求体。
- 请求体不再预先读入内存，在使用之前一直是活的流。
- `IHttpResponse` 不再默认 `Content-Type: application/json`，由写入方显式设置。
- `Cache-Control` 的默认值从 `SecurityHeadersMiddleware` 移到响应收尾阶段，
  以便按响应类型区分：普通响应 `no-store`，文件响应
  `public, max-age=0, must-revalidate`（`ETag` 配 `no-store` 永远用不上）。
  两种情况都只在应用未显式设置 `cache-control` 时生效，显式设置始终优先。
- 流式响应的 body 改用 `UnsyncBoxBody` 装箱，只要求 `Send`——要求 `Sync` 会
  排除掉大多数流式读取器。
- `HttpStatus` 新增 `PARTIAL_CONTENT`、`NOT_MODIFIED`、`PAYLOAD_TOO_LARGE`、
  `UNSUPPORTED_MEDIA_TYPE`、`RANGE_NOT_SATISFIABLE`。

#### 修复

- **安全**：`rustls` 升级到 `0.23.45`、`rustls-webpki` 升级到 `0.103.15`，修复
  RUSTSEC-2026-0285（TLS 1.3 握手消息曾可跨加密层级被接受）。
- `crates/spa` 的 MIME 推断改用完整扩展名表（`.csv`、`.md`、`.webp`、`.mp4` 等
  不再落到 `application/octet-stream`）。
- 静态资源不再被强制 `cache-control: no-store`。
- 修正 `custom-endpoints.md` 中「可在 `register()` 里注册自定义 `IEndpoint`」的错误说明。

#### 参考应用

- **dmbit**：库存导出改为带 `Content-Disposition` 的 `text/csv` 真下载；
  库存导入改为真正的 `multipart/form-data` 上传（`file` + `confirm_update`），
  不再把 CSV 编码进 JSON 字符串。

## [0.3.7] — 2026-09-01 — rust-ef 1.8.3 + docbit DbContext

> **English** · **简体中文**

### English

#### Fixed

- **docbit**: migrate host seed / handler writes to `DbContext` add/update mutations required by rust-ef 1.8.
- **deps**: bump workspace `rust-ef` / `rust-ef-sqlite` / `rust-ef-mysql` to **1.8.3** (String FK `EntityType` fix).

### 简体中文

#### 修复

- **docbit**：主机种子与 handler 写入改为 rust-ef 1.8 所需的 `DbContext` add/update。
- **依赖**：工作区 `rust-ef` 系列升级到 **1.8.3**。

## [0.3.6] — 2026-09-01 — rust-dix 0.7 / rust-ef 1.8.2

> **English** · **简体中文**

### English

#### Changed

- **DI / ORM**: `rust-dix 0.7` + `rust-ef 1.8.2` (single ServiceProvider graph; inject diagnostics; built-in `ScopeFactory`).

### 简体中文

#### 变更

- **DI / ORM**：升级到 `rust-dix 0.7` 与 `rust-ef 1.8.2`。

## [0.3.5] — 2026-08-31 — Cors appsettings bind + docbit Production

> **English** · **简体中文**

### English

#### Fixed

- **CorsSection**: bind PascalCase `Origins` / `Methods` / `Headers` from `appsettings*.json` (previously ignored → default `*`, Production fail-fast panic).
- **docbit Production**: CORS whitelist for `https://www.lusida.net` (+ apex); `TrustProxy` for reverse proxy; SiteUrl `www.lusida.net`.
- **run.sh**: require `JWT_SECRET` / `APP__Jwt__Secret` before start; document lusida.net overrides.

### 简体中文

#### 修复

- **CorsSection**：正确绑定 appsettings 中的 PascalCase CORS 字段（此前回退为 `*`，生产启动直接 panic）。
- **docbit 生产配置**：CORS 含 `www.lusida.net`；反代 `TrustProxy`；站点 URL。
- **run.sh**：启动前校验 JWT 环境变量。

## [0.3.4] — 2026-08-31 — Docbit SQLite-only and rustls

> **English** · **简体中文**

### English

Maintenance release focused on simpler docbit production packaging and OpenSSL-free TLS on Unix.

#### Changed

- **docbit**: SQLite-only runtime (MySQL path removed from production docs, compose, and host wiring).
- **rust-webx-host / docbit**: `reqwest` uses `rustls-tls` (no OpenSSL on Unix); `testing` feature not enabled for docbit production builds.
- **Docbit publish**: `publish.ps1 -Linux` cross-compiles `docbit-host` for `x86_64-unknown-linux-gnu` (Zig linker wrappers under `.tools/`).

### 简体中文

维护版本：docbit 仅 SQLite 部署，以及 Unix 上免 OpenSSL 的 TLS。

#### 变更

- **docbit**：生产环境仅 SQLite（移除 MySQL 相关说明与编排）。
- **rust-webx-host / docbit**：`reqwest` 使用 `rustls-tls`；生产构建不启用 `testing`。
- **Docbit 发布**：`publish.ps1 -Linux` 支持交叉编译。
## [0.3.3] — 2026-08-31 — Docbit cross-compile and docs

> **English** · **简体中文**

### English

Maintenance release: docbit-host cross-compile tooling, ecosystem docs publishing, and documentation polish. Published crate APIs are unchanged from 0.3.2.

#### Changed

- **docbit-host**: use `native-tls` with vendored features instead of `openssl-sys` for Windows → linux-gnu cross builds.
- **Docbit publish**: copy ecosystem docs from source repos; PowerShell 5.1 UTF-8 BOM on scripts; Path wildcard fix for doc copy.
- **Docs**: standardized `INDEX.json` metadata; exhibition titles and descriptions in docs seed.

### 简体中文

维护版本：docbit-host 交叉编译、生态文档发布与文档润色。已发布 crate 的 API 与 0.3.2 一致。

#### 变更

- **docbit-host**：交叉编译改用 `native-tls` vendored。
- **Docbit 发布脚本**与**文档 INDEX/展览**更新。
## [0.3.2] — 2026-08-31 — Architecture remediation Phases 1–4

> **English** · **简体中文**

### English

Breaking and behavioral changes from the architecture remediation (Phases 1–4). Migration guide: [docs/ARCHITECTURE_REMEDIATION.md](docs/ARCHITECTURE_REMEDIATION.md).

#### Breaking

- **Orphan routes/handlers fail at startup** — `HostBuilder::build()` panics when a route lacks `#[handler]`, a handler lacks a route, or duplicate `#[handler]` registrations exist. Run `cargo run -p <host> -- --doctor` before deploy.
- **SPA no longer serves `/api/*` unknown paths** — unmatched API routes return 404/501 from the router, not `index.html`.
- **`global_provider()` / `set_global_provider()` deprecated** — use `host.provider()` or `dispatch_provider()` inside `DispatchRuntime` scope. `Host::build()` no longer sets process-wide provider.
- **`register_handlers!` deprecated for HTTP** — inventory + `#[handler]` is the sole HTTP registration path; macro retained for Mediator-only scenarios.

#### Added

- **`DispatchRuntime`** on each `Host` — instance-scoped provider + `HandlerCache`; HTTP and `IHostedService::start` run inside `dispatch_runtime().run()`.
- **Query binding** — GET/DELETE merges route + query params via `Deserialize`.
- **`#[authorize(permission = "…")]`** — parsed into route metadata; Resource Auth via `use_resource_authorization()`.
- **`#[derive(WebxRequestMeta)]`** — OpenAPI query/path/body param metadata from field attributes (`#[from_query]`, `#[from_route]`, `#[from_body]`).
- **Route diagnostics** — `--doctor` reports orphan routes/handlers and duplicate registrations with fix hints.

#### Changed

- **Middleware order** — CORS → JWT → SPA → Router; `SpaMiddleware` skips `/api/*`.
- **Stub endpoints** — without dispatch return **501** (RFC 7807), not silent 200 stubs.
- **Docbit/dmbit** — `DbInitService` uses `dispatch_provider()`; GET DTOs use `WebxRequestMeta`.

#### Documented (not changed)

- **`jwt_secret()`** remains a process-wide config shim (separate from DI); see security-best-practices.md.

### 简体中文

架构整改 Phase 1–4 的破坏性变更与行为调整。迁移请参阅 [ARCHITECTURE_REMEDIATION.md](docs/ARCHITECTURE_REMEDIATION.md)。

#### 破坏性变更

- **孤儿路由/Handler 启动即 panic** — 运行 `cargo run -p <host> -- --doctor` 排查。
- **`/api/*` 未匹配路径不再返回 SPA `index.html`**。
- **`global_provider()` 已弃用** — 改用 `dispatch_provider()` / `host.provider()`。
- **HTTP 不再以 `register_handlers!` 为主路径** — 使用 inventory + `#[handler]`。

#### 新增

- **`DispatchRuntime`**、**Query 绑定**、**`#[authorize(permission)]`**、**`WebxRequestMeta`**、**`--doctor` 路由诊断**。

#### 变更

- 中间件顺序、Stub 501、Docbit GET DTO OpenAPI 元数据。

## [0.3.1] — 2026-08-16 — crates.io 再发布对齐 · Re-release alignment

> **English** · **简体中文**

### English

The `rust-webx` crate family has been re-published to crates.io and is now managed
under the `Rust-Framework` organization.

### Changed

- **crates.io re-publish**: `rust-webx` / `rust-webx-core` / `rust-webx-host` /
  `rust-webx-macros` / `rust-webx-spa` / `rust-webx-openapi` aligned; repository
  metadata (`license`, `repository`, `documentation`) aligned with the GitHub repo.
- **Automated publishing**: new GitHub Actions `publish.yml` publishes crates in
  dependency order on `v*` tag push.
- **Docs landing page**: added `docs/README.md` bilingual navigation.

> Maintenance release; the runtime API is unchanged.

### 简体中文

- **crates.io 再发布**：`rust-webx` / `rust-webx-core` / `rust-webx-host` / `rust-webx-macros`
  / `rust-webx-spa` / `rust-webx-openapi` 全系重新发布到 crates.io，归入 `Rust-Framework`
  组织统一管理；仓库元数据（`license`、`repository`、`documentation`）对齐 GitHub 仓库。
- **自动化发布**：新增 GitHub Actions `publish.yml`，推送 `v*` tag 时按依赖顺序自动发布。
- **文档落地页**：新增 `docs/README.md` 中英文档导航入口。

> 本版本为发布维护迭代，不改变运行时 API。

## [0.3.0] — 2026-07-09

### Changed (Breaking)

- **`HandlerRegistration.factory` / `HandlerEntry.factory`** signature: `fn(&dyn IServiceResolver) -> Box<dyn Any + Send>` → `fn(&dyn IServiceResolver) -> Result<Box<dyn Any + Send>>`. The `#[handler]` macro generates the new signature automatically; only manual `HandlerRegistration` constructions need updating.

### Fixed

- **Cache stampede**: `get_or_create` / `get_or_try_create` use per-key mutex + double-check to prevent thundering herd under high concurrency.
- **`MemoryCache` lock contention**: `get` / `exists` use read-lock-first (clone data → drop → short write lock for refresh) instead of write-lock for every read.
- **`MemoryCache` eviction**: FIFO via `VecDeque` replaces random `keys().next()` for predictable, fair eviction.
- **Macro panics**: `#[handler]`-generated factory/call functions return `Result` instead of `panic!`/`expect` on downcast failure.
- **`RequestIdMiddleware`**: propagates upstream `x-request-id` header instead of always generating a new UUID.
- **413 short-circuit**: `handle_request` skips the pipeline when `HttpContext::new` already set a 4xx response (prevents deserialization errors from overwriting 413 Payload Too Large).
- **`RequestTracing`**: propagates upstream `x-request-id` as `x-trace-id` (lock-free `AtomicU64` for sequence).
- Various clippy warnings resolved across macro-generated code and test scaffolding.

### Added

- Integration test suite (`request_path_test.rs`) covering 16 request-processing paths: GET/POST/PUT/DELETE, path params, JSON body, 400/404/405/413/422/500 status mapping, x-request-id propagation, unit-response 204.

## [0.2.1] — 2026-07-08

### Changed

- **Unified dispatch**: HTTP endpoints and `Mediator::send` share `dispatch::dispatch` (HandlerCache → scope → pipeline → handler).
- **Handler lookup**: `HandlerRegistration` adds `req_type_id: TypeId` for reliable in-process dispatch.
- **`add_mediator()`**: registers `Mediator` as transient (DI-injectable after host build).
- **`HandlerRegistry`**: type alias for `HandlerCache`.

### Fixed

- docbit `DocService` resolves monorepo docs at `<workspace>/docs` when `<app_base>/docs` is absent.
- docbit startup recreates SQLite schema on datatype mismatch; skips doc index when docs dir missing.

## [0.2.0] — 2026-07-08

### Changed

- **Rebrand**: crate series renamed from `rust-webapp` to `rust-webx` (`rust_webx` import path).
- **DI**: upgraded to `rust-dix 0.6` (formerly `rust-dicore 0.5`); `build()` returns `Arc<ServiceProvider>`; `get()` / `get_owned()` return `Result`.
- **ORM**: upgraded to `rust-ef 1.5.1` (+ `rust-ef-sqlite`, `rust-ef-mysql` from crates.io).
- Removed local `[patch.crates-io]` overrides for rust-ef; all ecosystem crates resolve from crates.io.

### Fixed

- `ScopeFactory` trait import for per-request DI scopes (`create_scope()`).
- Mediator / host tests adapted to rust-dix 0.6 `ServiceProvider` API.

### Migration — 0.1.x → 0.2.0

1. `Cargo.toml`: `rust-webapp = "0.1"` → `rust-webx = "0.2"`.
2. `use rust_webapp::*` → `use rust_webx::*`.
3. `rust_dicore` → `rust_dix`; `rust-dicore` → `rust-dix`.
4. Remove `Arc::new()` around `ServiceCollection::build()`; handle `Result` from `get()` / `get_owned()`.
5. `rust-ef = "1.5.1"` with provider crates on crates.io (no path patch).

## [0.2.0] — production readiness (docbit)

### Added

- `JWT_SECRET` environment variable support (overridden by `APP__Jwt__Secret`).
- Production fail-fast when JWT secret is missing, short, or a known placeholder.
- docbit production middleware stack: rate limit, compression, timing, request tracing.
- `docbit/Dockerfile`, `docbit/docker-compose.yml`, `docbit/.env.example`, `docbit/publish.sh`, `docbit/PRODUCTION.md`.
- CI job: Docker build for docbit.

### Framework production fixes (0.2.0)

- **Dynamic health checks**: `/health` and `/health/ready` evaluate probes per request; `fail` returns HTTP 503.
- **SIGTERM**: Unix graceful shutdown via tokio signal listener (alongside Ctrl+C).
- **`IHost::stop()`** and **`run_at()`** now trigger shutdown and run hosted service lifecycle.
- **Production fail-fast** for CORS wildcard `*`.
- **OpenAPI UI** registered only in Development mode.
- Tests: runtime health probe, production guard panics, `APP__Jwt__Secret` precedence.

### Framework production fixes (0.2.0) — continued

- **Unified error format**: 401/403/429/413 统一为 RFC 7807 `application/problem+json`（`problem_response` 模块）。
- **SIGTERM / shutdown tests** + **TLS HTTPS 集成测试**（rcgen 自签证书）。
- **docs/rust-webx** 批量更新：`rust_dix`、`add_memory_cache`、TLS/health API。

### Framework P2 (0.2.0)

- **RateLimit appsettings**：`RateLimit.Enabled/RequestsPerSecond/BurstSize/MaxTrackedIps`，build 时自动注册中间件。
- **Rate limit LRU**：超过 `MaxTrackedIps` 时淘汰最久未刷新的 IP bucket。
- **`GET /metrics`**：Prometheus text 格式（`Metrics.Enabled`）。
- docbit Production 改用 appsettings 配置 RateLimit/Metrics。
- 测试：openapi spec、spa 工具函数、metrics 集成、rate limit LRU。

### Changed

- `appsettings.Production.json`: JWT secret removed from file; must be supplied via env.

### Known limitations (0.2.0)

- **OpenTelemetry export**: not built-in; use structured JSON logs (Production), `RequestTracing` middleware, and optional `GET /metrics` (Prometheus). OTLP planned for a future minor release.
- **rust-ef insert ID**: `save_changes` does not backfill auto-increment IDs (1.5.1); docbit handlers re-query by natural keys (documented as `FIXME(upstream)`).

## [0.1.0] — 2026-06

Initial release as `rust-webapp` (superseded by 0.2.0 rebrand).
