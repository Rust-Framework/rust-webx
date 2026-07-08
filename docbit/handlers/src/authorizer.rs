//! Role-based dynamic authorizer — gates admin routes.
//!
//! 通过 `#[inject]` 自动注册为 `dyn IDynamicAuthorizer` 单例，
//! 框架在 `Host::build()` 时统一收集并应用到所有 `#[authorize]` 路由。
//!
//! 鉴权策略：
//! 1. `/api/auth/*` 公开接口直接放行；
//! 2. `/api/comments*` 评论接口允许已登录用户访问（所有权检查在 handler 内）；
//! 3. 其他 `/api/*` 管理接口要求 `admin` 角色。

use rust_webx::*;

#[derive(Inject)]
pub struct RoleAuthorizer;

#[inject]
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
