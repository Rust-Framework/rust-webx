use rust_webapp_core::error::{Error, Result};

#[test]
fn error_status_code_http_maps_to_400() {
    let err = Error::Http("bad request".into());
    assert_eq!(err.status_code(), 400);
}

#[test]
fn error_status_code_di_maps_to_500() {
    let err = Error::Di("di failure".into());
    assert_eq!(err.status_code(), 500);
}

#[test]
fn error_status_code_routing_maps_to_404() {
    let err = Error::Routing("route not found".into());
    assert_eq!(err.status_code(), 404);
}

#[test]
fn error_status_code_serialization_maps_to_400() {
    let err =
        Error::Serialization(serde_json::from_str::<serde_json::Value>("invalid").unwrap_err());
    assert_eq!(err.status_code(), 400);
}

#[test]
fn error_status_code_internal_maps_to_500() {
    let err = Error::Internal("internal error".into());
    assert_eq!(err.status_code(), 500);
}

#[test]
fn error_status_code_message_maps_to_500() {
    let err = Error::Message("something went wrong".into());
    assert_eq!(err.status_code(), 500);
}

#[test]
fn error_status_code_validation_maps_to_400() {
    let err = Error::Validation("invalid input".into());
    assert_eq!(err.status_code(), 400);
}

#[test]
fn error_status_code_not_found_maps_to_404() {
    let err = Error::NotFound("resource missing".into());
    assert_eq!(err.status_code(), 404);
}

#[test]
fn error_display_http() {
    let err = Error::Http("bad request".into());
    assert_eq!(err.to_string(), "HTTP error: bad request");
}

#[test]
fn error_display_di() {
    let err = Error::Di("service not found".into());
    assert_eq!(err.to_string(), "DI error: service not found");
}

#[test]
fn error_display_not_found() {
    let err = Error::NotFound("user 42".into());
    assert_eq!(err.to_string(), "user 42");
}

#[test]
fn error_display_validation() {
    let err = Error::Validation("email required".into());
    assert_eq!(err.to_string(), "email required");
}

#[test]
fn error_display_serialization() {
    let err = Error::Serialization(serde_json::from_str::<i32>("\"not_a_number\"").unwrap_err());
    assert!(err.to_string().contains("Serialization error"));
}

#[test]
fn error_debug_format() {
    let err = Error::NotFound("test".into());
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
        Err(Error::NotFound("gone".into()))
    }
    assert!(returns_error().is_err());
}
