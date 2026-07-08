//! OpenAPI spec generation tests.

use rust_webx_openapi::generate_openapi_spec;

#[test]
fn openapi_spec_has_required_top_level_fields() {
    let spec = generate_openapi_spec("Test API", "1.0.0");
    assert_eq!(spec["openapi"], "3.0.3");
    assert_eq!(spec["info"]["title"], "Test API");
    assert_eq!(spec["info"]["version"], "1.0.0");
    assert!(spec["paths"].is_object());
}
