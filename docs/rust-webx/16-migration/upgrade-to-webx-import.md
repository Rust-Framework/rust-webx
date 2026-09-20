# 升级到 `webx` 导入路径（0.4 → 0.5）

本次迭代把**代码导入路径**从 `rust_webx` 统一为 `webx`，并把 SPA 内嵌改为显式入口。
**Cargo 包名保持不变**（仍是 `rust-webx` / `rust-webx-*`），所以 `Cargo.toml` 的依赖声明
无需改动，只需修改 `use` 路径与嵌入入口。

## 变更一览

| 项目 | 旧 | 新 |
|------|-----|-----|
| 伞 crate 导入 | `use rust_webx::*;` | `use webx::*;` |
| 子 crate 导入 | `rust_webx_core::…` / `rust_webx_host::…` / `rust_webx_spa::…` / `rust_webx_openapi::…` / `rust_webx_macros::…` | `webx_core::…` / `webx_host::…` / `webx_spa::…` / `webx_openapi::…` / `webx_macros::…` |
| 入口宏 | `#[rust_webx::main]` | `#[webx::main]` |
| 嵌入宏 | `rust_webx::spa::embed_assets!()` | `webx::spa::embed_assets!()` |
| 应用基准目录环境变量 | `RUST_WEBX_APP_BASE` | `WEBX_APP_BASE` |
| 内嵌开关环境变量 | `RUST_WEBX_EMBED` | `WEBX_EMBED` |
| 构建期 crate | `rust_webx_build::embed_assets("wwwroot")` | `webx::builder().web_root("wwwroot").build()` |
| 生成文件 | `rust_webx_embedded_assets.rs` | `webx_embedded_assets.rs` |
| Builder 方法 | `.embed()` | 已删除（改由入口宏 / `build.rs` 决定） |

> **包名不动**：`rust-webx`、`rust-webx-core`、`rust-webx-host`、`rust-webx-macros`、
> `rust-webx-spa`、`rust-webx-openapi`、`rust-webx-build`。
> package 名（带连字符）与导入名（下划线）不是一回事，这是 Rust 的常态：
> `rust-webx = "0.5"` 提供 `webx::`。

## 迁移步骤

### 1. 批量替换标识符

```bash
# 只替换下划线形式；包名（带连字符）不受影响
grep -rl 'rust_webx' --include='*.rs' . | xargs sed -i 's/rust_webx_build/webx/g; s/rust_webx/webx/g'
grep -rl 'RUST_WEBX' --include='*.rs' . | xargs sed -i 's/RUST_WEBX/WEBX/g'
```

（PowerShell：注意 `-replace` 默认大小写不敏感，建议用 `-creplace`。）

### 2. SPA 内嵌改为显式

**旧写法**

```rust
// build.rs
fn main() -> Result<(), rust_webx_build::Error> {
    rust_webx_build::embed_assets("wwwroot")
}

// main.rs
rust_webx::spa::embed_assets!();          // crate root
Host::builder().use_spa("wwwroot").embed().build()
```

**新写法**

```rust
// build.rs —— 编译期源树（烤进二进制的目录）
fn main() -> Result<(), webx::Error> {
    webx::builder()
        .web_root("wwwroot")
        .build()
}

// main.rs —— (embed) 明确链接该表
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
| `web_root(...)` | 编译期 | **烤进 bin 的源目录** |
| `#[webx::main(embed)]` | 编译期入口 | **确认链接**该表；不写则完全不嵌入 |
| `.use_spa("wwwroot")` | 运行期 | **磁盘覆盖目录**（运维可改 favicon 等） |

### 3. 集成测试

测试进程不执行 `fn main`，用属性宏注册同一张表：

```rust
#[webx::embed_assets]
mod __embedded_assets {}
```

### 4. 环境变量

部署脚本 / 容器环境变量同步改名：

- `RUST_WEBX_APP_BASE` → `WEBX_APP_BASE`
- `RUST_WEBX_EMBED` → `WEBX_EMBED`

## 验证

```bash
cargo check --workspace
cargo test -p rust-webx-build
cargo test -p docbit-host --test e2e_test without_wwwroot
```

## 常见问题

| 现象 | 原因 | 处理 |
|------|------|------|
| `unresolved import webx` | 依赖未升级，或仍在用 `rust_webx` | 升级到 0.5+；把 `use rust_webx::` 换成 `use webx::` |
| 编译报「缺少 OUT_DIR 生成文件」 | 写了 `(embed)` 但 `build.rs` 未配 `web_root` | 补 `build.rs`，或去掉 `(embed)` |
| `embed() found no compiled-in assets` | 仍在调用已删除的 `.embed()` | 删除该方法调用，见步骤 2 |
| 旧资源仍被内嵌 | `OUT_DIR` 中残留旧生成文件 | `cargo clean` 后重建 |
| 环境变量不生效 | 仍用 `RUST_WEBX_*` | 改为 `WEBX_*` |

## 相关文档

- [OpenAPI 与 SPA 托管](../11-production/openapi-spa.md) — 内嵌模型与覆盖顺序
- [Crate 分层结构](../04-architecture/crate-layout.md) — 编译期 crate 的职责
