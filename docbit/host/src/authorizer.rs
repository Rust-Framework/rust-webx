//! Role-based dynamic authorizer — gates admin routes.
//!
//! 通过 `#[rust_dicore::inject_attr]` 自动注册为 `dyn IDynamicAuthorizer` 单例。
//! 鉴权策略：
//! 1. `/api/auth/*` 公开接口直接放行；
//! 2. `/api/comments*` 评论接口允许已登录用户访问（所有权检查在 handler 内）；
//! 3. 其他 `/api/*` 管理接口要求 `admin` 角色；
//! 4. 公开 GET 接口（`/api/site`、`/api/blog`、`/api/exhibitions`、`/api/docs` 等）
//!    由 `#[authorize(role = "admin")]` 标注的请求才需要 admin，未标注的请求
//!    框架已放行，不会进入此 authorizer。

use rust_webapp::*;

#[rust_dicore::inject_attr(singleton, as = dyn IDynamicAuthorizer)]
pub struct RoleAuthorizer;

#[async_trait]
impl IDynamicAuthorizer for RoleAuthorizer {
    async fn authorize(
        &self,
        claims: &dyn IClaims,
        route_pattern: &str,
        _method: &str,
    ) -> Result<()> {
        if route_pattern.starts_with("/api/auth/") {
            return Ok(());
        }
        if route_pattern.contains("/comments") {
            return Ok(());
        }
        if claims.has_role("admin") {
            return Ok(());
        }
        Err(Error::Http(format!(
            "Forbidden: admin role required for '{}'",
            route_pattern
        )))
    }
}
