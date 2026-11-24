//! `HostBuilder` extensions — ASP.NET Core `Use*` pipeline helpers.

use webx::*;

/// Dmbit host pipeline helpers.
pub trait HostBuilderExt {
    /// Compression, timing, and request tracing (Production).
    fn use_production_middleware(self) -> Self;
}

impl HostBuilderExt for HostBuilder {
    fn use_production_middleware(self) -> Self {
        self.use_middleware(MiddlewareSpec::new::<CompressionMiddleware>())
            .use_middleware(MiddlewareSpec::new::<TimingMiddleware>())
            .use_middleware(MiddlewareSpec::new::<RequestTracing>())
    }
}
