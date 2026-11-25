//! Auth context — register / login / me / forgot-password / reset-password.
//!
//! JWT claims carry `roles: Vec<String>` for multi-role RBAC; `sub` is the user's
//! UUID. Tokens are minted through the `ITokenIssuer` port, never by hand.

pub mod forgot_password;
pub mod login;
pub mod me;
pub mod queries;
pub mod register;
pub mod reset_password;

pub use forgot_password::ForgotPasswordHandler;
pub use login::LoginHandler;
pub use me::AuthMeHandler;
pub use register::RegisterHandler;
pub use reset_password::ResetPasswordHandler;
