//! Matchit-based router with zero-alloc route matching.
//!
//! Uses `matchit::Router` (the same library as Axum) for efficient
//! radix-tree based route matching with zero heap allocations per request.

use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::routing::{HttpMethod, IEndpoint, IRouter};
use std::collections::HashMap;
use std::sync::Arc;

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
        let _ = self.inner.insert(path, value);
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
        let method = HttpMethod::from_str(method_str).unwrap_or(HttpMethod::Get);

        let router = self.method_router(method);

        if let Some(matched) = router.at(path) {
            let mut params = HashMap::new();
            for (key, value) in matched.params.iter() {
                params.insert(key.to_string(), value.to_string());
            }

            let (endpoint, pattern) = matched.value;
            return Ok(Some((Arc::clone(endpoint), params, pattern.clone())));
        }

        Ok(None)
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}
