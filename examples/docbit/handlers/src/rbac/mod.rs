//! RBAC context — Role / Resource / Authorize / RoleUser handlers.
//!
//! Principal tables (`role`, `resource`) are soft-deleted; join tables
//! (`role_user`, `authorize`) are hard-deleted with a typed predicate.

pub mod assign_role;
pub mod create_authorize;
pub mod create_resource;
pub mod create_role;
pub mod delete_authorize;
pub mod delete_resource;
pub mod delete_role;
pub mod list_authorizes;
pub mod list_resources;
pub mod list_roles;
pub mod revoke_role;
pub mod update_resource;
pub mod update_role;

pub use assign_role::AssignRoleHandler;
pub use create_authorize::CreateAuthorizeHandler;
pub use create_resource::CreateResourceHandler;
pub use create_role::CreateRoleHandler;
pub use delete_authorize::DeleteAuthorizeHandler;
pub use delete_resource::DeleteResourceHandler;
pub use delete_role::DeleteRoleHandler;
pub use list_authorizes::ListAuthorizesHandler;
pub use list_resources::ListResourcesHandler;
pub use list_roles::ListRolesHandler;
pub use revoke_role::RevokeRoleHandler;
pub use update_resource::UpdateResourceHandler;
pub use update_role::UpdateRoleHandler;
