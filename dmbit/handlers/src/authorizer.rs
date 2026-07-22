//! Role-based authorizer.

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

        if claims.has_role("admin") {
            return Ok(());
        }

        Err(Error::Http(format!(
            "无权限访问「{route_pattern}」，需要管理员角色"
        )))
    }
}
