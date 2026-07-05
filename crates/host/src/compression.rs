//! Response compression using gzip/deflate (via flate2).
//!
//! Compression is applied at the hyper response layer.
//! This module provides the `compress_body` helper function
//! and a middleware that sets the Vary header.

use flate2::write::GzEncoder;
use flate2::Compression;
use rust_webapp_core::error::Result;
use rust_webapp_core::http::IHttpContext;
use rust_webapp_core::middleware::IMiddleware;
use std::io::Write;
use std::ops::ControlFlow;

/// Compress a byte buffer using gzip at the given compression level (0-9).
pub fn compress_gzip(data: &[u8], level: u32) -> Option<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 2), Compression::new(level));
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

/// Compression configuration.
pub struct CompressionConfig {
    pub level: u32,
    pub min_size: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            level: 6,
            min_size: 1024,
        }
    }
}

impl CompressionConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn level(mut self, l: u32) -> Self {
        self.level = l;
        self
    }
    pub fn min_size(mut self, s: usize) -> Self {
        self.min_size = s;
        self
    }
}

/// Middleware that compresses response bodies with gzip when the client
/// supports it (via `Accept-Encoding: gzip`) and the response exceeds
/// `min_size` bytes.
///
/// Skips already-compressed content types (images, video, audio).
///
/// ```ignore
/// Host::builder()
///     .use_middleware::<CompressionMiddleware>()
///     .build()
/// ```
pub struct CompressionMiddleware {
    config: CompressionConfig,
}

impl Default for CompressionMiddleware {
    fn default() -> Self {
        Self {
            config: CompressionConfig::default(),
        }
    }
}

impl CompressionMiddleware {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_config(config: CompressionConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl IMiddleware for CompressionMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        ctx.response_mut().set_header("vary", "accept-encoding");
        Ok(ControlFlow::Continue(()))
    }

    async fn after(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let accept = ctx.request().header("accept-encoding").unwrap_or("");
        if !accept.to_lowercase().contains("gzip") {
            return Ok(());
        }

        let body = ctx.response().body_bytes();
        if body.len() < self.config.min_size {
            return Ok(());
        }

        let ct = ctx
            .response()
            .header("content-type")
            .unwrap_or("")
            .to_lowercase();
        if ct.starts_with("image/") || ct.starts_with("video/") || ct.starts_with("audio/") {
            return Ok(());
        }

        if let Some(compressed) = compress_gzip(&body, self.config.level) {
            ctx.response_mut().set_header("content-encoding", "gzip");
            ctx.response_mut().write_bytes(compressed).await?;
        }
        Ok(())
    }
}
