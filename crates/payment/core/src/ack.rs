//! Provider-specific callback acknowledgements.

use webx_core::action::{ContentResult, JsonResult, Response, StatusCodeResult};

/// `200 OK` with no body (Stripe, PayPal, Adyen, Square).
pub fn empty_ok_ack() -> Response {
    Box::new(StatusCodeResult(200))
}

/// Alipay's literal `success` body.
pub fn text_success_ack() -> Response {
    Box::new(ContentResult::text("success"))
}

/// WeChat Pay APIv3 success envelope.
pub fn json_success_ack() -> Response {
    Box::new(JsonResult::new(serde_json::json!({
        "code": "SUCCESS",
        "message": "成功"
    })))
}
