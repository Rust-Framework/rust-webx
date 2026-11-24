//! Pagination contract types.
//!
//! Since 3.0 these are the shared `rust-core` contracts; this module only
//! re-exports them so the historical `webx_core::pagination::*` paths keep
//! working. Do not define a second copy here.
//!
//! * [`PagedRequest`] carries the plain request fields; defaults and range
//!   validation belong to the binding layer (no `from_parts` normalization).
//! * [`PagedResult`] is the flat operation envelope
//!   `{success,status,message,items,total}`; the page part has no `page` /
//!   `page_size` / `total_pages` echo.

pub use rust_core::pagination::{
    PagedRequest, PagedResult, DEFAULT_PAGE, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_defaults_and_limits_are_the_shared_constants() {
        assert_eq!(DEFAULT_PAGE, 1);
        assert_eq!(DEFAULT_PAGE_SIZE, 20);
        assert_eq!(MAX_PAGE_SIZE, 100);
    }

    #[test]
    fn offset_is_u64_and_saturates_a_zero_page() {
        assert_eq!(
            PagedRequest {
                page: 3,
                page_size: 20
            }
            .offset(),
            40
        );
        assert_eq!(
            PagedRequest {
                page: 0,
                page_size: 20
            }
            .offset(),
            0
        );
        assert_eq!(
            PagedRequest {
                page: u32::MAX,
                page_size: u32::MAX
            }
            .offset(),
            u64::from(u32::MAX - 1) * u64::from(u32::MAX)
        );
    }

    #[test]
    fn page_result_is_a_flat_operation_envelope_without_request_echo() {
        let page = PagedResult::new(vec!["a", "b"], 7);
        let value = serde_json::to_value(&page).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "success": true,
                "status": 0,
                "message": "ok",
                "items": ["a", "b"],
                "total": 7
            })
        );
        let object = value.as_object().expect("object");
        assert_eq!(object.len(), 5);
        assert!(!object.contains_key("page"));
        assert!(!object.contains_key("page_size"));
        assert!(!object.contains_key("total_pages"));
    }

    #[test]
    fn negative_query_values_are_rejected_by_deserialization() {
        // `u32` is the contract: a negative page never reaches a handler.
        assert!(serde_json::from_str::<PagedRequest>(r#"{ "page": -1 }"#).is_err());
        assert!(serde_json::from_str::<PagedRequest>(r#"{ "page_size": -20 }"#).is_err());
    }
}
