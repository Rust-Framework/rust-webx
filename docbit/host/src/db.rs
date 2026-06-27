//! 应用级 DbContext 注册 —— docbit 自己的数据访问配置。
//!
//! 参考 ASP.NET Core 架构：Web 框架（`rust-webapp-host`）本身不依赖 EF Core，
//! `AddDbContext` 是应用在 startup 里调用的扩展方法。本模块即 docbit 的等价物：
//! 给 `HostBuilder` 加 `add_dbcontext(|o| ...)`，封装 appsettings 读取、
//! provider 自动选择与 DbContext 注册。
//!
//! ## Provider 自动识别
//!
//! 从 `appsettings.{Mode}.json` 的 `Database:ConnectionString` 读取连接串，
//! 按前缀自动选择 Provider：
//!
//! | 连接串                              | Provider | 说明                         |
//! |-------------------------------------|----------|------------------------------|
//! | `mysql://user:pwd@host/db`          | MySQL    | 前缀 `mysql://`              |
//! | 其它非空值（如 `app.db`、`./data.db`）| SQLite   | 视为文件路径                 |
//! | 空 / 未配置                          | SQLite   | 默认 `<app_base>/app.db`     |
//!
//! ## 用法
//!
//! ```ignore
//! Host::builder()
//!     .add_dbcontext(|o| {
//!         o.add_interceptor(AuditInterceptor);
//!     })
//!     .add_authentication()
//!     .build()
//! ```

use std::sync::Arc;

use rust_ef::db_context::{DbContext, DbContextOptions, DbContextOptionsBuilder};
use rust_ef_mysql::DbContextOptionsBuilderExt as _;
use rust_ef_sqlite::DbContextOptionsBuilderExt as _;
use rust_webapp::{app_base, load_appsettings, AppMode, HostBuilder};
use rust_webapp::rust_dicore::ServiceCollection;
use tokio::sync::Mutex;

/// SQLite 默认数据库文件名（appsettings 未配置 Database 时的回退路径，相对 app_base）。
const DEFAULT_SQLITE_FILENAME: &str = "app.db";
const DATABASE_SECTION: &str = "Database";
const CONNECTION_STRING_KEY: &str = "ConnectionString";

/// docbit 应用给 `HostBuilder` 加的 EF 注册扩展。
///
/// 框架（rust-webapp-host）本身不依赖 EF，本 trait 由应用层定义，
/// 让 `Host::builder().add_dbcontext(|o| ...)` 链式调用可用。
pub trait HostBuilderDbExt {
    /// 自动注册 DbContext。Provider 由 `appsettings` 的 `Database:ConnectionString`
    /// 自动识别（见模块文档表格）。闭包接收已配置好 Provider 的 builder，
    /// 用于追加拦截器等自定义项；闭包内也可调用 `use_sqlite`/`use_mysql` 覆盖。
    fn add_dbcontext<F>(self, configure: F) -> Self
    where
        F: FnOnce(&mut DbContextOptionsBuilder) + Send + 'static;
}

impl HostBuilderDbExt for HostBuilder {
    fn add_dbcontext<F>(self, configure: F) -> Self
    where
        F: FnOnce(&mut DbContextOptionsBuilder) + Send + 'static,
    {
        let mode = AppMode::from_env();
        self.register(move |svc| register_db_context(svc, mode, configure))
    }
}

/// 读取 `Database:ConnectionString`（应用 overlay 与 env override 后的最终值）。
fn read_connection_string(mode: AppMode) -> String {
    let appsettings = match load_appsettings(mode) {
        Some(v) => v,
        None => return String::new(),
    };
    appsettings
        .get(DATABASE_SECTION)
        .and_then(|v| v.get(CONNECTION_STRING_KEY))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// 按连接串构建 DbContextOptions，注册 `Arc<Mutex<DbContext>>`。
fn register_db_context<F>(
    mut svc: ServiceCollection,
    mode: AppMode,
    configure: F,
) -> ServiceCollection
where
    F: FnOnce(&mut DbContextOptionsBuilder),
{
    let cs = read_connection_string(mode);
    let options = build_db_options(&cs, configure);

    tracing::info!("[docbit] DbContext provider: {}", provider_name(&cs));

    svc = svc.singleton::<Mutex<DbContext>>(move |_| {
        let ctx = DbContext::from_options(&options)
            .expect("Failed to create DbContext from options");
        Arc::new(Mutex::new(ctx))
    });

    svc
}

/// 根据连接串构建 `DbContextOptions`：
/// - `mysql://` 前缀 → MySQL
/// - 其它非空 → SQLite（视为文件路径）
/// - 空 → SQLite 默认 `<app_base>/<DEFAULT_SQLITE_FILENAME>`
///
/// 应用闭包在 provider 选定后、`build()` 之前执行，可追加拦截器或覆盖 provider。
fn build_db_options<F>(connection_string: &str, configure: F) -> Arc<DbContextOptions>
where
    F: FnOnce(&mut DbContextOptionsBuilder),
{
    let mut builder = DbContextOptionsBuilder::new();

    if connection_string.starts_with("mysql://") {
        builder.use_mysql(connection_string);
    } else if !connection_string.is_empty() {
        builder.use_sqlite(connection_string);
    } else {
        let path = app_base().join(DEFAULT_SQLITE_FILENAME);
        tracing::info!("[docbit] SQLite default path: {}", path.display());
        builder.use_sqlite(&path.to_string_lossy());
    }

    configure(&mut builder);
    Arc::new(builder.build())
}

/// 供日志展示的 provider 名称。
fn provider_name(connection_string: &str) -> &'static str {
    if connection_string.starts_with("mysql://") {
        "MySQL"
    } else if connection_string.is_empty() {
        "SQLite (default app.db)"
    } else {
        "SQLite"
    }
}
