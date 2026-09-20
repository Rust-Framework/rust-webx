//! Matchit-based router with zero-alloc route matching.
//!
//! Uses `matchit::Router` (the same library as Axum) for efficient
//! radix-tree based route matching with zero heap allocations per request.

use std::collections::HashMap;
use std::sync::Arc;
use webx_core::error::Result;
use webx_core::http::IHttpContext;
use webx_core::routing::{HttpMethod, IEndpoint, IRouter};

type RouteValue = (Arc<dyn IEndpoint>, String);

/// Per-method matchit router.
struct MethodRouter {
    inner: matchit::Router<RouteValue>,
}

impl MethodRouter {
    fn new() -> Self {
        Self {
            inner: matchit::Router::new(),
        }
    }

    fn insert(&mut self, path: &str, endpoint: Arc<dyn IEndpoint>) {
        let value = (endpoint, path.to_string());
        if let Err(e) = self.inner.insert(path, value) {
            tracing::warn!("[Router] Failed to register route '{}': {}", path, e);
        }
    }

    fn at<'a>(&'a self, path: &'a str) -> Option<matchit::Match<'a, 'a, &'a RouteValue>> {
        self.inner.at(path).ok()
    }
}

/// Matchit-based router.
pub struct Router {
    get: MethodRouter,
    post: MethodRouter,
    put: MethodRouter,
    delete: MethodRouter,
    patch: MethodRouter,
    head: MethodRouter,
    options: MethodRouter,
}

impl Router {
    pub fn new() -> Self {
        Self {
            get: MethodRouter::new(),
            post: MethodRouter::new(),
            put: MethodRouter::new(),
            delete: MethodRouter::new(),
            patch: MethodRouter::new(),
            head: MethodRouter::new(),
            options: MethodRouter::new(),
        }
    }

    fn method_router_mut(&mut self, m: HttpMethod) -> &mut MethodRouter {
        match m {
            HttpMethod::Get => &mut self.get,
            HttpMethod::Post => &mut self.post,
            HttpMethod::Put => &mut self.put,
            HttpMethod::Delete => &mut self.delete,
            HttpMethod::Patch => &mut self.patch,
            HttpMethod::Head => &mut self.head,
            HttpMethod::Options => &mut self.options,
        }
    }

    fn method_router(&self, m: HttpMethod) -> &MethodRouter {
        match m {
            HttpMethod::Get => &self.get,
            HttpMethod::Post => &self.post,
            HttpMethod::Put => &self.put,
            HttpMethod::Delete => &self.delete,
            HttpMethod::Patch => &self.patch,
            HttpMethod::Head => &self.head,
            HttpMethod::Options => &self.options,
        }
    }

    /// Check if a path is registered for any HTTP method.
    pub fn path_exists(&self, path: &str) -> bool {
        self.get.at(path).is_some()
            || self.post.at(path).is_some()
            || self.put.at(path).is_some()
            || self.delete.at(path).is_some()
            || self.patch.at(path).is_some()
            || self.head.at(path).is_some()
            || self.options.at(path).is_some()
    }
}

#[async_trait::async_trait]
impl IRouter for Router {
    fn register(&mut self, method: HttpMethod, path: &str, endpoint: Arc<dyn IEndpoint>) {
        self.method_router_mut(method).insert(path, endpoint);
    }

    async fn match_route(
        &self,
        ctx: &mut dyn IHttpContext,
    ) -> Result<Option<(Arc<dyn IEndpoint>, HashMap<String, String>, String)>> {
        let path = ctx.request().path();
        let method_str = ctx.request().method();
        let method = match HttpMethod::from_str(method_str) {
            Some(m) => m,
            None => return Ok(None),
        };

        if let Some(matched) = self.method_router(method).at(path) {
            return Ok(Some(collect_match(matched)));
        }

        // HEAD falls back to the GET route for the same path (RFC 9110 §9.3.2:
        // HEAD is identical to GET without a body). The host suppresses the
        // body when it writes the response, so the headers — including
        // `Content-Length`, `ETag` and `Accept-Ranges` — are exactly those of
        // the equivalent GET.
        if method == HttpMethod::Head {
            if let Some(matched) = self.get.at(path) {
                return Ok(Some(collect_match(matched)));
            }
        }

        Ok(None)
    }
}

fn collect_match(
    matched: matchit::Match<'_, '_, &RouteValue>,
) -> (Arc<dyn IEndpoint>, HashMap<String, String>, String) {
    let mut params = HashMap::new();
    for (key, value) in matched.params.iter() {
        params.insert(key.to_string(), value.to_string());
    }
    let (endpoint, pattern) = matched.value;
    (Arc::clone(endpoint), params, pattern.clone())
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}
