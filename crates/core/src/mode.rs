//! Application mode: Development vs Production.
//!
//! Controls framework behavior such as startup banner verbosity
//! and built-in request logging.
//!
//! 环境识别遵循 ASP.NET Core 约定，但**默认值偏向安全**：
//! 1. 环境变量 `APP_ENV`（不区分大小写）显式指定，取值 `Production`/`Prod` 或
//!    `Development`/`Dev`；
//! 2. 未设置或值非法时默认 `Production`，并输出**一次**警告 —— 安全开关不能
//!    取决于“环境变量忘了写”；本机开发必须显式 `APP_ENV=Development`。
//!
//! 框架据此自动加载 `appsettings.{Environment}.json` overlay。

use std::sync::Once;

/// Controls how the framework behaves at startup and at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Full startup banner, per-request console logging, verbose diagnostics.
    ///
    /// Opt-in only (`APP_ENV=Development`): local development must say so
    /// explicitly.
    Development,
    /// Minimal one-line startup message, no framework-initiated console logging.
    ///
    /// The default: a forgotten `APP_ENV` keeps the production guards on instead
    /// of silently weakening them.
    #[default]
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

    /// 从环境变量 `APP_ENV` 解析运行模式；未设置或值非法时返回 `Production`。
    ///
    /// 接受（不区分大小写）：`Production` / `Prod` / `Development` / `Dev`。
    /// 未设置或值非法时输出一次 `tracing::warn!`：安全默认值必须显式关掉，
    /// 本机开发设置 `APP_ENV=Development` 即可。
    pub fn from_env() -> Self {
        Self::from_env_var("APP_ENV")
    }

    /// 从指定环境变量名解析运行模式。
    pub fn from_env_var(var: &str) -> Self {
        match std::env::var(var) {
            Ok(raw) => match Self::parse(&raw) {
                Some(mode) => mode,
                None => {
                    warn_invalid_once(var, &raw);
                    AppMode::Production
                }
            },
            Err(_) => {
                warn_unset_once(var);
                AppMode::Production
            }
        }
    }

    /// 解析环境变量取值；`Production`/`Prod`、`Development`/`Dev`（不区分大小写，
    /// 允许首尾空白），其它取值返回 `None`。
    fn parse(raw: &str) -> Option<AppMode> {
        match raw.trim().to_lowercase().as_str() {
            "production" | "prod" => Some(AppMode::Production),
            "development" | "dev" => Some(AppMode::Development),
            _ => None,
        }
    }
}

/// 未设置环境变量时的警告：每次进程只提示一次，避免每次 `from_env()` 刷屏。
fn warn_unset_once(var: &str) {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "{var} is not set; defaulting to Production. \
             Set APP_ENV=Development for local development."
        );
    });
}

/// 环境变量取值非法时的警告：每次进程只提示一次（合法值不受影响）。
fn warn_invalid_once(var: &str, raw: &str) {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "Invalid {var}={raw:?}; expected Production/Prod or Development/Dev. \
             Falling back to Production."
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_production() {
        // Safe default: a missing APP_ENV keeps the production guards armed.
        assert_eq!(AppMode::default(), AppMode::Production);
    }

    #[test]
    fn parse_accepts_known_values_case_insensitively() {
        assert_eq!(AppMode::parse("Production"), Some(AppMode::Production));
        assert_eq!(AppMode::parse("prod"), Some(AppMode::Production));
        assert_eq!(AppMode::parse("  PROD  "), Some(AppMode::Production));
        assert_eq!(AppMode::parse("Development"), Some(AppMode::Development));
        assert_eq!(AppMode::parse("DEV"), Some(AppMode::Development));
    }

    #[test]
    fn parse_rejects_unknown_values() {
        assert_eq!(AppMode::parse("staging"), None);
        assert_eq!(AppMode::parse(""), None);
        assert_eq!(AppMode::parse("1"), None);
    }

    #[test]
    fn from_env_var_unset_falls_back_to_production() {
        // A variable name that is never set: no `set_var`, so no cross-test
        // environment pollution (the warning is emitted once per process).
        let mode = AppMode::from_env_var("WEBX_TEST_APP_ENV_THAT_IS_NEVER_SET");
        assert_eq!(mode, AppMode::Production);
    }

    #[test]
    fn from_env_var_invalid_falls_back_to_production() {
        std::env::set_var("WEBX_TEST_APP_ENV_INVALID", "staging");
        let mode = AppMode::from_env_var("WEBX_TEST_APP_ENV_INVALID");
        std::env::remove_var("WEBX_TEST_APP_ENV_INVALID");
        assert_eq!(mode, AppMode::Production);
    }

    #[test]
    fn from_env_var_valid_value_is_honoured() {
        std::env::set_var("WEBX_TEST_APP_ENV_PRODUCTION", "prod");
        let mode = AppMode::from_env_var("WEBX_TEST_APP_ENV_PRODUCTION");
        std::env::remove_var("WEBX_TEST_APP_ENV_PRODUCTION");
        assert_eq!(mode, AppMode::Production);
    }

    #[test]
    fn from_env_var_explicit_development_opts_out() {
        std::env::set_var("WEBX_TEST_APP_ENV_DEVELOPMENT", "dev");
        let mode = AppMode::from_env_var("WEBX_TEST_APP_ENV_DEVELOPMENT");
        std::env::remove_var("WEBX_TEST_APP_ENV_DEVELOPMENT");
        assert_eq!(mode, AppMode::Development);
    }
}
