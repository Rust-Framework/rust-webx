// rust-webapp-macros — Procedural macros for the Rust WebApi framework.
// - #[controller] / #[controller("/base")]
// - #[endpoint(HttpMethod, "/path")] — full form
// - #[get("/path")], #[post("/path")], #[put("/path")], #[delete("/path")] — shortcuts
// - #[HttpGet], #[HttpPost], #[HttpPut], #[HttpDelete] — controller method attrs
// - #[FromBody], #[FromRoute], #[FromQuery] — parameter binding
// - rust_webapp::request! declaration macro

mod claims;
mod controller;
mod endpoint;
mod handler;
mod param;
mod route;

use proc_macro::TokenStream;

/// Marks a struct as a controller with an optional base path.
///
/// ```ignore
/// #[controller("/api/users")]
/// struct UserController { mediator: Arc<dyn IMediator> }
/// ```
#[proc_macro_attribute]
pub fn controller(attr: TokenStream, item: TokenStream) -> TokenStream {
    controller::controller_impl(attr, item)
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
// Controller method attributes
// ---------------------------------------------------------------------------

/// Marks a controller method as HTTP GET with optional path.
#[proc_macro_attribute]
pub fn http_get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::http_method_impl("GET", attr, item)
}

/// Marks a controller method as HTTP POST with optional path.
#[proc_macro_attribute]
pub fn http_post(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::http_method_impl("POST", attr, item)
}

/// Marks a controller method as HTTP PUT with optional path.
#[proc_macro_attribute]
pub fn http_put(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::http_method_impl("PUT", attr, item)
}

/// Marks a controller method as HTTP DELETE with optional path.
#[proc_macro_attribute]
pub fn http_delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::http_method_impl("DELETE", attr, item)
}

// ---------------------------------------------------------------------------
// Parameter binding
// ---------------------------------------------------------------------------

/// Marks a field or parameter as deserialized from the JSON request body.
#[proc_macro_attribute]
pub fn from_body(_attr: TokenStream, item: TokenStream) -> TokenStream {
    param::from_attribute_impl("Body", item)
}

/// Marks a field or parameter as extracted from the route path.
#[proc_macro_attribute]
pub fn from_route(_attr: TokenStream, item: TokenStream) -> TokenStream {
    param::from_attribute_impl("Route", item)
}

/// Marks a field or parameter as extracted from the query string.
#[proc_macro_attribute]
pub fn from_query(_attr: TokenStream, item: TokenStream) -> TokenStream {
    param::from_attribute_impl("Query", item)
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

/// One-shot macro to generate a Request struct, its IRequest impl with
/// route metadata, and parameter binding code.
///
/// ```ignore
/// rust_webapp::request! {
///     #[Http(Get, "/users/{id}")]
///     GetUserRequest => UserModel {
///         #[FromRoute] id: String,
///     }
/// }
/// ```
#[proc_macro]
pub fn request(input: TokenStream) -> TokenStream {
    endpoint::request_macro_impl(input)
}
