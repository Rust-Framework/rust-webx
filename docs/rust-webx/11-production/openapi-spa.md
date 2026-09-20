# OpenAPI 与 SPA 托管

## OpenAPI 自动生成

框架从编译时收集的路由元数据自动生成 OpenAPI 3.0 规范。两个端点**只在
Development 下注册**，Production 下不存在：

```
GET /api/openapi.json    → OpenAPI 规范 JSON
GET /api/openapi.html    → 内置 API 文档页
```

```rust
use webx::{generate_openapi_spec, APIUI_HTML};
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

`Cache-Control` 的取值见 [文件服务的生产部署](file-serving.md)。

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

一个二进制同时服务 API、前端与内置 API 文档。

## 把 wwwroot 编译进 exe

默认部署需要带上 `wwwroot/` 目录。要把静态文件**烤进**可执行文件，需要两处显式声明
（对标 ASP.NET：csproj 声明资源 + Program 仍写 UseStaticFiles）：

| 符号 | 时机 | 含义 |
|------|------|------|
| `build.rs` → `web_root(...)` | 编译期 | **烤进二进制的源目录**（文件集固定） |
| `#[webx::main(embed)]` | 编译期入口 | **确认链接**该表；不写 `(embed)` 则不进包 |
| `.use_spa("wwwroot")` | 运行期 | **磁盘覆盖目录**（运维可改 favicon 等） |

不写 `build.rs` / 不写 `(embed)`：行为与纯磁盘 SPA 相同（`.use_spa` 或自动检测
`wwwroot/`），exe 里没有内嵌表。

```rust
// build.rs —— 编译期源树
fn main() -> Result<(), webx::Error> {
    webx::builder()
        .web_root("wwwroot")   // 或 "../wwwroot"（crate 在子目录时）
        .build()
}
```

```rust
// main.rs —— (embed) = 明确「要嵌入」
#[webx::main(embed)]
async fn main() {
    Host::builder()
        .use_spa("wwwroot")   // 部署旁的覆盖目录；可与 web_root 路径不同
        .build()
        .run()
        .await?;
}
```

只有 `web_root` 没有 `(embed)`：build 仍会生成表，但**不会**链进二进制。  
只有 `(embed)` 没有 `web_root`：编译失败（缺少 `OUT_DIR` 生成文件）。

`Cargo.toml` 需要 `[build-dependencies] rust-webx-build = "0.5"`（库名导入为
`webx`）。

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

### 内嵌文件怎么压缩

`build.rs` 会对可压缩的文件做 brotli 编码（质量 11），表里存的是**压缩后的字节流
加上原始长度**。因此二进制显著变小，而磁盘上不需要任何中间产物。

已压缩的媒体（PNG、WOFF2、ZIP 等）和小于 256 字节的文件原样存储——再压一次既费
构建时间，也不会变小。

运行期按 `Accept-Encoding` 协商：

| 客户端 | 响应 |
|--------|------|
| 接受 `br` | 直接发送压缩字节流，附 `Content-Encoding: br`，不解压、不分配 |
| 不接受 `br` | 解压一次并缓存到进程结束，发送原文件 |

两条路径都带 `Vary: Accept-Encoding`，且 `ETag` 按表示区分：压缩表示是
`"sha256-…-br"`，原文件是 `"sha256-…"`。共享缓存因此不会把 brotli 字节交给只请求
原文件的客户端。

解压是按需的：只有客户端不支持 `br` 时才会发生，并且每个文件最多一次。反过来，接受
`br` 的客户端拿到的静态资源也是压缩传输的——磁盘文件本身没有被压缩过，这是内嵌表
额外带来的收益。

### 必须知道的取舍

* **僵尸覆盖（stale override）**：磁盘上的同名文件**永远**赢。如果某次更新改了 exe 内的
  `app.css`，而部署目录里还留着上一版手改的 `wwwroot/app.css`，新版本会被永久遮蔽。
  框架在启动时会把被覆盖的文件列出来告警：

  ```
  WARN [Host] 1 embedded file(s) are overridden on disk: app.css. An override older
  than the current build stays in effect until it is updated or deleted.
  ```

  建议把覆盖目录当白名单用：只放确实要自定义的文件，升级时一并检查。
* **体积**：可压缩文件先经 brotli 编码才进入二进制；docbit 实测 576 个文件 / 27 MiB
  的 `wwwroot` 只占约 4.7 MiB。详见[内嵌文件怎么压缩](#内嵌文件怎么压缩)。
* **编译期与运行期**：`web_root` 是**编译目录**；`.use_spa` 是**部署覆盖目录**。
  二者可以不同（docbit 从 `../wwwroot` 编译、部署用 `wwwroot`）。
* **增删文件**：build.rs 会为整个目录树发出 `cargo:rerun-if-changed`，所以新增、
  修改、删除文件都会正确触发重编译。
* **符号链接**：不跟随。链接到别处的文件会让内嵌内容超出 manifest 的描述范围，
  链接的目录还可能造成遍历环。需要内嵌就放真实文件或副本。
* **失败即报错**：缺少目录时由 `?` 冒泡成构建失败。推荐写法：

  ```
  error: failed to run custom build command for `docbit-host v0.1.0`
  --- stderr
  Error: static asset directory `../wwwrooot` does not exist
  (resolved to `D:\...\docbit\wwwrooot`); create it, or pass the right path to builder().web_root(...)
  ```

* **调试逃生开关**：`WEBX_EMBED=off` 让运行期忽略内嵌文件、只读磁盘，
  不必重新编译就能确认问题是否来自覆盖。
* **大小写**：内嵌表按精确路径匹配，区分大小写（与 Linux 一致）；磁盘在不区分
  大小写的系统上仍按系统行为匹配。
* **依赖**：生成的代码引用伞 crate `::webx`；`build-dependencies` 加
  `rust-webx-build`（在 build.rs 里以 `webx::` 使用）即可。

### 发布脚本

`docbit/publish.ps1` 的 `-NoWwwroot` 跳过 `wwwroot/` 同步，产出「exe + 配置」的干净目录：

```powershell
.\publish.ps1 -Destination D:\deploy\docbit -Production -NoWwwroot
```

## 小结

OpenAPI 从类型信息自动生成，SPA 托管让全栈单体部署成为可能；`build.rs` 的
`web_root` 再把静态资源收进 exe，部署收敛为单个文件，同时保留逐个文件的覆盖能力。

下一节：[文件服务的生产部署](file-serving.md)
