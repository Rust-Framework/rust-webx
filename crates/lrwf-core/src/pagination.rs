//! Pagination types for list endpoints.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct PagedRequest {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
}

fn default_page() -> u32 {
    1
}
fn default_page_size() -> u32 {
    20
}

impl Default for PagedRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 20,
        }
    }
}

impl PagedRequest {
    pub fn new(page: u32, page_size: u32) -> Self {
        Self { page, page_size }
    }

    pub fn offset(&self) -> u32 {
        (self.page.saturating_sub(1)).saturating_mul(self.page_size)
    }

    pub fn clamped(mut self) -> Self {
        self.page = self.page.max(1);
        self.page_size = self.page_size.clamp(1, 100);
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PagedResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
}

impl<T: Serialize> PagedResponse<T> {
    pub fn new(items: Vec<T>, total: u64, page: u32, page_size: u32) -> Self {
        let total_pages = if page_size > 0 {
            ((total as f64) / (page_size as f64)).ceil() as u32
        } else {
            0
        };
        Self {
            items,
            total,
            page,
            page_size,
            total_pages,
        }
    }

    pub fn from_request(items: Vec<T>, total: u64, req: &PagedRequest) -> Self {
        Self::new(items, total, req.page, req.page_size)
    }
}
