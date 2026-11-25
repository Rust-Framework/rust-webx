//! Pagination contract types.
//!
//! Since 2.9.1 these are the shared `rust-core` contracts; this module only
//! re-exports them so the historical `webx_core::pagination::*` paths keep
//! working. Do not define a second copy here.
//!
//! * [`PagedRequest`] carries the plain request fields; defaults and range
//!   validation belong to the binding layer (no `from_parts` normalization).
//! * [`PagedResult`] is the flat page envelope
//!   `{success,status,message,items,total}`; the page part has no `page` /
//!   `page_size` / `total_pages` echo.

pub use rust_core::pagination::{
    PagedRequest, PagedResult, DEFAULT_PAGE, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};

use std::future::Future;

use crate::route::binding::{Binder, RequestModel};
use crate::route::scan::ParamMeta;

/// Bind a nested `#[fromquery(nested)] paging: PagedRequest` field.
///
/// Defaults and range checks live here, once: absent `page`/`page_size` use
/// [`DEFAULT_PAGE`] / [`DEFAULT_PAGE_SIZE`]; `page < 1` and `page_size` outside
/// `1..=MAX_PAGE_SIZE` are `400` field errors (never a silent clamp). Its
/// [`ParamMeta`] slice is spliced into the outer request's OpenAPI parameters
/// lazily by the derive (A2), so `page`/`page_size` stay documented.
impl RequestModel for PagedRequest {
    fn param_meta() -> &'static [ParamMeta] {
        const PARAMS: &[ParamMeta] = &[
            ParamMeta {
                name: "page",
                source: "query",
                type_hint: "integer",
                required: false,
                constraints: &[],
                json_key: None,
            },
            ParamMeta {
                name: "page_size",
                source: "query",
                type_hint: "integer",
                required: false,
                constraints: &[],
                json_key: None,
            },
        ];
        PARAMS
    }

    #[allow(clippy::manual_async_fn)]
    fn bind_inline(
        ctx: &mut dyn crate::http::IHttpContext,
    ) -> impl Future<Output = crate::error::Result<Self>> + Send + '_ {
        async move {
            let mut binder = Binder::new();
            let query = ctx.request().query();

            let page = match query.get("page") {
                None => DEFAULT_PAGE,
                Some(raw) => binder
                    .required::<u32>(Some(raw), "page", "integer")
                    .unwrap_or(DEFAULT_PAGE),
            };
            if page < 1 {
                binder.validation("page", "The `page` field must be at least 1.");
            }

            let page_size = match query.get("page_size") {
                None => DEFAULT_PAGE_SIZE,
                Some(raw) => binder
                    .required::<u32>(Some(raw), "page_size", "integer")
                    .unwrap_or(DEFAULT_PAGE_SIZE),
            };
            if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
                binder.validation(
                    "page_size",
                    format!("The `page_size` field must be between 1 and {MAX_PAGE_SIZE}."),
                );
            }

            if binder.has_errors() {
                return Err(binder.into_error());
            }
            Ok(PagedRequest {
                page: page.max(1),
                page_size: page_size.clamp(1, MAX_PAGE_SIZE),
            })
        }
    }
}

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
