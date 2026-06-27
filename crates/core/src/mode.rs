//! Application mode: Development vs Production.
//!
//! Controls framework behavior such as startup banner verbosity
//! and built-in request logging.
//!
//! 环境识别遵循 ASP.NET Core 约定：
//! 1. 环境变量 `APP_ENV`（不区分大小写）显式指定，取值 `Production`/`Prod` 或
//!    `Development`/`Dev`；
//! 2. 未设置时默认 `Development`。
//!
//! 框架据此自动加载 `appsettings.{Environment}.json` overlay。

/// Controls how the framework behaves at startup and at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Full startup banner, per-request console logging, verbose diagnostics.
    #[default]
    Development,
    /// Minimal one-line startup message, no framework-initiated console logging.
    Production,
}

impl AppMode {
    /// 环境模式对应的 overlay 文件名，如 `appsettings.Development.json`。
    pub fn overlay_filename(&self) -> &'static str {
        match self {
            AppMode::Development => "appsettings.Development.json",
            AppMode::Production => "appsettings.Production.json",
        }
    }

    /// 从环境变量 `APP_ENV` 解析运行模式；未设置或值非法时返回 `Development`。
    ///
    /// 接受（不区分大小写）：`Production` / `Prod` / `Development` / `Dev`。
    pub fn from_env() -> Self {
        Self::from_env_var("APP_ENV")
    }

    /// 从指定环境变量名解析运行模式。
    pub fn from_env_var(var: &str) -> Self {
        match std::env::var(var) {
            Ok(raw) => match raw.trim().to_lowercase().as_str() {
                "production" | "prod" => AppMode::Production,
                "development" | "dev" => AppMode::Development,
                _ => {
                    eprintln!(
                        "[AppMode] Invalid {}='{}'; expected Production/Development. Falling back to Development.",
                        var, raw
                    );
                    AppMode::Development
                }
            },
            Err(_) => AppMode::Development,
        }
    }
}
