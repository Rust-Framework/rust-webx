# 升级到 0.5（破坏性变更）

0.5 有两类破坏性变更：**导入路径改名**（机械替换即可）和 **静态资源内嵌模型的调整**
（需要改构建脚本与入口）。Cargo 依赖声明本身**不需要改**——包名保持 `rust-webx` /
`rust-webx-*`，变的只是代码里的 `use` 路径。

> 本章是**升级操作手册**：每一步都可以独立完成并验证。框架自身的版本变更记录见
> 仓库根目录的 `CHANGELOG.md`。

## 变更总览

| # | 项目 | 0.4 及更早 | 0.5 |
|---|------|-----------|-----|
| 1 | 伞 crate 导入 | `use rust_webx::*;` | `use webx::*;` |
| 2 | 子 crate 导入 | `rust_webx_core::…`、`rust_webx_host::…`、`rust_webx_spa::…`、`rust_webx_openapi::…`、`rust_webx_macros::…` | `webx_core::…`、`webx_host::…`、`webx_spa::…`、`webx_openapi::…`、`webx_macros::…` |
| 3 | 入口宏 | `#[rust_webx::main]` | `#[webx::main]` / `#[webx::main(embed)]` |
| 4 | 应用基准目录环境变量 | `RUST_WEBX_APP_BASE` | `WEBX_APP_BASE` |
| 5 | 内嵌开关环境变量 | `RUST_WEBX_EMBED` | `WEBX_EMBED` |
| 6 | 生成的资源表文件 | `rust_webx_embedded_assets.rs` | `webx_embedded_assets.rs` |
| 7 | SPA 内嵌开关 | `Host::builder()….embed()` | 方法已删除；改用 `#[webx::main(embed)]` + `build.rs` |
| 8 | 内嵌资源存储 | 原样存储 | 可压缩文件以 brotli 存储（`EmbeddedAsset` 新增字段） |

第 1–6 项是纯机械替换，第 7–8 项需要改代码结构。

## 步骤 1：批量替换标识符

包名带连字符（`rust-webx`），代码路径带下划线（`rust_webx`）。**只替换下划线形式**，
`Cargo.toml` 完全不受影响：

```bash
# 先看清楚会改到哪些文件
grep -rl 'rust_webx' --include='*.rs' .
grep -rl 'RUST_WEBX' --include='*.rs' . ; grep -rl 'RUST_WEBX' --include='*.toml' --include='*.yml' --include='*.ps1' --include='*.sh' .

# 再替换：先替换更长的 rust_webx_build，避免被 rust_webx 抢先匹配
grep -rl 'rust_webx' --include='*.rs' . | xargs sed -i 's/rust_webx_build/webx/g; s/rust_webx/webx/g'
grep -rl 'RUST_WEBX' --include='*.rs' . | xargs sed -i 's/RUST_WEBX/WEBX/g'
```

PowerShell 下注意 `-replace` 默认大小写不敏感，建议用 `-creplace`：

```powershell
Get-ChildItem -Recurse -Filter *.rs |
  Select-String -Pattern 'rust_webx' -List | ForEach-Object {
    (Get-Content $_.Path -Raw) -creplace 'rust_webx_build','webx' -creplace 'rust_webx','webx' |
      Set-Content $_.Path -NoNewline
  }
```

部署脚本、`Dockerfile`、k8s manifest 里的环境变量也要一起改（第 4、5 项）。

**验证：**

```bash
grep -rn 'rust_webx\|RUST_WEBX' --include='*.rs' .   # 应该没有任何输出
cargo check --workspace
```

## 步骤 2：SPA 内嵌改为显式入口

`HostBuilder::embed()` 已删除。0.5 把「哪些文件烤进二进制」和「运行期从哪个目录读覆盖」
拆成两处独立声明：

### 旧写法（0.4）

```rust
// build.rs
fn main() -> Result<(), rust_webx_build::Error> {
    rust_webx_build::embed_assets("wwwroot")
}
```

```rust
// main.rs
rust_webx::spa::embed_assets!();          // 在 crate 根部展开
Host::builder().use_spa("wwwroot").embed().build()
```

### 新写法（0.5）

```rust
// build.rs —— 编译期源树：决定「哪些文件进二进制」
fn main() -> Result<(), webx::Error> {
    webx::builder()
        .web_root("wwwroot")
        .build()
}
```

```rust
// main.rs —— (embed) 决定「这张表是否链接进本二进制」
#[webx::main(embed)]
async fn main() {
    Host::builder()
        .use_spa("wwwroot")   // 运行期磁盘覆盖目录
        .build()
        .run()
        .await
        .expect("Server failed");
}
```

三者语义必须分清：

| 符号 | 时机 | 含义 |
|------|------|------|
| `web_root(...)`（build.rs） | 编译期 | **烤进二进制的源目录**，文件集在此固定 |
| `#[webx::main(embed)]` | 编译期入口 | **确认链接**该表；不写 `(embed)` 则完全不嵌入 |
| `.use_spa("wwwroot")` | 运行期 | **磁盘覆盖目录**（运维可改 favicon 等） |

`web_root` 与 `.use_spa` 路径可以不同：从 `frontend/dist` 编译、在部署目录的 `wwwroot`
放覆盖，是完全正常的用法。

### 三种部署形态

| 配置 | 结果 |
|------|------|
| 无 `build.rs` web_root，且用 `#[webx::main]` | 纯磁盘 SPA（`.use_spa` 或自动检测 `wwwroot/`） |
| `build.rs` web_root + `#[webx::main(embed)]` | 资源烤进 exe，磁盘覆盖可选 |
| `build.rs` web_root + 但入口只写 `#[webx::main]` | 表已生成但**未链接**，等于没有内嵌 |

集成测试不执行 `fn main`，因此用属性宏注册同一张表：

```rust
#[webx::embed_assets]
mod __embedded_assets {}
```

## 步骤 3：运行期会按 `Accept-Encoding` 协商压缩

0.5 起，可压缩的内嵌资源以 **brotli（质量 11）** 存储，`Content-Encoding: br` 按需发送。
这一步通常**不需要你改任何代码**，但要知道行为：

| 客户端 | 响应 |
|--------|------|
| 接受 `br`（或 `*`） | 直接发送压缩字节流 + `Content-Encoding: br`，不解压、不分配 |
| 不接受 `br`（或 `br;q=0`） | 解压一次并缓存到进程结束，发送原文件 |

- 已压缩的媒体（PNG、JPEG、WOFF2、ZIP、MP4 等扩展名）与小于 256 字节的文件**原样存储**，
  再压一次不会变小。
- 只有可能随 `Accept-Encoding` 变化的响应才带 `Vary: Accept-Encoding`。
- `ETag` 按表示区分：压缩表示是 `"sha256-…-br"`，原文件是 `"sha256-…"`。因此共享缓存
  不会把 brotli 字节交给只请求原文件的客户端。
- 静态资源**首次**在传输层也被压缩了：此前磁盘文件不经过响应压缩中间件。

**构建时间**：brotli 质量 11 是最高档，代价在构建期而非运行期。docbit 实测 576 个文件 /
27 MiB 的 `wwwroot` 约需 30 秒，换来内嵌体积 27 MiB → 4.7 MiB、可执行文件 30.2 MiB →
12.9 MiB。资源很大的项目可以把 `/assets` 下的大体积 vendored 文件裁掉，收益最直接。

**排障开关**：`WEBX_EMBED=off`（或 `0` / `false` / `no`）让运行期忽略内嵌文件、只读磁盘，
不必重新编译即可确认问题是否来自磁盘覆盖。

## 步骤 4：只在手写资源表时才需要改

绝大多数项目由 `build.rs` 生成表，**无需处理**。只有手写 `EmbeddedAssets`（少见）时才受影响：
`EmbeddedAsset` 新增两个字段。

```rust
// 0.4
EmbeddedAsset {
    path: "app.css",
    bytes: include_bytes!("../wwwroot/app.css"),
    content_type: "text/css",
    etag: "\"sha256-…\"",
}

// 0.5
EmbeddedAsset {
    path: "app.css",
    bytes: include_bytes!("../wwwroot/app.css"),
    raw_len: 1234,                                   // 新增：解压后的长度
    encoding: webx::spa::ContentEncoding::Identity,   // 新增：Identity | Brotli
    content_type: "text/css",
    etag: "\"sha256-…\"",
}
```

约定：`raw_len` 始终是**解压后**的长度；`encoding` 为 `Identity` 时 `bytes` 就是文件本身，
且 `bytes.len() == raw_len`。`Encoding::Brotli` 时 `bytes` 是 brotli 流。

`EmbeddedAssets` 的统计口径也随之明确：`total_bytes()` 是**实际占用的字节**，
`total_raw_bytes()` 是**解压后的总大小**。

## 验证清单

```bash
cargo check --workspace
cargo test --workspace
# 启动后确认日志里的内嵌体积与来源
cargo run --release -p <your-host>
```

启动日志会明确报告内嵌情况，可用于确认第 2 步是否生效：

```
INFO [Host] Embedded assets: 576 files, 4.7 MiB embedded (27.0 MiB uncompressed), built from `../wwwroot`
```

只写了 `web_root` 而漏了 `(embed)` 时，这一行不会出现，SPA 会退化为纯磁盘模式——这是最
容易被忽略的失误。

## 常见问题

| 现象 | 原因 | 处理 |
|------|------|------|
| `unresolved import webx` | 依赖未升级，代码里仍是 `rust_webx` | 升级到 0.5+，按步骤 1 替换 `use` 路径 |
| `cannot find attribute rust_webx` | 入口宏未改名 | 写成 `#[webx::main]` |
| 编译报「缺少 OUT_DIR 生成文件」 | 写了 `(embed)` 但 `build.rs` 没有 `web_root` | 补 `build.rs`，或去掉 `(embed)` |
| `embed() found no compiled-in assets` | 仍在调用已删除的 `.embed()` | 删除该方法调用，见步骤 2 |
| 启动日志没有 `Embedded assets` 一行 | 入口宏漏写 `(embed)` | 改成 `#[webx::main(embed)]` |
| 改了资源但二进制没变 | `OUT_DIR` 残留旧生成文件 | `cargo clean` 后重建 |
| 环境变量不生效 | 仍用 `RUST_WEBX_*` | 改为 `WEBX_*`，含部署脚本与容器配置 |
| 客户端拿到乱码 | 中间层（反代/CDN）剥离或重复了 `Content-Encoding` | 确认它按 `Vary: Accept-Encoding` 缓存，且不二次压缩静态资源 |

## 相关文档

- [OpenAPI 与 SPA 托管](../11-production/openapi-spa.md) — 内嵌模型、压缩与覆盖顺序
- [Crate 分层结构](../04-architecture/crate-layout.md) — 包名与导入名的对应关系
- [文件服务的生产部署](../11-production/file-serving.md) — 缓存头与反向代理

下一节：[从 ASP.NET Core 迁移](from-aspnet-core.md)
