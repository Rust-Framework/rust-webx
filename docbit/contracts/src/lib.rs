//! Docbit contracts crate — DTOs, request types, and service traits.
//!
//! This crate is the innermost layer and must not depend on rust-ef.

pub mod auth;
pub mod blog;
pub mod cache;
pub mod category;
pub mod comment;
pub mod docs;
pub mod exhibition;
pub mod rbac;
pub mod site;
pub mod tracking;
pub mod user;

/// Generates an inherent `set_claims` method that shadows the blanket
/// no-op `IClaimsCarrier::set_claims` from the framework.
///
/// ```ignore
/// #[derive(Default, Deserialize)]
/// pub struct CreateBlogPostRequest {
///     #[serde(skip)]
///     pub claims: Option<Box<dyn IClaims>>,
///     // ...
/// }
/// impl_claims_carrier!(CreateBlogPostRequest);
/// ```
#[macro_export]
macro_rules! impl_claims_carrier {
    ($ty:ty) => {
        impl $ty {
            pub fn set_claims(&mut self, claims: Option<Box<dyn ::rust_webapp::IClaims>>) {
                self.claims = claims;
            }
        }
    };
}
