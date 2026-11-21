//! CSRF protection for cookie-authenticated requests (double-submit).
//!
//! A browser session cookie is sent automatically, so a cross-site form/script
//! could ride on it. The double-submit pattern defeats that without
//! server-side token storage:
//!
//! 1. Login sets a **readable** CSRF cookie (`__Host-webx_csrf`) alongside the
//!    `HttpOnly` session cookie.
//! 2. The client mirrors the token in the `X-CSRF-Token` header on unsafe
//!    methods.
//! 3. This middleware rejects unsafe requests whose cookie and header differ.
//!
//! The check only applies when the request actually carries the session
//! cookie; pure Bearer/API-key traffic is unaffected. `Sec-Fetch-Site:
//! cross-site` is rejected as defense in depth when the browser sends it.
//!
//! The host refuses to start when a `session-cookie` scheme is registered
//! without [`CsrfMiddleware`] — a cookie login without CSRF protection is a
//! deployment error, not a default.

use std::ops::ControlFlow;
use std::time::Duration;

use webx_core::error::{Error, Result};
use webx_core::http::IHttpContext;
use webx_core::middleware::IMiddleware;
use webx_core::security::constant_time_eq;

use crate::auth_cookie::{cookie_value, SameSite};

/// CSRF cookie/header configuration.
#[derive(Debug, Clone)]
pub struct CsrfConfig {
    /// Session cookie whose presence enables the check. Must match
    /// [`CookieOptions::name`](crate::auth_cookie::CookieOptions::name).
    pub session_cookie_name: String,
    /// Readable cookie that carries the CSRF token.
    pub cookie_name: String,
    /// Header the client must echo the token in.
    pub header_name: String,
    /// Cookie path.
    pub path: String,
    /// `Secure` attribute for the CSRF cookie.
    pub secure: bool,
    /// `SameSite` attribute for the CSRF cookie.
    pub same_site: SameSite,
    /// Reject requests whose `Sec-Fetch-Site` is `cross-site`.
    pub check_fetch_metadata: bool,
    /// CSRF cookie lifetime.
    pub max_age: Duration,
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            session_cookie_name: "__Host-webx_session".to_string(),
            cookie_name: "__Host-webx_csrf".to_string(),
            header_name: "x-csrf-token".to_string(),
            path: "/".to_string(),
            secure: true,
            same_site: SameSite::Lax,
            check_fetch_metadata: true,
            max_age: Duration::from_secs(12 * 60 * 60),
        }
    }
}

impl CsrfConfig {
    /// The double-submit configuration with safe defaults.
    pub fn double_submit() -> Self {
        Self::default()
    }

    /// Validate names/path/attributes. The host calls this at startup.
    pub fn validate(&self) -> Result<()> {
        validate_name("session_cookie_name", &self.session_cookie_name)?;
        validate_name("cookie_name", &self.cookie_name)?;
        if self.header_name.trim().is_empty() {
            return Err(Error::Di("csrf header_name must not be empty".to_string()));
        }
        if !self.path.starts_with('/') {
            return Err(Error::Di(
                "csrf cookie path must start with '/'".to_string(),
            ));
        }
        if self.same_site == SameSite::None && !self.secure {
            return Err(Error::Di(
                "SameSite=None requires a Secure CSRF cookie".to_string(),
            ));
        }
        if self.cookie_name.starts_with("__Host-") && (!self.secure || self.path != "/") {
            return Err(Error::Di(
                "__Host- CSRF cookies require Secure and Path=/".to_string(),
            ));
        }
        if self.max_age.is_zero() {
            return Err(Error::Di(
                "csrf max_age must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }

    /// Mint a fresh CSRF token (256 bits, hex).
    pub fn issue_token(&self) -> String {
        format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        )
    }

    /// `Set-Cookie` value carrying `token`. Deliberately **not** `HttpOnly`:
    /// the browser-side app must read it and copy it into the header.
    pub fn set_cookie(&self, token: &str) -> String {
        let mut cookie = format!(
            "{}={}; Path={}; Max-Age={}",
            self.cookie_name,
            token,
            self.path,
            self.max_age.as_secs()
        );
        if self.secure {
            cookie.push_str("; Secure");
        }
        cookie.push_str("; SameSite=");
        cookie.push_str(self.same_site.as_str());
        cookie
    }

    /// `Set-Cookie` value that clears the CSRF cookie.
    pub fn clear_cookie(&self) -> String {
        let mut cookie = format!("{}=; Path={}; Max-Age=0", self.cookie_name, self.path);
        if self.secure {
            cookie.push_str("; Secure");
        }
        cookie.push_str("; SameSite=");
        cookie.push_str(self.same_site.as_str());
        cookie
    }
}

fn validate_name(label: &str, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::Di(format!("csrf {label} must not be empty")));
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, ' ' | ';' | '=' | ','))
    {
        return Err(Error::Di(format!(
            "csrf {label} '{name}' contains an invalid character"
        )));
    }
    Ok(())
}

/// Double-submit CSRF middleware.
pub struct CsrfMiddleware {
    config: CsrfConfig,
}

impl CsrfMiddleware {
    /// Build the middleware; validates [`CsrfConfig`].
    pub fn new(config: CsrfConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    /// The configuration in effect.
    pub fn config(&self) -> &CsrfConfig {
        &self.config
    }
}

#[async_trait::async_trait]
impl IMiddleware for CsrfMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        if is_safe_method(ctx.request().method()) {
            return Ok(ControlFlow::Continue(()));
        }

        // Only cookie-authenticated requests need CSRF protection. Bearer and
        // API-key clients never send the session cookie.
        let Some(cookie_header) = ctx.request().header("cookie") else {
            return Ok(ControlFlow::Continue(()));
        };
        if cookie_value(cookie_header, &self.config.session_cookie_name).is_none() {
            return Ok(ControlFlow::Continue(()));
        }

        if self.config.check_fetch_metadata {
            if let Some(site) = ctx.request().header("sec-fetch-site") {
                if site.eq_ignore_ascii_case("cross-site") {
                    return Err(Error::Forbidden(
                        "cross-site request rejected by CSRF protection".to_string(),
                    ));
                }
            }
        }

        let token = cookie_value(cookie_header, &self.config.cookie_name).ok_or_else(|| {
            Error::Forbidden("CSRF cookie missing on a cookie-authenticated request".to_string())
        })?;
        let presented = ctx
            .request()
            .header(&self.config.header_name)
            .ok_or_else(|| {
                Error::Forbidden(
                    "CSRF header missing on a cookie-authenticated request".to_string(),
                )
            })?;
        if !constant_time_eq(token.as_bytes(), presented.as_bytes()) {
            return Err(Error::Forbidden("CSRF token mismatch".to_string()));
        }
        Ok(ControlFlow::Continue(()))
    }
}

fn is_safe_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS" | "TRACE"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_cookie_is_readable() {
        let config = CsrfConfig::default();
        config.validate().unwrap();
        let cookie = config.set_cookie("tok");
        assert!(!cookie.contains("HttpOnly"), "the app must read the token");
        assert!(cookie.contains("Secure"));
    }

    #[test]
    fn safe_methods_are_exempt() {
        for method in ["GET", "head", "OPTIONS", "TRACE"] {
            assert!(is_safe_method(method), "{method} must be exempt");
        }
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            assert!(!is_safe_method(method), "{method} must be checked");
        }
    }

    #[test]
    fn invalid_names_are_rejected() {
        let config = CsrfConfig {
            cookie_name: "bad name".to_string(),
            ..CsrfConfig::default()
        };
        assert!(config.validate().is_err());
    }
}
