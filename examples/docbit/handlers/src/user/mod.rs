//! User context — admin CRUD with audit fields and soft delete.

pub mod create_user;
pub mod delete_user;
pub mod get_user;
pub mod info;
pub mod list_users;
pub mod update_user;

pub use create_user::CreateUserHandler;
pub use delete_user::DeleteUserHandler;
pub use get_user::GetUserHandler;
pub use info::InfoHandler;
pub use list_users::ListUsersHandler;
pub use update_user::UpdateUserHandler;
