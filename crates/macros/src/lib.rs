//! Procedural macros for the rust-webx framework (package `rust-webx-macros`,
//! imported as `webx_macros`).
//!
//! - `#[get]` / `#[post]` / `#[put]` / `#[delete]` / `#[endpoint]` — route registration
//! - `#[handler]` — handler auto-registration via `inventory`
//! - `#[webx::main]` / `#[webx::main(embed)]` — application entry + asset embedding
//! - `#[webx::embed_assets]` — asset table include for integration tests
//! - `#[derive(WebxRequestMeta)]` — OpenAPI parameter metadata
//! - `#[claims]` / `#[authorize]` — authentication and authorization

mod app_main;
mod claims;
mod endpoint;
mod handler;
mod request_meta;

use proc_macro::TokenStream;

/// Application entry — ASP.NET Core `Main` analogue (`#[tokio::main]`).
///
/// | Attribute | Baked-in wwwroot |
/// |-----------|------------------|
/// | `#[webx::main]` | No — disk SPA only (`.use_spa` / auto-detect) |
/// | `#[webx::main(embed)]` | Yes — requires `build.rs` `web_root(...).build()` |
///
/// ```ignore
/// // Disk-only (no build.rs embed step)
/// #[webx::main]
/// async fn main() {
///     Host::builder().use_spa("wwwroot").build().run().await.unwrap();
/// }
///
/// // Baked into the binary
/// // build.rs: webx::builder().web_root("wwwroot").build()?;
/// #[webx::main(embed)]
/// async fn main() {
///     Host::builder().use_spa("wwwroot").build().run().await.unwrap();
/// }
/// ```
///
/// - **`web_root` (build.rs)** — which files are compiled into the executable  
/// - **`.use_spa(...)` (runtime)** — disk directory consulted first (operator overrides)  
/// - Without `build.rs` / without `(embed)` — nothing is baked in
#[proc_macro_attribute]
pub fn main(attr: TokenStream, item: TokenStream) -> TokenStream {
    app_main::main_impl(attr, item)
}

/// Include the `OUT_DIR` asset table (for integration tests that never run `main`).
#[proc_macro_attribute]
pub fn embed_assets(attr: TokenStream, item: TokenStream) -> TokenStream {
    app_main::embed_assets_impl(attr, item)
}

// ---------------------------------------------------------------------------
// Route macros: full form + shortcuts
// ---------------------------------------------------------------------------

/// Full form: marks an IRequest impl with HTTP method and route path.
///
/// ```ignore
/// #[endpoint(HttpGet, "/users/{id}")]
/// impl IRequest<UserModel> for GetUserRequest {}
/// ```
#[proc_macro_attribute]
pub fn endpoint(attr: TokenStream, item: TokenStream) -> TokenStream {
    endpoint::endpoint_impl(attr, item)
}

/// Shortcut: registers an IRequest impl at GET /path.
///
/// ```ignore
/// #[get("/users/{id}")]
/// impl IRequest<UserModel> for GetUserRequest {}
/// ```
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    endpoint::shortcut_get(attr, item)
}

/// Shortcut: registers an IRequest impl at POST /path.
///
/// ```ignore
/// #[post("/users")]
/// impl IRequest<UserModel> for CreateUserRequest {}
/// ```
#[proc_macro_attribute]
pub fn post(attr: TokenStream, item: TokenStream) -> TokenStream {
    endpoint::shortcut_post(attr, item)
}

/// Shortcut: registers an IRequest impl at PUT /path.
///
/// ```ignore
/// #[put("/users/{id}")]
/// impl IRequest<UserModel> for UpdateUserRequest {}
/// ```
#[proc_macro_attribute]
pub fn put(attr: TokenStream, item: TokenStream) -> TokenStream {
    endpoint::shortcut_put(attr, item)
}

/// Shortcut: registers an IRequest impl at DELETE /path.
///
/// ```ignore
/// #[delete("/users/{id}")]
/// impl IRequest<String> for DeleteUserRequest {}
/// ```
#[proc_macro_attribute]
pub fn delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    endpoint::shortcut_delete(attr, item)
}

// ---------------------------------------------------------------------------
// Handler auto-registration
// ---------------------------------------------------------------------------

/// Auto-registers an IRequestHandler implementation at compile time via inventory.
///
/// Place on `impl IRequestHandler<T, R> for Handler` blocks.
/// The handler struct MUST implement `Default`.
///
/// ```ignore
/// #[handler]
/// #[async_trait]
/// impl IRequestHandler<HelloRequest, String> for HelloHandler {
///     async fn handle(&mut self, _req: HelloRequest) -> Result<String> { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    handler::handler_impl(attr, item)
}

// ---------------------------------------------------------------------------
// Parameter binding (OpenAPI metadata via WebxRequestMeta derive helpers)
// ---------------------------------------------------------------------------
//
// `from_query`, `from_route`, and `from_body` are helper attributes for
// `#[derive(WebxRequestMeta)]` — not standalone attribute macros (avoids
// rustc conflicts on struct fields).

/// Registers OpenAPI parameter metadata from `#[from_query]` / `#[from_route]` / `#[from_body]` / `#[from_form]` fields.
///
/// `FormFile` fields are recognised automatically and reported as
/// `multipart/form-data` binary parts.
///
/// Add `#[webx_request(query_all)]` to treat all non-skipped fields as query parameters.
#[proc_macro_derive(
    WebxRequestMeta,
    attributes(from_query, from_route, from_body, from_form, webx_request)
)]
pub fn webx_request_meta(input: TokenStream) -> TokenStream {
    request_meta::derive_request_meta(input)
}

// ---------------------------------------------------------------------------
// Declaration macros
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Claims attribute
// ---------------------------------------------------------------------------

/// Marks a request struct as carrying authentication claims.
///
/// Appends a `#[serde(skip)] pub claims: Option<Box<dyn IClaims>>` field to
/// the struct and generates an inherent `set_claims` method that shadows the
/// blanket no-op `IClaimsCarrier::set_claims`. The dispatcher injects claims
/// into the request *before* calling `IRequestHandler::handle`.
///
/// Must be the outermost attribute so that `#[derive(...)]` sees the injected
/// field:
///
/// ```ignore
/// #[claims]
/// #[derive(Default, Deserialize)]
/// pub struct CreateCommentRequest {
///     pub blog_id: i32,
///     pub content: String,
/// }
/// ```
#[proc_macro_attribute]
pub fn claims(attr: TokenStream, item: TokenStream) -> TokenStream {
    claims::claims_impl(attr, item)
}

// ---------------------------------------------------------------------------
// Authorization attribute (declarative, handled by emit_endpoint)
// ---------------------------------------------------------------------------

/// Declares authorization requirements on a route.
///
/// ```ignore
/// #[get("/api/users")]
/// #[authorize(role = "admin")]
/// impl IRequest<Vec<UserModel>> for ListUsersRequest {}
///
/// #[get("/api/auth/me")]
/// #[authorize]
/// impl IRequest<UserView> for AuthMeRequest {}
/// ```
#[proc_macro_attribute]
pub fn authorize(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass through — the attribute is read by emit_endpoint inside
    // the route macro (get, post, etc.) via item_impl.attrs.
    item
}
