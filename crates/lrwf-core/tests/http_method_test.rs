use lrwf_core::routing::HttpMethod;

#[test]
fn http_method_as_str_get() {
    assert_eq!(HttpMethod::Get.as_str(), "GET");
}

#[test]
fn http_method_as_str_post() {
    assert_eq!(HttpMethod::Post.as_str(), "POST");
}

#[test]
fn http_method_as_str_put() {
    assert_eq!(HttpMethod::Put.as_str(), "PUT");
}

#[test]
fn http_method_as_str_delete() {
    assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
}

#[test]
fn http_method_as_str_patch() {
    assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
}

#[test]
fn http_method_as_str_head() {
    assert_eq!(HttpMethod::Head.as_str(), "HEAD");
}

#[test]
fn http_method_as_str_options() {
    assert_eq!(HttpMethod::Options.as_str(), "OPTIONS");
}

#[test]
fn http_method_from_str_valid() {
    assert_eq!(HttpMethod::from_str("GET"), Some(HttpMethod::Get));
    assert_eq!(HttpMethod::from_str("POST"), Some(HttpMethod::Post));
    assert_eq!(HttpMethod::from_str("PUT"), Some(HttpMethod::Put));
    assert_eq!(HttpMethod::from_str("DELETE"), Some(HttpMethod::Delete));
    assert_eq!(HttpMethod::from_str("PATCH"), Some(HttpMethod::Patch));
    assert_eq!(HttpMethod::from_str("HEAD"), Some(HttpMethod::Head));
    assert_eq!(HttpMethod::from_str("OPTIONS"), Some(HttpMethod::Options));
}

#[test]
fn http_method_from_str_invalid() {
    assert_eq!(HttpMethod::from_str("INVALID"), None);
    assert_eq!(HttpMethod::from_str(""), None);
    assert_eq!(HttpMethod::from_str("get"), None); // case-sensitive
}

#[test]
fn http_method_debug_format() {
    assert_eq!(format!("{:?}", HttpMethod::Get), "Get");
    assert_eq!(format!("{:?}", HttpMethod::Post), "Post");
}

#[test]
fn http_method_equality() {
    assert_eq!(HttpMethod::Get, HttpMethod::Get);
    assert_ne!(HttpMethod::Get, HttpMethod::Post);
}

#[test]
fn http_method_clone() {
    let method = HttpMethod::Get;
    let cloned = method.clone();
    assert_eq!(method, cloned);
}

#[test]
fn http_method_copy() {
    let method = HttpMethod::Post;
    let copied = method;
    assert_eq!(method, copied);
}
