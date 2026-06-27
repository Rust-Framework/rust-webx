//! Docbit host crate entry point — composition root.
//!
//! 启动流程：
//! 1. 解析 `AppPaths`（wwwroot/db/docs/blog-data）；
//! 2. 通过 `Host::builder()` 装配 DI / 中间件 / SPA / JWT / MemoryCache；
//! 3. 注册 `bootstrap::configure(mode)` 提供 `AppPaths` / `Mutex<DbContext>` / `SiteConfig`；
//! 4. `Host::build()` 自动收集所有 `#[rust_dicore::inject_attr]` 标注的服务与 handler；
//! 5. `host.run()` 启动 HTTP 服务，并在启动前执行 `IHostedService`（如 `DbInitService`）。

mod authorizer;
mod bootstrap;
mod config;
mod doc_service;
mod interceptor;
mod paths;
mod startup;

use rust_webapp::*;

// 显式引用 handlers 与 domain crate，确保它们的 `#[rust_dicore::inject_attr]`
// 与 `inventory::submit!` 注册被链接进最终二进制（否则链接器可能丢弃未使用 crate）。
extern crate docbit_domain;
extern crate docbit_handlers;

use crate::paths::AppPaths;

#[tokio::main]
async fn main() {
    let mode = AppMode::Development; // 生产环境由 `cargo run --release` 或环境变量切换
    let wwwroot = AppPaths::resolve().wwwroot;

    let host = Host::builder()
        .mode(mode)
        .register(bootstrap::configure(mode))
        .use_spa(wwwroot.to_string_lossy().into_owned())
        .use_auth()
        .use_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}
