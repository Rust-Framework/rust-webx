//! Trie-based router with dual-mode endpoint dispatch.
//!
//! Supports two endpoint types:
//! 1. IRequest-based endpoints (registered via #[endpoint] macro)
//! 2. Controller-based endpoints (registered via #[controller] macro)
//!
//! Path segments can be:
//! - Static: literal strings like "users"
//! - Dynamic: parameterized like "{id}" — extracted into route_params

use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::routing::{HttpMethod, IEndpoint, IRouter};
use std::collections::HashMap;
use std::sync::Arc;

/// A route entry in the trie.
#[derive(Clone)]
struct RouteNode {
    /// Static children: segment → child node
    static_children: HashMap<String, RouteNode>,
    /// Dynamic parameter child (e.g., "{id}")
    param_child: Option<Box<RouteNode>>,
    /// The parameter name for dynamic segments
    param_name: Option<String>,
    /// Endpoints registered at this node, keyed by HTTP method.
    /// Value is (endpoint, original_route_pattern).
    handlers: HashMap<HttpMethod, (Arc<dyn IEndpoint>, String)>,
}

impl RouteNode {
    fn new() -> Self {
        Self {
            static_children: HashMap::new(),
            param_child: None,
            param_name: None,
            handlers: HashMap::new(),
        }
    }
}

/// Trie-based router implementation.
pub struct Router {
    root: RouteNode,
}

impl Router {
    pub fn new() -> Self {
        Self {
            root: RouteNode::new(),
        }
    }
}

#[async_trait::async_trait]
impl IRouter for Router {
    fn register(&mut self, method: HttpMethod, path: &str, endpoint: Arc<dyn IEndpoint>) {
        let segments = parse_path_segments(path);
        let mut node = &mut self.root;

        for segment in segments {
            if segment.starts_with('{') && segment.ends_with('}') {
                // Dynamic parameter segment
                let param_name = segment[1..segment.len() - 1].to_string();
                if node.param_child.is_none() {
                    node.param_child = Some(Box::new(RouteNode::new()));
                }
                node.param_name = Some(param_name);
                node = node.param_child.as_mut().unwrap();
            } else {
                // Static segment
                node = node
                    .static_children
                    .entry(segment.to_string())
                    .or_insert_with(RouteNode::new);
            }
        }

        node.handlers
            .insert(method, (endpoint, path.to_string()));
    }

    async fn match_route(
        &self,
        ctx: &mut dyn IHttpContext,
    ) -> Result<Option<(Arc<dyn IEndpoint>, HashMap<String, String>, String)>> {
        let path = ctx.request().path().to_string();
        let method_str = ctx.request().method().to_string();
        let method = HttpMethod::from_str(&method_str).unwrap_or(HttpMethod::Get);

        let segments = parse_path_segments(&path);
        let mut params = HashMap::new();

        if let Some(endpoint) = self.match_node(&self.root, &segments, 0, &mut params) {
            if let Some((handler, pattern)) = endpoint.handlers.get(&method) {
                return Ok(Some((Arc::clone(handler), params, pattern.clone())));
            }
        }

        Ok(None)
    }
}

impl Router {
    /// Recursively match path segments against the trie.
    fn match_node<'a>(
        &self,
        node: &'a RouteNode,
        segments: &[String],
        index: usize,
        params: &mut HashMap<String, String>,
    ) -> Option<&'a RouteNode> {
        if index >= segments.len() {
            // Reached the end of the path
            if !node.handlers.is_empty() {
                return Some(node);
            }
            return None;
        }

        let segment = &segments[index];

        // Try static match first
        if let Some(child) = node.static_children.get(segment) {
            if let Some(result) = self.match_node(child, segments, index + 1, params) {
                return Some(result);
            }
        }

        // Try dynamic parameter match
        if let Some(ref param_child) = node.param_child {
            if let Some(ref param_name) = node.param_name {
                params.insert(param_name.clone(), segment.clone());
            }
            if let Some(result) = self.match_node(param_child, segments, index + 1, params) {
                return Some(result);
            }
            // Backtrack: remove the param if it didn't lead to a match
            if let Some(ref param_name) = node.param_name {
                params.remove(param_name);
            }
        }

        None
    }
}

/// Parse a path like "/users/{id}/posts" into segments ["users", "{id}", "posts"].
fn parse_path_segments(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}
