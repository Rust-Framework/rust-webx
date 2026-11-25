//! `HostBuilder` extensions — ASP.NET Core `Use*` pipeline helpers.

use webx::*;

/// Dmbit host pipeline helpers.
pub trait HostBuilderExt {
    /// The framework middleware Production needs: compression, timing, tracing.
    ///
    /// Every layer here is a framework middleware registration; the application
    /// does not hand-roll tower-style layers of its own.
    fn use_production_middleware(self) -> Self;
}

impl HostBuilderExt for HostBuilder {
    fn use_production_middleware(self) -> Self {
        self.use_middleware(MiddlewareSpec::new::<CompressionMiddleware>())
            .use_middleware(MiddlewareSpec::new::<TimingMiddleware>())
            .use_middleware(MiddlewareSpec::new::<RequestTracing>())
    }
}
