//! Host builder and hyper server integration.
//!
//! Includes built-in exception middleware: errors produced by endpoints
//! are caught and converted to well-formed HTTP error responses using
//! `Error::status_code()`.

use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use lrdi::{ServiceCollection, ServiceProvider};
use lrwf_core::app::IHost;
use lrwf_core::config::{self, AppOptions};
use lrwf_core::error::Result;
use lrwf_core::http::IHttpContext;
use lrwf_core::middleware::IMiddleware;
use lrwf_core::mode::AppMode;
use lrwf_core::routing::{HttpMethod, IRouter};

use crate::context::HttpContext;
use crate::cors::{CorsConfig, CorsMiddleware};
use crate::endpoint::{StaticHtmlEndpoint, StaticJsonEndpoint, StubEndpoint};
use crate::pipeline::{HandlerFn, MiddlewarePipeline};
use crate::router::Router;
use lrwf_core::di::scan::{HandlerRegistration, RouteEntry};
use lrwf_openapi::{generate_openapi_spec, APIUI_HTML};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

pub struct Host {
    provider: Arc<ServiceProvider>,
    pub options: AppOptions,
    pipeline: Arc<MiddlewarePipeline>,
    router: Arc<Mutex<Router>>,
    mode: AppMode,
    spa_root: Option<String>,
}

pub struct HostBuilder {
    service_configs: Vec<Box<dyn FnOnce(ServiceCollection) -> ServiceCollection + Send>>,
    mode: AppMode,
    spa_root: Option<String>,
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
    cors_config: Option<CorsConfig>,
}

pub struct HostAppBuilder {
    options_modifiers: Vec<Box<dyn FnOnce(&mut AppOptions) + Send>>,
}

impl HostAppBuilder {
    fn new() -> Self {
        Self { options_modifiers: Vec::new() }
    }

    pub fn useOptions<F>(&mut self, f: F)
    where F: FnOnce(&mut AppOptions) + Send + 'static,
    {
        self.options_modifiers.push(Box::new(f));
    }
}

impl HostBuilder {
    pub fn new() -> Self {
        Self {
            service_configs: Vec::new(),
            mode: AppMode::default(),
            spa_root: None,
            options_modifiers: Vec::new(),
            cors_config: None,
        }
    }

    pub fn register<F>(mut self, f: F) -> Self
    where F: FnOnce(ServiceCollection) -> ServiceCollection + Send + 'static,
    {
        self.service_configs.push(Box::new(f));
        self
    }

    pub fn configure<F>(mut self, f: F) -> Self
    where F: FnOnce(&mut HostAppBuilder) + Send + 'static,
    {
        let mut builder = HostAppBuilder::new();
        f(&mut builder);
        self.options_modifiers.append(&mut builder.options_modifiers);
        self
    }

    pub fn mode(mut self, mode: AppMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn use_spa(mut self, root: impl Into<String>) -> Self {
        self.spa_root = Some(root.into());
        self
    }

    pub fn use_cors(mut self, config: CorsConfig) -> Self {
        self.cors_config = Some(config);
        self
    }

    pub fn build(self) -> Host {
        let mut svc = ServiceCollection::new();
        for cfg in self.service_configs {
            svc = cfg(svc);
        }

        for reg in inventory::iter::<HandlerRegistration> {
            svc = (reg.register)(svc);
        }

        let provider = Arc::new(svc.build().expect("Failed to build ServiceProvider"));

        let mut pipeline = MiddlewarePipeline::new();
        let middlewares: Vec<Arc<dyn IMiddleware>> = provider.get_all::<dyn IMiddleware>();
        for mw in middlewares {
            pipeline.add_middleware(mw);
        }

        let appsettings = config::load_appsettings(self.mode)
            .unwrap_or_else(|| serde_json::json!({}));
        let mut options: AppOptions = config::bind_root(&appsettings);
        for modifier in self.options_modifiers {
            modifier(&mut options);
        }

        let cors = self.cors_config.unwrap_or_else(|| {
            let cs = &options.cors;
            CorsConfig {
                origins: cs.origins.clone(),
                methods: cs.methods.clone(),
                headers: cs.headers.clone(),
                allow_credentials: cs.allow_credentials,
                max_age: cs.max_age,
            }
        });
        pipeline.add_middleware(Arc::new(CorsMiddleware::new(cors)));
        let pipeline = Arc::new(pipeline);

        let mut router = Router::new();
        let mut route_count = 0usize;
        for entry in inventory::iter::<RouteEntry> {
            route_count += 1;
            let stub = Arc::new(StubEndpoint {
                method: entry.method.as_str(),
                path: entry.path,
                handler_type: entry.handler_type,
            });
            router.register(entry.method, entry.path, stub);
        }

        let openapi_spec = generate_openapi_spec("LRWF API", "1.0.0");
        let openapi_bytes = serde_json::to_vec(&openapi_spec).unwrap_or_default();
        router.register(HttpMethod::Get, "/api/openapi.json",
            Arc::new(StaticJsonEndpoint { body: openapi_bytes }));
        router.register(HttpMethod::Get, "/api/docs",
            Arc::new(StaticHtmlEndpoint { body: APIUI_HTML }));

        if self.mode == AppMode::Development {
            let version = env!("CARGO_PKG_VERSION");
            println!("╔══════════════════════════════════════════════════════════════╗");
            println!("║                    LRWF v{:<5}                             ║", version);
            println!("║  Rust WebApi Framework — ASP.NET Core inspired               ║");
            println!("╠══════════════════════════════════════════════════════════════╣");
            println!("║  App:      {:<50}║", options.app.name);
            println!("║  OpenAPI:  /api/openapi.json                                  ║");
            println!("║  API Docs: /api/docs                                         ║");
            println!("║  CORS:     enabled                                           ║");
            if let Some(ref root) = self.spa_root {
                println!("║  SPA Root: {:<50}║", root);
            }
            println!("╚══════════════════════════════════════════════════════════════╝");
            if route_count > 0 {
                println!();
                println!("[LRWF] {} route(s) registered", route_count);
            }
            println!();
        } else if route_count > 0 {
            println!("[LRWF] {} route(s) registered", route_count);
        }

        let router = Arc::new(Mutex::new(router));

        Host { provider, options, pipeline, router, mode: self.mode, spa_root: self.spa_root }
    }
}

impl Host {
    pub fn builder() -> HostBuilder { HostBuilder::new() }

    pub fn options(&self) -> &AppOptions { &self.options }

    /// Start the server — address read from AppOptions.app.address.
    pub async fn run(&self) -> Result<()> {
        self.run_at(&self.options.app.address).await
    }

    /// Start the server at an explicit address.
    pub async fn run_at(&self, addr: &str) -> Result<()> {
        self.run_inner(addr).await
    }

    async fn run_inner(&self, addr: &str) -> Result<()> {
        let socket_addr: SocketAddr = addr
            .parse()
            .map_err(|e| lrwf_core::error::Error::Http(format!("Invalid address: {}", e)))?;

        let listener = tokio::net::TcpListener::bind(socket_addr)
            .await
            .map_err(|e| lrwf_core::error::Error::Http(format!("Failed to bind: {}", e)))?;

        if self.mode == AppMode::Development {
            tracing::info!("LRWF server listening on http://{}", addr);
            println!("Running in DEVELOPMENT mode");
            println!("LRWF server listening on http://{}", addr);
        } else {
            let version = env!("CARGO_PKG_VERSION");
            println!("LRWF v{} listening on http://{}", version, addr);
        }

        let pipeline = Arc::clone(&self.pipeline);
        let router = Arc::clone(&self.router);
        let mode = self.mode;

        loop {
            let (stream, _) = listener.accept().await.map_err(|e| {
                lrwf_core::error::Error::Http(format!("Accept error: {}", e))
            })?;

            let io = TokioIo::new(stream);
            let pipeline = Arc::clone(&pipeline);
            let router = Arc::clone(&router);

            tokio::task::spawn(async move {
                let svc_fn = service_fn(move |req: Request<Incoming>| {
                    let pipeline = Arc::clone(&pipeline);
                    let router = Arc::clone(&router);
                    let mode = mode;
                    async move {
                        let start = Instant::now();
                        let method = req.method().to_string();
                        let path = req.uri().path().to_string();
                        let result = handle_request(req, pipeline, router).await;
                        let elapsed = start.elapsed();
                        if mode == AppMode::Development {
                            let status = result.as_ref().map(|r| r.status().as_u16()).unwrap_or(500);
                            println!("[{}] {} → {} ({:.0}ms)", method, path, status,
                                elapsed.as_secs_f64() * 1000.0);
                        }
                        result
                    }
                });

                if let Err(err) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc_fn).await
                {
                    tracing::error!("Connection error: {}", err);
                }
            });
        }
    }
}

#[async_trait::async_trait]
impl IHost for Host {
    async fn run(&self, addr: &str) -> Result<()> { self.run_inner(addr).await }
    async fn stop(&self) -> Result<()> { Ok(()) }
}

fn make_router_handler(router: Arc<Mutex<Router>>) -> HandlerFn {
    Arc::new(move |ctx: &mut dyn IHttpContext| {
        let router = Arc::clone(&router);
        Box::pin(async move {
            let router = router.lock().await;
            match router.match_route(ctx).await? {
                Some((endpoint, params, pattern)) => {
                    drop(router);
                    for (key, value) in params {
                        ctx.request_mut().route_params_mut().insert(key, value);
                    }
                    *ctx.request_mut().route_pattern_mut() = Some(pattern);
                    endpoint.handle(ctx).await
                }
                None => {
                    drop(router);
                    write_error_response(ctx, 404, "Not Found").await;
                    Ok(())
                }
            }
        })
    })
}

async fn handle_request(
    req: Request<Incoming>,
    pipeline: Arc<MiddlewarePipeline>,
    router: Arc<Mutex<Router>>,
) -> std::result::Result<hyper::Response<Full<Bytes>>, std::convert::Infallible> {
    let mut ctx = HttpContext::new(req).await;
    let router_handler = make_router_handler(router);
    let result = pipeline.execute(&mut ctx, router_handler).await;

    if let Err(e) = result {
        let status = e.status_code();
        write_error_response(&mut ctx, status, &e.to_string()).await;
    }

    Ok(ctx.into_response())
}

async fn write_error_response(ctx: &mut dyn IHttpContext, status: u16, message: &str) {
    ctx.response_mut().set_status(status);
    ctx.response_mut().set_header("content-type", "application/json");
    let body = serde_json::json!({ "error": message, "status": status });
    let _ = ctx.response_mut()
        .write_bytes(serde_json::to_vec(&body).unwrap_or_default())
        .await;
}
