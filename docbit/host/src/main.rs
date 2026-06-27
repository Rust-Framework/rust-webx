//! Docbit host crate entry point — composition root.
//!
//! 参考 ASP.NET Core 的极简启动：
//! - 运行模式（`APP_ENV`）→ 框架在 `Host::builder()` 内部自动读取，应用无需感知
//! - appsettings 加载、环境 overlay 合并、配置文件定位 → 框架自动
//!   （`appsettings.{Mode}.json` 自动合并）
//! - SPA 静态资源 → 框架自动检测 `wwwroot/`，无需 `use_spa`
//! - DbContext 注册 → 应用级 `add_dbcontext(|o| ...)`，provider 由 appsettings
//!   的 `Database:ConnectionString` 自动识别（框架本身不依赖 EF）
//! - 应用专属配置（`SiteConfig`）→ 框架 `bind_config::<SiteConfig>("Site")` 自动绑定
//! - handler / hosted service → `#[rust_dicore::inject_attr]` 编译期自动注册

mod authorizer;
mod db;
mod doc_service;
mod interceptor;
mod startup;

use docbit_contracts::site::SiteConfig;
use rust_webapp::*;

use crate::db::HostBuilderDbExt;

// 显式引用 handlers 与 domain crate，确保它们的 `#[rust_dicore::inject_attr]`
// 与 `inventory::submit!` 注册被链接进最终二进制（否则链接器可能丢弃未使用 crate）。
extern crate docbit_domain;
extern crate docbit_handlers;

#[tokio::main]
async fn main() {
    let host = Host::builder()
        .add_dbcontext(|o| {
            o.add_interceptor(interceptor::AuditInterceptor);
        })
        .add_options::<SiteConfig>("Site")
        .add_authentication()
        .add_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}
