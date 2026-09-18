# OpenAPI 与 SPA 托管

## OpenAPI 自动生成

框架从编译时收集的路由元数据自动生成 OpenAPI 3.0 规范：

```
GET /openapi.json    → OpenAPI 规范 JSON
GET /swagger         → Swagger UI 页面
```

```rust
use rust_webx::{generate_openapi_spec, APIUI_HTML};
```

## SPA 静态托管

```rust
Host::builder().use_spa("wwwroot")
```

`SpaMiddleware` 行为：
1. 请求路径匹配静态文件 → 交给宿主流式发送
2. 非 API 路径无匹配 → fallback 到 `index.html`
3. 支持前端 History 路由（React Router、Vue Router 等）

静态文件不是整份读进内存，而是以文件响应发送，因此自带：

| 能力 | 行为 |
|------|------|
| MIME 类型 | 按扩展名推断（完整扩展名表） |
| `ETag` / `Last-Modified` | 自动生成 |
| `Range` | 支持 `206` 断点续传 |
| `HEAD` | 与 GET 相同头部，无响应体 |
| `Cache-Control` | `/assets/**` 为 `public, max-age=31536000, immutable`；其余为 `public, max-age=0, must-revalidate`（覆盖默认的 `no-store`） |

> `wwwroot` 下的文件**不经过授权检查**。需要鉴权的文件请放在可写目录，用
> [`ResponseData::file`](../05-request-pattern/file-upload-download.md) + `#[authorize]` 提供。

### 目录结构

```
wwwroot/
├── index.html
├── app.js
├── app.css
└── assets/
```

## 全栈单体部署

```rust
Host::builder()
    .use_spa("wwwroot")
    .add_authentication()
    .build()
    .run()
    .await?;
```

一个二进制同时服务 API + 前端 + Swagger 文档。

## 把 wwwroot 编译进 exe

默认部署需要带上 `wwwroot/` 目录。加上 `.embed()` 后静态文件直接编译进可执行文件，
发布产物可以只有一个 exe；同时保留「在 exe 旁放 `wwwroot/` 覆盖个别文件」的能力
（比如替换 favicon）。这与 ASP.NET Core 的 `CompositeFileProvider` +
`ManifestEmbeddedFileProvider` 是同一个模型。

三步接入：

```rust
// 1) build.rs —— 目录只在这里出现一次；错误用 ? 冒泡成构建失败
fn main() -> Result<(), rust_webx_build::Error> {
    rust_webx_build::embed_assets("wwwroot")
}
```

```rust
// 2) 每个二进制声明一次（lib.rs 或 main.rs 的模块层级）
rust_webx::spa::embed_assets!();
```

```rust
// 3) 启用
Host::builder()
    .use_spa("wwwroot")   // 覆盖文件的读取目录；不写则用编译目录
    .embed()
    .build()
    .run()
    .await?;
```

`Cargo.toml` 需要 `[build-dependencies] rust-webx-build = "0.4"`。

### 查找顺序

每个请求按「同名文件 → `index.html` 回退」的顺序解析：

| 顺序 | 来源 | 说明 |
|------|------|------|
| 1 | `<use_spa 目录>/<路径>` | 磁盘文件优先，运维放入的覆盖生效 |
| 2 | 编译进 exe 的同名文件 | 兜底基线 |
| 3 | 磁盘 `index.html` → exe 内 `index.html` | SPA 路由回退 |

`/assets/**` 走同样的顺序（磁盘 → 内嵌 → 404），只是缓存指令不同。

### 内嵌文件为什么能断点续传

内嵌资源在 host 里以「已知长度、可 seek」的流发送，所以 `Content-Length`、
`Range`/`206`、`If-None-Match`/`304`、`HEAD` 与磁盘文件完全一致。区别在校验器：

| 来源 | `ETag` | 特点 |
|------|--------|------|
| 磁盘文件 | 大小 + mtime | 文件一变就变 |
| 内嵌文件 | 内容 `sha256` | 跨机器、跨重新编译都一致，多副本/CDN 不会缓存出两份 |

### 必须知道的取舍

* **僵尸覆盖（stale override）**：磁盘上的同名文件**永远**赢。如果某次更新改了 exe 内的
  `app.css`，而部署目录里还留着上一版手改的 `wwwroot/app.css`，新版本会被永久遮蔽。
  框架在启动时会把被覆盖的文件列出来告警：

  ```
  WARN [Host] 1 embedded file(s) are overridden on disk: app.css. An override older
  than the current build stays in effect until it is updated or deleted.
  ```

  建议把覆盖目录当白名单用：只放确实要自定义的文件，升级时一并检查。
* **体积**：内嵌会让每个 exe 变大（docbit 实测 576 个文件 / 27 MiB）。因此 `.embed()`
  是显式 opt-in，不调用就与现在完全一致。
* **编译期与运行期**：`.embed()` 记录的是**编译目录**，不随部署变化。构建目录与部署
  目录不同时用 `use_spa` 指定（docbit 从 `../wwwroot` 编译、部署用 `wwwroot`，
  因此写了 `.use_spa("wwwroot")`）。
* **增删文件**：build.rs 会为整个目录树发出 `cargo:rerun-if-changed`，所以新增、
  修改、删除文件都会正确触发重编译。
* **符号链接**：不跟随。链接到别处的文件会让内嵌内容超出 manifest 的描述范围，
  链接的目录还可能造成遍历环。需要内嵌就放真实文件或副本。
* **失败即报错**：`embed_assets` 返回 `Result`，目录不存在时由 `?` 冒泡成构建失败，
  而不是 panic。这也是 `fn main() -> Result<(), rust_webx_build::Error>` 的写法：

  ```
  error: failed to run custom build command for `docbit-host v0.1.0`
  --- stderr
  Error: static asset directory `../wwwrooot` does not exist
  (resolved to `D:\...\docbit\wwwrooot`); create it, or pass the right path to embed_assets()
  ```

* **调试逃生开关**：`RUST_WEBX_EMBED=off` 让运行期忽略内嵌文件、只读磁盘，
  不必重新编译就能确认问题是否来自覆盖。
* **大小写**：内嵌表按精确路径匹配，区分大小写（与 Linux 一致）；磁盘在不区分
  大小写的系统上仍按系统行为匹配。
* **依赖**：生成的代码引用 `::rust_webx`，因此调用 `embed_assets` 的 crate 需依赖
  伞 crate `rust-webx`；`build-dependencies` 加 `rust-webx-build` 即可。

### 发布脚本

`docbit/publish.ps1` 的 `-NoWwwroot` 跳过 `wwwroot/` 同步，产出「exe + 配置」的干净目录：

```powershell
.\publish.ps1 -Destination D:\deploy\docbit -Production -NoWwwroot
```

## 小结

OpenAPI 从类型信息自动生成，SPA 托管让全栈单体部署成为可能；`embed()` 再把静态资源
收进 exe，部署收敛为单个文件，同时保留逐个文件的覆盖能力。

下一节：[优雅关闭与可观测性](graceful-shutdown.md)
