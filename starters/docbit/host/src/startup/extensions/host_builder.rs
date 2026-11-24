//! `HostBuilder` extensions — ASP.NET Core `Use*` pipeline helpers.

use webx::*;

/// Docbit host pipeline helpers.
pub trait HostBuilderExt {
    /// Compression, timing, and request tracing (Production).
    fn use_production_middleware(self) -> Self;
}

impl HostBuilderExt for HostBuilder {
    fn use_production_middleware(self) -> Self {
        self.use_middleware::<CompressionMiddleware>()
            .use_middleware::<TimingMiddleware>()
            .use_middleware::<RequestTracing>()
    }
}
