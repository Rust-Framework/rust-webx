//! Procedural macros for the rust-webx framework (package `rust-webx-macros`,
//! imported as `webx_macros`).
//!
//! - `#[get]` / `#[post]` / `#[put]` / `#[delete]` / `#[endpoint]` — route registration
//! - `#[handler]` — handler auto-registration via `inventory`
//! - `#[webx::main]` / `#[webx::main(embed)]` — application entry + asset embedding
//! - `#[webx::embed_assets]` — asset table include for integration tests
//! - `#[derive(WebxRequestMeta)]` — OpenAPI parameter metadata
//! - `#[authorize]` — declarative route authorization
//! - `#[display]` / `#[description]` — endpoint name and description resource
//!   references
//! - `#[category]` — optional display grouping for endpoint listings
//!
//! Endpoint text priority is `#[display]` / `#[description]` first, then the
//! `impl` block's `///` doc comment. A doc comment's first line is the name
//! (OpenAPI `summary`); the full comment is the description. When there is no
//! text at all, the name is derived from the request type.
//!
//! `#[display(key = "endpoint.blog.list.name")]` stores a resource key; the
//! translation lives in a resource file and is resolved by the host's
//! `Culture.Default`. `#[display("博客列表")]` stores a literal instead.

mod app_main;
mod endpoint;
mod handler;
mod request_meta;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_quote;

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
///     async fn handle(&mut self, _req: HelloRequest) -> ActionResult<String> { ... }
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
// `frompath`, `fromquery`, `frombody`, `fromform`, `fromfile`, `fromheader`
// and `fromclaim` are helper attributes for `#[derive(WebxRequestMeta)]` — not
// standalone attribute macros (avoids rustc conflicts on struct fields).

/// Registers parameter metadata and the request binder from `#[from*]` fields.
///
/// Markers: `#[frompath]`, `#[fromquery]`, `#[frombody]`, `#[fromform]`,
/// `#[fromfile]`, `#[fromheader("Name")]`, `#[fromclaim("key")]`. Every marker
/// except `#[frombody]` accepts an optional source key.
///
/// # Identity fields (fail-closed)
///
/// The authenticated identity is a normal request field of type `IdentityUser`.
/// Anonymous access must be declared explicitly, so a request that silently
/// works for anonymous callers cannot happen by accident:
///
/// | Field | Declaration | Anonymous callers |
/// |-------|-------------|-------------------|
/// | `user: IdentityUser` | `#[fromclaim]` | rejected with `401` |
/// | `user: Option<IdentityUser>` | `#[fromclaim(optional)]` | allowed, filled with `None` |
///
/// A bare `#[fromclaim]` on `Option<IdentityUser>` is a compile error, and so is
/// `#[fromclaim(optional)]` on anything else. A request may carry at most one
/// identity field. The derive also implements `RequestUser` for that field.
///
/// Unmarked fields follow the request method: `GET`/`HEAD`/`DELETE`/`OPTIONS`
/// bind from the query string, then the route; `POST`/`PUT`/`PATCH` bind from
/// the JSON body, then the form, then the query string. Headers and claims are
/// opt-in only. `#[request(queryall)]` forces unmarked fields to the query.
///
/// The derive also registers the request type's `TypeId`, so OpenAPI resolves
/// its parameter metadata unambiguously — a qualified `self_ty`
/// (`contracts::ListOrdersRequest`) or two same-named request types in
/// different modules/crates never lose or share parameters.
#[proc_macro_derive(
    WebxRequestMeta,
    attributes(
        frompath, fromquery, frombody, fromform, fromfile, fromheader, fromclaim, request
    )
)]
pub fn webx_request_meta(input: TokenStream) -> TokenStream {
    request_meta::derive_request_meta(input)
}

// ---------------------------------------------------------------------------
// Declaration macros
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Authorization attribute (declarative, handled by emit_endpoint)
// ---------------------------------------------------------------------------

/// Declares authorization requirements on a route.
///
/// Each attribute is one rule and **every** rule must be satisfied. Inside a
/// rule, `roles` / `permissions` are any-of lists and the two dimensions are
/// ANDed; a bare `#[authorize]` requires a valid authenticated principal.
///
/// ```ignore
/// #[get("/api/users")]
/// #[authorize(role = "admin")]
/// impl IRequest<Vec<UserModel>> for ListUsersRequest {}
///
/// #[get("/api/auth/me")]
/// #[authorize]
/// impl IRequest<UserView> for AuthMeRequest {}
///
/// // Either role is enough.
/// #[get("/api/reports")]
/// #[authorize(roles = ["admin", "auditor"])]
/// impl IRequest<Vec<Report>> for ListReportsRequest {}
///
/// // Both rules must hold: admin role and the export permission.
/// #[post("/api/reports/export")]
/// #[authorize(role = "admin")]
/// #[authorize(permission = "reports:export")]
/// impl IRequest<ResponseData> for ExportReportsRequest {}
/// ```
#[proc_macro_attribute]
pub fn authorize(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass through — the attribute is read by emit_endpoint inside
    // the route macro (get, post, etc.) via item_impl.attrs.
    item
}

// ---------------------------------------------------------------------------
// Endpoint text attributes (declarative, handled by emit_endpoint)
// ---------------------------------------------------------------------------

/// Declares an endpoint's display name as a resource reference.
///
/// `#[display("literal")]` is a fixed name. `#[display(key = "...")]` resolves
/// the key from the host's resource files; `text` supplies a literal fallback
/// when the key is absent. Localization is a host-wide culture setting, not one
/// attribute per language.
///
/// ```ignore
/// #[get("/api/blog")]
/// #[display(key = "endpoint.blog.list.name", text = "博客模块-博客列表")]
/// impl IRequest<Vec<BlogModel>> for ListBlogRequest {}
/// ```
#[proc_macro_attribute]
pub fn display(attr: TokenStream, item: TokenStream) -> TokenStream {
    rewrite_as_hidden(attr, item, "__webx_display", "display")
}

/// Declares an endpoint's long description as a resource reference.
///
/// Takes priority over the impl block's `///` doc comment. Accepts the same
/// forms as [`display`].
///
/// ```ignore
/// #[get("/api/blog")]
/// #[description(key = "endpoint.blog.list.description")]
/// impl IRequest<Vec<BlogModel>> for ListBlogRequest {}
/// ```
#[proc_macro_attribute]
pub fn description(attr: TokenStream, item: TokenStream) -> TokenStream {
    rewrite_as_hidden(attr, item, "__webx_description", "description")
}

/// Declares an optional display grouping for the endpoint.
///
/// The category is metadata for endpoint listings (permission trees, admin
/// navigation, OpenAPI tags); it does not affect routing or authorization.
/// It accepts the same forms as [`display`].
///
/// ```ignore
/// #[get("/api/blog")]
/// #[category("博客模块")]
/// impl IRequest<Vec<BlogModel>> for ListBlogRequest {}
/// ```
#[proc_macro_attribute]
pub fn category(attr: TokenStream, item: TokenStream) -> TokenStream {
    rewrite_as_hidden(attr, item, "__webx_category", "category")
}

/// Rewrite an endpoint-text attribute into a hidden attribute the route macro
/// can read regardless of declaration order.
///
/// Attribute macros expand outermost-first, so a `#[display]` above `#[post]`
/// runs first and would otherwise be invisible to the route macro. Carrying the
/// same arguments on `#[webx::__webx_display(...)]` lets the route macro read it
/// in either order; the hidden attribute is a no-op passthrough afterwards.
fn rewrite_as_hidden(
    attr: TokenStream,
    item: TokenStream,
    hidden: &str,
    public: &str,
) -> TokenStream {
    let mut item_impl: syn::ItemImpl = match syn::parse(item) {
        Ok(item_impl) => item_impl,
        Err(_) => {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("`#[{public}]` must be placed on the `impl` block of a route"),
            )
            .to_compile_error()
            .into()
        }
    };

    let hidden = syn::Ident::new(hidden, proc_macro2::Span::call_site());
    let args = proc_macro2::TokenStream::from(attr);
    let hidden_attr: syn::Attribute = parse_quote!(#[::webx::#hidden(#args)]);
    item_impl.attrs.push(hidden_attr);

    quote!(#item_impl).into()
}

/// Internal carrier for [`display`] — do not use directly.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn __webx_display(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Internal carrier for [`description`] — do not use directly.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn __webx_description(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Internal carrier for [`category`] — do not use directly.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn __webx_category(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
