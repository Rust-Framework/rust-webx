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

    /// Build a request from the two optional query fields of a list endpoint.
    ///
    /// `None` falls back to the framework defaults (page `1`, page size `20`)
    /// and the result is [`clamped`](Self::clamped), so the binding layer may
    /// hand over `Option<u32>` without every handler repeating the defaults:
    ///
    /// ```
    /// # use webx_core::pagination::PagedRequest;
    /// let defaults = PagedRequest::from_parts(None, None);
    /// assert_eq!((defaults.page, defaults.page_size), (1, 20));
    ///
    /// // Out-of-range values are clamped instead of rejected.
    /// let zero = PagedRequest::from_parts(Some(0), Some(0));
    /// assert_eq!((zero.page, zero.page_size), (1, 1));
    /// let wide = PagedRequest::from_parts(Some(3), Some(1_000));
    /// assert_eq!((wide.page, wide.page_size), (3, 100));
    /// ```
    ///
    /// Equivalent to `PagedRequest::new(page.unwrap_or(1), page_size.unwrap_or(20)).clamped()`.
    pub fn from_parts(page: Option<u32>, page_size: Option<u32>) -> Self {
        Self::new(page.unwrap_or(1), page_size.unwrap_or(20)).clamped()
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
pub struct PagedResult<T: Serialize> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
}

impl<T: Serialize> PagedResult<T> {
    pub fn new(items: Vec<T>, total: u64, page: u32, page_size: u32) -> Self {
        let total_pages = if page_size > 0 {
            total.div_ceil(page_size as u64) as u32
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(page: Option<u32>, page_size: Option<u32>) -> (u32, u32) {
        let request = PagedRequest::from_parts(page, page_size);
        (request.page, request.page_size)
    }

    #[test]
    fn from_parts_defaults_when_both_are_absent() {
        assert_eq!(parts(None, None), (1, 20));
    }

    #[test]
    fn from_parts_fills_only_the_missing_half() {
        assert_eq!(parts(Some(4), None), (4, 20));
        assert_eq!(parts(None, Some(50)), (1, 50));
    }

    #[test]
    fn from_parts_clamps_zero_and_over_the_limit() {
        // Zero is not addressable: page 0 would underflow an offset and page
        // size 0 would divide by zero in `total_pages`.
        assert_eq!(parts(Some(0), Some(0)), (1, 1));
        assert_eq!(parts(Some(0), Some(1_000)), (1, 100));
        assert_eq!(parts(Some(7), Some(1_000)), (7, 100));
        assert_eq!(parts(Some(u32::MAX), Some(u32::MAX)), (u32::MAX, 100));
    }

    #[test]
    fn from_parts_keeps_in_range_values_untouched() {
        assert_eq!(parts(Some(1), Some(1)), (1, 1));
        assert_eq!(parts(Some(3), Some(100)), (3, 100));
    }

    #[test]
    fn from_parts_is_new_plus_clamped() {
        for (page, page_size) in [
            (None, None),
            (Some(0), None),
            (None, Some(0)),
            (Some(0), Some(101)),
            (Some(2), Some(25)),
            (Some(u32::MAX), Some(u32::MAX)),
        ] {
            let from_parts = PagedRequest::from_parts(page, page_size);
            let expected = PagedRequest::new(page.unwrap_or(1), page_size.unwrap_or(20)).clamped();
            assert_eq!(
                (from_parts.page, from_parts.page_size),
                (expected.page, expected.page_size),
                "from_parts must equal new(..).clamped() for {page:?}/{page_size:?}"
            );
        }
    }

    #[test]
    fn from_parts_clamps_before_offset_is_computed() {
        // The clamped page size, not the raw one, feeds `offset`.
        assert_eq!(PagedRequest::from_parts(Some(3), None).offset(), 40);
        assert_eq!(
            PagedRequest::from_parts(Some(3), Some(0)).offset(),
            2,
            "a zero page size is clamped to 1, so it cannot zero the offset"
        );
    }

    #[test]
    fn negative_query_values_are_rejected_by_deserialization() {
        // `u32` is the contract: a negative page never reaches `from_parts`,
        // the binding layer answers 400 before the handler runs.
        assert!(serde_json::from_str::<PagedRequest>(r#"{ "page": -1 }"#).is_err());
        assert!(serde_json::from_str::<PagedRequest>(r#"{ "page_size": -20 }"#).is_err());
    }
}
