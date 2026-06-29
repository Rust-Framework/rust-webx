//! Docbit host crate entry point — composition root.
//!
//! 参考 ASP.NET Core 的极简启动：
//! - 运行模式（`APP_ENV`）→ 框架在 `Host::builder()` 内部自动读取
//! - appsettings 加载、环境 overlay 合并、配置文件定位 → 框架自动
//! - SPA 静态资源 → 框架自动检测 `wwwroot/`，无需 `use_spa`
//! - DbContext → 应用层直接注册（Development=SQLite, Production=MySQL）
//! - 应用专属配置（`SiteConfig`）→ 框架 `add_options::<SiteConfig>("Site")` 自动绑定
//! - handler / hosted service / dynamic authorizer / document service → `#[rust_dicore::inject]` 编译期自动注册

mod startup;

use std::sync::Arc;

use docbit_contracts::site::SiteConfig;
use rust_ef::db_context::{DbContext, DbContextOptionsBuilder};
use rust_ef_mysql::DbContextOptionsBuilderExt as _;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use rust_webapp::*;
use rust_webapp::rust_dicore::ServiceCollection;
use tokio::sync::Mutex;

// 显式引用 handlers 与 domain crate，确保它们的 `#[inject]`
// 与 `inventory::submit!` 注册被链接进最终二进制（否则链接器可能丢弃未使用 crate）。
extern crate docbit_domain;
extern crate docbit_handlers;

#[tokio::main]
async fn main() {
    let host = Host::builder()
        .register(|svc| register_db_context(svc))
        .add_options::<SiteConfig>("Site")
        .add_authentication()
        .add_memory_cache()
        .build();

    host.run().await.expect("Server failed");
}

/// 注册 DbContext。
///
/// - `Development` → SQLite，路径 `<app_base>/app.db`
/// - `Production`  → MySQL，连接串来自 `DATABASE_URL` 环境变量
fn register_db_context(svc: ServiceCollection) -> ServiceCollection {
    let mut builder = DbContextOptionsBuilder::new();
    match AppMode::from_env() {
        AppMode::Production => {
            let cs = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable required in Production");
            builder.use_mysql(&cs);
            tracing::info!("[docbit] DbContext provider: MySQL");
        }
        AppMode::Development => {
            let path = app_base().join("app.db");
            tracing::info!("[docbit] SQLite path: {}", path.display());
            builder.use_sqlite(&path.to_string_lossy());
        }
    }
    let options = Arc::new(builder.build());
    // Scoped：每个 HTTP 请求获得独立的 DbContext（per-request unit-of-work）。
    // endpoint.rs 的 dispatch 通过 `provider.create_scope()` 创建请求级 Scope，
    // 此处注册的 Mutex<DbContext> 在该 Scope 内缓存，请求结束自动释放。
    // 这消除了全局单例导致的跨请求变更追踪污染、虚假并发争用与性能退化（R1）。
    svc.scoped::<Mutex<DbContext>>(move |_| {
        let ctx = DbContext::from_options(&options)
            .expect("Failed to create DbContext from options");
        Arc::new(Mutex::new(ctx))
    })
}
