//! Response compression using gzip/deflate (via flate2).
//!
//! Compression is applied at the hyper response layer.
//! This module provides the `compress_body` helper function
//! and a middleware that sets the Vary header.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

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
