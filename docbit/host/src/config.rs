//! Configuration — `SiteConfig` 加载与 `DbContextOptions` 构建。
//!
//! 运行模式（`AppMode`）与环境 overlay 的识别由框架统一处理：
//! - 框架 `HostBuilder::new()` 默认从 `APP_ENV` 环境变量解析模式；
//! - 框架 `load_appsettings(mode)` 自动合并 `appsettings.{Mode}.json` overlay；
//! - 框架 `read_json_file` 按 cwd → exe 同级 → 上溯目录顺序定位配置文件。
//!
//! 本模块仅消费框架已合并好的 appsettings，绑定 `SiteConfig` 与 `DatabaseConfig`，
//! 并据此构建 `DbContextOptions`。

use std::sync::Arc;

use rust_ef::db_context::{DbContext, DbContextOptions, DbContextOptionsBuilder};
use rust_ef_mysql::DbContextOptionsBuilderExt as MysqlExt;
use rust_ef_sqlite::DbContextOptionsBuilderExt as SqliteExt;
use rust_webapp::{load_appsettings, AppMode};

use docbit_contracts::site::SiteConfig;

use crate::interceptor::AuditInterceptor;
use crate::paths::AppPaths;

/// 从 appsettings 读取 `Site` 节并解析为 `SiteConfig`，失败时返回默认值。
pub fn load_site_config(mode: AppMode) -> SiteConfig {
    let appsettings = match load_appsettings(mode) {
        Some(v) => v,
        None => {
            tracing::warn!("[Config] appsettings.json not found; using default SiteConfig");
            return SiteConfig::default();
        }
    };
    appsettings
        .get("Site")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// 数据库配置节。
#[derive(Debug, Clone, Default)]
pub struct DatabaseConfig {
    /// MySQL 连接串；为空则回退到 SQLite。
    pub connection_string: String,
}

pub fn load_database_config(mode: AppMode) -> DatabaseConfig {
    let appsettings = match load_appsettings(mode) {
        Some(v) => v,
        None => return DatabaseConfig::default(),
    };
    let cs = appsettings
        .get("Database")
        .and_then(|v| v.get("ConnectionString"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    DatabaseConfig {
        connection_string: cs,
    }
}

/// 根据 `AppMode` 构建合适的 `DbContextOptions`：
/// - Production 且配置了连接串 → MySQL
/// - 否则 → SQLite（开发默认）
pub fn build_db_options(
    mode: AppMode,
    paths: &AppPaths,
    db_config: &DatabaseConfig,
) -> Arc<DbContextOptions> {
    let mut builder = DbContextOptionsBuilder::new();
    if mode == AppMode::Production && !db_config.connection_string.is_empty() {
        tracing::info!("[Config] Using MySQL provider (production)");
        builder
            .use_mysql(&db_config.connection_string)
            .add_interceptor(AuditInterceptor);
    } else {
        tracing::info!(
            "[Config] Using SQLite provider (development): {}",
            paths.db_path.display()
        );
        builder
            .use_sqlite(paths.db_path.to_string_lossy().as_ref())
            .add_interceptor(AuditInterceptor);
    }
    Arc::new(builder.build())
}

/// 从 options 创建 `DbContext`，失败时 panic（启动期错误）。
pub fn create_db_context(options: &DbContextOptions) -> DbContext {
    DbContext::from_options(options).expect("Failed to create DbContext")
}
