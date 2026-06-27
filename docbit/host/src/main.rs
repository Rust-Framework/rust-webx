//! Docbit host crate entry point — composition root.
//!
//! 启动流程：
//! 1. 框架 `Host::builder()` 内部按 `APP_ENV` 环境变量自动解析运行模式，并据此
//!    合并 `appsettings.{Mode}.json` overlay、定位配置文件（cwd / exe 同级 / 上溯）；
//! 2. 应用侧同样读取 `APP_ENV` 决定数据库 Provider（MySQL / SQLite），传入
//!    `bootstrap::configure(mode)`；
//! 3. 解析 `AppPaths`（wwwroot/db/docs）；
//! 4. 装配 DI / 中间件 / SPA / JWT / MemoryCache；
//! 5. `Host::build()` 自动收集所有 `#[rust_dicore::inject_attr]` 标注的服务与 handler；
//! 6. `host.run()` 启动 HTTP 服务，并在启动前执行 `IHostedService`（如 `DbInitService`）。

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
    // 应用侧读取运行模式以选择数据库 Provider；框架侧由 HostBuilder 自行读取同一变量。
    let mode = AppMode::from_env();
    tracing::info!("[docbit] running in {:?} mode", mode);
    let wwwroot = AppPaths::resolve().wwwroot;

    let host = Host::builder()
        .register(bootstrap::configure(mode))
        .use_spa(wwwroot.to_string_lossy().into_owned())
        .use_auth()
        .use_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}
