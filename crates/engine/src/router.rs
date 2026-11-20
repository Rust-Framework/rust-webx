//! Route table and route targets for the 2.0 engine.
//!
//! One `matchit` radix tree per HTTP method, built once at startup. Each
//! registration compiles the route pattern into:
//!
//! * an `Arc<str>` pattern that is cloned (never copied) on every match, so a
//!   request never allocates a pattern `String`;
//! * the ordered `{param}` names as `&'static str` slices of the pattern, so a
//!   request only allocates the `String` values of the parameters it actually
//!   has (a static route allocates nothing).
//!
//! `match_route` returns the target, the pattern and the parameters, and
//! implements the same dispatch rules as the 1.5 host router:
//!
//! * `HEAD` falls back to the `GET` route (RFC 9110 §9.3.2). The host
//!   finalises the response and suppresses the body, so both protocols agree on
//!   the headers;
//! * a path that exists under another method is not a fallback: `match_route`
//!   returns `None` and the caller answers 405/OPTIONS using
//!   [`Router::path_exists`] and [`Router::allowed_methods`];
//! * a configured fallback (SPA shell / branded 404) is returned only for a
//!   genuine miss, i.e. for a path no method registered.

use std::sync::Arc;

use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::routing::{HttpMethod, IEndpoint};

use crate::context::EngineContext;

/// A compiled direct-dispatch function.
///
/// This is the signature of
/// `webx_core::route::scan::HandlerDispatch::dispatch`: the macro-generated
/// bridge binds the request from the live [`IHttpContext`], constructs the
/// handler, calls it and returns the response as an
/// [`EndpointResult`](webx_core::action::EndpointResult), so the engine does not
/// box `JsonResult`, `NoContentResult` or a `Mediator` round trip for an
/// ordinary JSON handler.
pub type DirectDispatchFn = webx_core::route::scan::HandlerDispatchFn;

/// The terminal component of a matched route.
///
/// A target writes the response into `ctx` and returns `Ok(())` — transport
/// errors (a failed body read, a rejected write) are returned as `Err` and
/// travel back through the middleware `after` hooks.
#[async_trait::async_trait]
pub trait RouteTarget: Send + Sync {
    /// Handle the request, writing status, headers and body into `ctx`.
    async fn handle(&self, ctx: &mut EngineContext) -> Result<()>;
}

/// Target for a handler reached through compile-time direct dispatch.
pub struct DirectTarget {
    /// The generated dispatch function (see [`DirectDispatchFn`]).
    pub dispatch: DirectDispatchFn,
    /// Route-owned response encoder for the declared response type.
    pub encoder: webx_core::route::scan::DirectResponseEncoderFn,
}

impl DirectTarget {
    /// Wrap a dispatch function and its route-owned response encoder.
    pub fn new(
        dispatch: DirectDispatchFn,
        encoder: webx_core::route::scan::DirectResponseEncoderFn,
    ) -> Self {
        Self { dispatch, encoder }
    }
}

#[async_trait::async_trait]
impl RouteTarget for DirectTarget {
    async fn handle(&self, ctx: &mut EngineContext) -> Result<()> {
        // The generated bridge takes the 1.5-compatible trait object, so the
        // engine context is handed over as `&mut dyn IHttpContext`; the result
        // is written through the concrete context afterwards.
        let result = (self.dispatch)(&mut *ctx, self.encoder).await?;
        ctx.write_endpoint_result(result).await
    }
}

/// Target for an endpoint that owns its own dispatch (built-in endpoints such
/// as health, metrics, OpenAPI, and host-provided endpoints).
pub struct EndpointTarget {
    /// The endpoint contract from `webx_core`.
    pub endpoint: Arc<dyn IEndpoint>,
}

impl EndpointTarget {
    /// Wrap an endpoint.
    pub fn new(endpoint: Arc<dyn IEndpoint>) -> Self {
        Self { endpoint }
    }
}

#[async_trait::async_trait]
impl RouteTarget for EndpointTarget {
    async fn handle(&self, ctx: &mut EngineContext) -> Result<()> {
        // `EngineContext` implements `IHttpContext`; the endpoint contract is
        // unchanged from 1.5.
        let ctx: &mut dyn IHttpContext = ctx;
        IEndpoint::handle(self.endpoint.as_ref(), ctx).await
    }
}

/// One successful route match.
pub struct RouteMatch {
    /// The target that writes the response.
    pub target: Arc<dyn RouteTarget>,
    /// The registered pattern (`"/api/users/{id}"`), shared with every match of
    /// this route.
    pub pattern: Arc<str>,
    /// Matched `{param}` names and values, in pattern order.
    ///
    /// Empty — and unallocated — for a static route.
    pub params: Vec<(&'static str, String)>,
}

/// Synthetic pattern reported for a fallback match (see
/// [`Router::set_fallback`]).
pub const FALLBACK_PATTERN: &str = "/{*path}";

/// Number of method trees: GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS.
const METHOD_COUNT: usize = 7;

const GET: usize = 0;
const POST: usize = 1;
const PUT: usize = 2;
const DELETE: usize = 3;
const PATCH: usize = 4;
const HEAD: usize = 5;
const OPTIONS: usize = 6;

fn method_index(method: HttpMethod) -> usize {
    match method {
        HttpMethod::Get => GET,
        HttpMethod::Post => POST,
        HttpMethod::Put => PUT,
        HttpMethod::Delete => DELETE,
        HttpMethod::Patch => PATCH,
        HttpMethod::Head => HEAD,
        HttpMethod::Options => OPTIONS,
    }
}

/// A registered route.
struct RouteValue {
    target: Arc<dyn RouteTarget>,
    pattern: Arc<str>,
    /// `{param}` names in pattern order. Slices into the `&'static str`
    /// pattern, so they are themselves `&'static str`.
    param_names: Box<[&'static str]>,
}

/// Per-method `matchit` route table.
pub struct Router {
    routes: [matchit::Router<RouteValue>; METHOD_COUNT],
    fallback: Option<RouteValue>,
}

impl Router {
    /// Create an empty router.
    pub fn new() -> Self {
        Self {
            routes: std::array::from_fn(|_| matchit::Router::new()),
            fallback: None,
        }
    }

    /// Register `target` for `method` + `path`.
    ///
    /// `path` is `&'static str` because the pattern is stored once as
    /// `Arc<str>` and the parameter names are slices of it; nothing is parsed
    /// or allocated per request.
    ///
    /// A conflicting or malformed pattern is a startup configuration error; it
    /// is logged and not registered (the 1.5 behavior), so the process still
    /// starts and the route answers 404 instead of panicking mid-boot.
    pub fn insert(&mut self, method: HttpMethod, path: &'static str, target: Arc<dyn RouteTarget>) {
        if let Err(error) = self.try_insert(method, path, target) {
            tracing::warn!("{error}");
        }
    }

    /// Register a route, surfacing a malformed/conflicting pattern as a typed
    /// startup error.
    pub fn try_insert(
        &mut self,
        method: HttpMethod,
        path: &'static str,
        target: Arc<dyn RouteTarget>,
    ) -> Result<()> {
        let value = RouteValue {
            target,
            pattern: Arc::from(path),
            param_names: param_names(path).into_boxed_slice(),
        };
        self.routes[method_index(method)]
            .insert(path, value)
            .map_err(|error| {
                webx_core::error::Error::Http(format!(
                    "failed to register route {} {}: {error}",
                    method.as_str(),
                    path
                ))
            })
    }

    /// Set the target for a genuine miss (SPA index fallback or a branded 404).
    ///
    /// It is only consulted when no method registered the path: a 405/OPTIONS
    /// answer must not be replaced by the app shell. The target is responsible
    /// for deciding whether it can serve (e.g. `Accept: text/html`) and writes
    /// the 404 itself when it cannot.
    pub fn set_fallback(&mut self, target: Arc<dyn RouteTarget>) {
        self.fallback = Some(RouteValue {
            target,
            pattern: Arc::from(FALLBACK_PATTERN),
            param_names: Vec::new().into_boxed_slice(),
        });
    }

    /// Match `method` + `path`.
    ///
    /// Returns `None` when the path is registered for other methods (the caller
    /// writes 405/OPTIONS) or when nothing matches and no fallback is
    /// configured.
    pub fn match_route(&self, method: &str, path: &str) -> Option<RouteMatch> {
        let Some(method) = HttpMethod::from_str(method) else {
            return if self.path_exists(path) {
                None
            } else {
                self.fallback_match()
            };
        };

        if let Some(found) = self.match_method(method, path) {
            return Some(found);
        }

        // HEAD is GET without a body (RFC 9110 §9.3.2). Only the route is
        // redirected; the host suppresses the body when it finalises the
        // response, so the GET representation's headers are preserved.
        if method == HttpMethod::Head {
            if let Some(found) = self.match_method(HttpMethod::Get, path) {
                return Some(found);
            }
        }

        // A method mismatch on a known path is a 405, never the SPA shell.
        if self.path_exists(path) {
            return None;
        }
        self.fallback_match()
    }

    fn match_method(&self, method: HttpMethod, path: &str) -> Option<RouteMatch> {
        let matched = self.routes[method_index(method)].at(path).ok()?;
        let value = matched.value;

        let mut params = Vec::new();
        if !matched.params.is_empty() {
            params.reserve(matched.params.len());
            for (index, (name, matched_value)) in matched.params.iter().enumerate() {
                // matchit reports parameters in pattern order; the names were
                // parsed from the same pattern at registration time, so they
                // can be handed out as `&'static str`.
                debug_assert_eq!(
                    value.param_names.get(index).copied(),
                    Some(name),
                    "matchit parameter order must follow the pattern text"
                );
                let Some(param_name) = value.param_names.get(index).copied() else {
                    continue;
                };
                params.push((param_name, matched_value.to_string()));
            }
        }

        Some(RouteMatch {
            target: Arc::clone(&value.target),
            pattern: Arc::clone(&value.pattern),
            params,
        })
    }

    fn fallback_match(&self) -> Option<RouteMatch> {
        let fallback = self.fallback.as_ref()?;
        Some(RouteMatch {
            target: Arc::clone(&fallback.target),
            pattern: Arc::clone(&fallback.pattern),
            params: Vec::new(),
        })
    }

    /// Whether any HTTP method registered `path`.
    ///
    /// Used to distinguish 405 (path exists) from 404 (it does not).
    pub fn path_exists(&self, path: &str) -> bool {
        self.routes.iter().any(|routes| routes.at(path).is_ok())
    }

    /// The methods registered for `path`, for an `Allow` header and OPTIONS.
    ///
    /// `HEAD` is included whenever `GET` is registered, because the router
    /// serves HEAD through the GET route; an explicitly registered HEAD is
    /// reported once. The list is empty when the path is unknown.
    pub fn allowed_methods(&self, path: &str) -> Vec<&'static str> {
        let has = |method| self.routes[method_index(method)].at(path).is_ok();

        let mut methods = Vec::new();
        if has(HttpMethod::Get) {
            methods.push("GET");
            methods.push("HEAD");
        }
        if has(HttpMethod::Post) {
            methods.push("POST");
        }
        if has(HttpMethod::Put) {
            methods.push("PUT");
        }
        if has(HttpMethod::Patch) {
            methods.push("PATCH");
        }
        if has(HttpMethod::Delete) {
            methods.push("DELETE");
        }
        if has(HttpMethod::Head) && !methods.contains(&"HEAD") {
            methods.push("HEAD");
        }
        if has(HttpMethod::Options) {
            methods.push("OPTIONS");
        }
        methods
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the `{param}` names from a route pattern, in order.
///
/// Matches the `matchit` syntax: `{name}`, `{name}suffix`, `prefix{name}` and
/// `{*catch_all}`. `{{` and `}}` are literals, not parameter delimiters.
fn param_names(pattern: &'static str) -> Vec<&'static str> {
    let bytes = pattern.as_bytes();
    let mut names = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'{' if bytes.get(cursor + 1) == Some(&b'{') => cursor += 2,
            b'}' if bytes.get(cursor + 1) == Some(&b'}') => cursor += 2,
            b'{' => {
                let start = cursor + 1;
                let Some(offset) = bytes[start..].iter().position(|byte| *byte == b'}') else {
                    break;
                };
                let end = start + offset;
                let name = &pattern[start..end];
                names.push(name.strip_prefix('*').unwrap_or(name));
                cursor = end + 1;
            }
            _ => cursor += 1,
        }
    }

    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use webx_core::action::EndpointResult;

    /// A target that only counts dispatches; router tests never call it.
    struct CountingTarget(AtomicUsize);

    #[async_trait::async_trait]
    impl RouteTarget for CountingTarget {
        async fn handle(&self, _ctx: &mut EngineContext) -> Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn target() -> Arc<dyn RouteTarget> {
        Arc::new(CountingTarget(AtomicUsize::new(0)))
    }

    fn get_router() -> Router {
        Router::new()
    }

    #[test]
    fn static_route_has_no_params_and_reuses_its_pattern() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/api/health", target());

        let first = router
            .match_route("GET", "/api/health")
            .expect("static route matches");
        assert!(first.params.is_empty());
        assert_eq!(
            first.params.capacity(),
            0,
            "a static route must not allocate a parameter vector"
        );
        assert_eq!(&*first.pattern, "/api/health");

        let second = router
            .match_route("GET", "/api/health")
            .expect("static route matches again");
        assert!(
            Arc::ptr_eq(&first.pattern, &second.pattern),
            "the pattern is allocated once at registration, not per request"
        );
        assert!(
            Arc::ptr_eq(&first.target, &second.target),
            "the target Arc is cloned, not reallocated"
        );
    }

    #[test]
    fn dynamic_params_are_reported_in_pattern_order() {
        let mut router = get_router();
        router.insert(
            HttpMethod::Get,
            "/api/users/{user_id}/posts/{post_id}",
            target(),
        );

        let found = router
            .match_route("GET", "/api/users/42/posts/7")
            .expect("dynamic route matches");
        assert_eq!(
            found.params,
            vec![("user_id", "42".to_string()), ("post_id", "7".to_string())]
        );
        assert_eq!(&*found.pattern, "/api/users/{user_id}/posts/{post_id}");
    }

    #[test]
    fn catch_all_parameter_keeps_its_name_and_full_value() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/assets/{*file_path}", target());

        let found = router
            .match_route("GET", "/assets/css/app.css")
            .expect("catch-all route matches");
        assert_eq!(found.params, vec![("file_path", "css/app.css".to_string())]);
    }

    #[test]
    fn escaped_braces_are_not_parameters() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/{{literal}}", target());

        let found = router
            .match_route("GET", "/{literal}")
            .expect("escaped-brace route matches");
        assert!(found.params.is_empty());
    }

    #[test]
    fn unknown_path_is_not_found() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/api/health", target());

        assert!(router.match_route("GET", "/missing").is_none());
        assert!(!router.path_exists("/missing"));
        assert!(router.allowed_methods("/missing").is_empty());
    }

    #[test]
    fn fallback_target_handles_a_genuine_miss_only() {
        let mut router = get_router();
        router.insert(HttpMethod::Post, "/api/orders", target());
        let fallback = target();
        router.set_fallback(Arc::clone(&fallback));

        let missed = router
            .match_route("GET", "/some/spa/route")
            .expect("fallback serves a genuine miss");
        assert!(missed.params.is_empty());
        assert!(Arc::ptr_eq(&missed.target, &fallback));
        assert_eq!(&*missed.pattern, FALLBACK_PATTERN);

        // A known path under another method stays a 405, not the app shell.
        assert!(
            router.match_route("GET", "/api/orders").is_none(),
            "fallback must not mask 405"
        );
        assert!(router.path_exists("/api/orders"));
    }

    #[test]
    fn method_mismatch_reports_405_data() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/items/{id}", target());

        assert!(router.match_route("DELETE", "/items/9").is_none());
        assert!(router.match_route("OPTIONS", "/items/9").is_none());
        assert!(router.path_exists("/items/9"));
        assert_eq!(router.allowed_methods("/items/9"), vec!["GET", "HEAD"]);
    }

    #[test]
    fn allowed_methods_cover_every_method_without_duplicate_head() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/all", target());
        router.insert(HttpMethod::Head, "/all", target());
        router.insert(HttpMethod::Post, "/all", target());
        router.insert(HttpMethod::Put, "/all", target());
        router.insert(HttpMethod::Patch, "/all", target());
        router.insert(HttpMethod::Delete, "/all", target());
        router.insert(HttpMethod::Options, "/all", target());

        assert_eq!(
            router.allowed_methods("/all"),
            vec!["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
        );
    }

    #[test]
    fn explicit_head_route_wins_over_get_fallback() {
        let mut router = get_router();
        let get_target = target();
        let head_target = target();
        router.insert(HttpMethod::Get, "/page", Arc::clone(&get_target));
        router.insert(HttpMethod::Head, "/page", Arc::clone(&head_target));

        let head = router
            .match_route("HEAD", "/page")
            .expect("explicit HEAD route matches");
        assert!(Arc::ptr_eq(&head.target, &head_target));
        assert_eq!(router.allowed_methods("/page"), vec!["GET", "HEAD"]);

        let get = router
            .match_route("GET", "/page")
            .expect("GET still matches");
        assert!(Arc::ptr_eq(&get.target, &get_target));
    }

    #[test]
    fn head_falls_back_to_get_route() {
        let mut router = get_router();
        let get_target = target();
        router.insert(HttpMethod::Get, "/api/items", Arc::clone(&get_target));

        let head = router
            .match_route("HEAD", "/api/items")
            .expect("HEAD falls back to the GET route");
        assert!(Arc::ptr_eq(&head.target, &get_target));
        assert_eq!(&*head.pattern, "/api/items");
        assert_eq!(router.allowed_methods("/api/items"), vec!["GET", "HEAD"]);
    }

    #[test]
    fn head_without_get_route_is_not_found() {
        let mut router = get_router();
        router.insert(HttpMethod::Post, "/api/orders", target());

        assert!(router.match_route("HEAD", "/api/orders").is_none());
    }

    #[test]
    fn non_action_method_is_not_found() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/api/health", target());

        assert!(router.match_route("TRACE", "/api/health").is_none());
        assert!(router.match_route("GET", "/api/health").is_some());
    }

    // -----------------------------------------------------------------------
    // Targets (exercised against the real `EngineContext`)
    // -----------------------------------------------------------------------

    fn test_context() -> EngineContext {
        let (parts, ()) = http::Request::new(()).into_parts();
        EngineContext::from_parts(
            parts,
            bytes::Bytes::new(),
            Arc::new(crate::body::BodyLimits::default()),
            None,
            None,
        )
    }

    fn ignore_encoder(_value: &dyn std::any::Any) -> Result<EndpointResult> {
        Err(webx_core::error::Error::Internal(
            "test encoder must not be called".to_string(),
        ))
    }

    fn dispatch_json<'a>(
        _ctx: &'a mut dyn IHttpContext,
        _encoder: webx_core::route::scan::DirectResponseEncoderFn,
    ) -> Pin<Box<dyn Future<Output = Result<EndpointResult>> + Send + 'a>> {
        Box::pin(async move {
            Ok(EndpointResult::Json {
                status: 201,
                body: bytes::Bytes::from_static(b"{\"ok\":true}"),
            })
        })
    }

    #[tokio::test]
    async fn direct_target_writes_the_endpoint_result() {
        let target = DirectTarget::new(dispatch_json, ignore_encoder);
        let mut ctx = test_context();

        target
            .handle(&mut ctx)
            .await
            .expect("direct dispatch writes the response");

        assert_eq!(ctx.response().status(), 201);
        assert_eq!(
            ctx.response().header("content-type"),
            Some("application/json")
        );
        assert_eq!(ctx.response().body_bytes(), b"{\"ok\":true}".to_vec());
    }

    struct FixedEndpoint;

    #[async_trait::async_trait]
    impl IEndpoint for FixedEndpoint {
        async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
            ctx.response_mut().set_status(202);
            ctx.response_mut().write_bytes(b"endpoint".to_vec()).await
        }
    }

    #[tokio::test]
    async fn endpoint_target_calls_the_endpoint_with_the_context() {
        let target = EndpointTarget::new(Arc::new(FixedEndpoint));
        let mut ctx = test_context();

        target
            .handle(&mut ctx)
            .await
            .expect("endpoint writes the response");

        assert_eq!(ctx.response().status(), 202);
        assert_eq!(ctx.response().body_bytes(), b"endpoint".to_vec());
    }

    #[tokio::test]
    async fn matched_route_feeds_the_engine_context() {
        let mut router = get_router();
        router.insert(HttpMethod::Get, "/api/users/{id}", target());

        let matched = router
            .match_route("GET", "/api/users/42")
            .expect("dynamic route matches");
        let mut ctx = test_context();
        ctx.set_route(matched.pattern, matched.params);

        assert_eq!(ctx.request().route_pattern(), Some("/api/users/{id}"));
        assert_eq!(
            ctx.request().route_params().get("id").map(String::as_str),
            Some("42")
        );
    }
}
