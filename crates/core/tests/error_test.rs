use webx_core::error::{Error, Result};

#[test]
fn error_status_code_http_maps_to_400() {
    let err = Error::bad_request("bad request");
    assert_eq!(err.status_code(), 400);
}

#[test]
fn error_status_code_di_maps_to_500() {
    let err = Error::di("di failure");
    assert_eq!(err.status_code(), 500);
}

#[test]
fn error_status_code_routing_maps_to_404() {
    let err = Error::route_not_found("route not found");
    assert_eq!(err.status_code(), 404);
}

#[test]
fn error_status_code_serialization_maps_to_400() {
    let err =
        Error::invalid_json(serde_json::from_str::<serde_json::Value>("invalid").unwrap_err());
    assert_eq!(err.status_code(), 400);
}

#[test]
fn error_status_code_internal_maps_to_500() {
    let err = Error::internal("internal error");
    assert_eq!(err.status_code(), 500);
}

#[test]
fn error_status_code_message_maps_to_500() {
    let err = Error::internal("something went wrong");
    assert_eq!(err.status_code(), 500);
}

#[test]
fn error_status_code_validation_maps_to_400() {
    let err = Error::validation("invalid input");
    assert_eq!(err.status_code(), 400);
}

#[test]
fn error_status_code_not_found_maps_to_404() {
    let err = Error::not_found("resource missing");
    assert_eq!(err.status_code(), 404);
}

#[test]
fn error_display_http() {
    let err = Error::bad_request("bad request");
    assert_eq!(err.to_string(), "bad request");
}

#[test]
fn error_display_di() {
    let err = Error::di("service not found");
    assert_eq!(err.to_string(), "service not found");
}

#[test]
fn error_display_not_found() {
    let err = Error::not_found("user 42");
    assert_eq!(err.to_string(), "user 42");
}

#[test]
fn error_display_validation() {
    let err = Error::validation("email required");
    assert_eq!(err.to_string(), "email required");
}

#[test]
fn error_display_serialization() {
    let err = Error::invalid_json(serde_json::from_str::<i32>("\"not_a_number\"").unwrap_err());
    assert!(err.to_string().contains("expected"));
}

#[test]
fn error_debug_format() {
    let err = Error::not_found("test");
    let debug_str = format!("{:?}", err);
    assert!(debug_str.contains("NotFound"));
    assert!(debug_str.contains("test"));
}

#[test]
fn result_type_alias_works() {
    fn returns_result() -> Result<String> {
        Ok("success".to_string())
    }
    assert_eq!(returns_result().unwrap(), "success");

    fn returns_error() -> Result<String> {
        Err(Error::not_found("gone"))
    }
    assert!(returns_error().is_err());
}
