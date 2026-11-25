//! RBAC context — Role / Resource / Authorize / RoleUser contracts.
//!
//! The join tables (`role_user`, `authorize`) are hard-deleted; the principal
//! tables (`role`, `resource`) are soft-deleted.

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
pub mod models;
pub mod revoke_role;
pub mod update_resource;
pub mod update_role;

pub use assign_role::AssignRoleRequest;
pub use create_authorize::CreateAuthorizeRequest;
pub use create_resource::CreateResourceRequest;
pub use create_role::CreateRoleRequest;
pub use delete_authorize::DeleteAuthorizeRequest;
pub use delete_resource::DeleteResourceRequest;
pub use delete_role::DeleteRoleRequest;
pub use list_authorizes::ListAuthorizesRequest;
pub use list_resources::ListResourcesRequest;
pub use list_roles::ListRolesRequest;
pub use models::{AuthorizeModel, ResourceModel, RoleModel};
pub use revoke_role::RevokeRoleRequest;
pub use update_resource::UpdateResourceRequest;
pub use update_role::UpdateRoleRequest;
