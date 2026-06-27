//! Composition-root registrations for framework-adjacent infrastructure.
//!
//! 通过 `Host::builder().register(configure(mode))` 注册 rust-dicore 无法自动
//! 构造的依赖：`AppPaths`、`Mutex<DbContext>`、`SiteConfig`。
//!
//! 其他类型由 `#[rust_dicore::inject_attr]` 在编译期收集，`Host::build()` 自动注入：
//! - `IDocumentService` → `DocService`（host/src/doc_service.rs）
//! - `IDynamicAuthorizer` → `RoleAuthorizer`（host/src/authorizer.rs）
//! - `IHostedService` → `DbInitService`（host/src/startup.rs）
//! - `IRequestHandler<…>` → handlers crate 中的所有 handler
//!
//! `MemoryCache` 由 `Host::builder().use_memory_cache()` 注册，此处不再重复。

use std::sync::Arc;

use rust_dicore::ServiceCollection;
use rust_ef::db_context::DbContext;
use rust_webapp::AppMode;
use tokio::sync::Mutex;

use docbit_contracts::site::SiteConfig;

use crate::config::{
    build_db_options, create_db_context, load_database_config, load_site_config,
};
use crate::paths::AppPaths;

/// 注册所有基础设施依赖到 DI 容器。
pub fn configure(mode: AppMode) -> impl FnOnce(ServiceCollection) -> ServiceCollection {
    move |mut svc| {
        let paths = Arc::new(AppPaths::resolve());
        tracing::info!("[docbit] docs root: {}", paths.docs_root.display());
        tracing::info!("[docbit] blog root: {}", paths.blog_root.display());
        tracing::info!("[docbit] database: {}", paths.db_path.display());
        tracing::info!("[docbit] wwwroot: {}", paths.wwwroot.display());

        let db_config = load_database_config(mode);
        let options = build_db_options(mode, &paths, &db_config);
        let site_config = Arc::new(load_site_config(mode));

        let options_for_ctx = options.clone();
        let paths_for_di = paths.clone();
        let site_for_di = site_config.clone();

        svc = svc
            // AppPaths — 由 DocService / DbInitService 注入
            .singleton::<AppPaths>(move |_| Arc::clone(&paths_for_di))
            // Mutex<DbContext> - 由所有 handler / DbInitService 注入（Arc<Mutex<DbContext>>）
            .singleton::<Mutex<DbContext>>(move |_| {
                let ctx = create_db_context(&options_for_ctx);
                Arc::new(Mutex::new(ctx))
            })
            // SiteConfig - 由 SiteInfoHandler 注入
            .singleton::<SiteConfig>(move |_| Arc::clone(&site_for_di));

        svc
    }
}
