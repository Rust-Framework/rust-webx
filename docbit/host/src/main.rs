//! Docbit host crate entry point — composition root.
//!
//! 参考 ASP.NET Core 的极简启动：
//! - 运行模式（`APP_ENV`）→ 框架在 `Host::builder()` 内部自动读取
//! - appsettings 加载、环境 overlay 合并、配置文件定位 → 框架自动
//! - SPA 静态资源 → 框架自动检测 `wwwroot/`，无需 `use_spa`
//! - DbContext → 通过 `rust_ef::di::DbContextServiceCollectionExt::add_dbcontext` 注册为 Scoped
//!   （Development=SQLite, Production=MySQL），handler 以 bare `ctx: DbContext` 字段
//!   经 `get_owned` 解析为 owned 实例，实现 per-request unit-of-work。
//! - 应用专属配置（`SiteConfig`）→ 框架 `add_options::<SiteConfig>("Site")` 自动绑定
//! - handler / hosted service / dynamic authorizer / document service → `#[inject]` 编译期自动注册

mod startup;

use docbit_contracts::site::SiteConfig;
use rust_ef::di::DbContextServiceCollectionExt as _;
use rust_ef_mysql::DbContextOptionsBuilderExt as _;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use rust_webapp::*;
use rust_webapp::rust_dicore::ServiceCollection;

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
///
/// `add_dbcontext` 将 `DbContext` 注册为 **Scoped**：HTTP 管道自动为每个请求创建
/// 请求级 Scope，handler 的 bare `ctx: DbContext` 字段通过 `get_owned` 解析为
/// 全新 owned 实例（绕过 Scope 缓存），实现 EFCore 风格的 per-request
/// unit-of-work —— 无 `Arc<Mutex>`、无内部可变性、无跨请求变更追踪污染。
fn register_db_context(svc: ServiceCollection) -> ServiceCollection {
    svc.add_dbcontext(|opts| match AppMode::from_env() {
        AppMode::Production => {
            let cs = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable required in Production");
            opts.use_mysql(&cs);
            tracing::info!("[docbit] DbContext provider: MySQL");
        }
        AppMode::Development => {
            let path = app_base().join("app.db");
            tracing::info!("[docbit] SQLite path: {}", path.display());
            opts.use_sqlite(&path.to_string_lossy());
        }
    })
}
