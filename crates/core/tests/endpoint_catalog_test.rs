//! Endpoint identity: a duplicate `request_id` is a diagnosable startup error,
//! never a panic and never an ambiguous permission key.
//!
//! This test binary deliberately submits two routes with one identity; the
//! real route table is in the host/webx test binaries.

use webx_core::localization::TextRef;
use webx_core::route::catalog::{EndpointCatalog, EndpointGuid};
use webx_core::route::diagnostics::format_route_diagnostics;
use webx_core::route::scan::RouteEntry;
use webx_core::routing::HttpMethod;

const DUPLICATE_ID: &str = "endpoint_catalog_test::CreateOrderRequest";

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Get,
        "/api/orders",
        "CreateOrderRequest",
        "String",
        TextRef::text("orders"),
        TextRef::text(""),
        TextRef::text(""),
        &[],
        &[],
    )
    .with_request_id(DUPLICATE_ID)
}

inventory::submit! {
    RouteEntry::new(
        HttpMethod::Post,
        "/api/orders",
        "CreateOrderRequest",
        "String",
        TextRef::text("orders"),
        TextRef::text(""),
        TextRef::text(""),
        &[],
        &[],
    )
    .with_request_id(DUPLICATE_ID)
}

#[test]
fn duplicate_endpoint_ids_fail_the_catalog_build_without_panicking() {
    let Err(error) = EndpointCatalog::try_from_inventory() else {
        panic!("two routes share one request identity, the catalog must not build");
    };

    // Operator-facing startup failure: the client never sees the detail.
    assert_eq!(error.code(), "dependency_injection");
    assert_eq!(error.status_code(), 500);
    assert_eq!(error.public_detail(), None);

    let operator = error.operator().expect("operator-facing detail");
    assert!(operator.contains(DUPLICATE_ID), "{operator}");
    assert!(operator.contains("GET /api/orders"), "{operator}");
    assert!(operator.contains("POST /api/orders"), "{operator}");
}

#[test]
fn the_historic_catalog_entry_point_is_fallible_too() {
    let Err(error) = EndpointCatalog::from_inventory() else {
        panic!("from_inventory must not silently accept a duplicate id");
    };
    assert!(
        error.operator().unwrap_or_default().contains(DUPLICATE_ID),
        "{error}"
    );
}

#[test]
fn doctor_report_lists_the_duplicate_endpoint_id() {
    let report = format_route_diagnostics();
    assert!(report.contains("Duplicate endpoint ids"), "{report}");
    assert!(report.contains(DUPLICATE_ID), "{report}");
    assert!(report.contains("GET /api/orders"), "{report}");
    assert!(report.contains("POST /api/orders"), "{report}");
}

/// Pins the RFC 4122 v5 derivation shared by the endpoint catalog and the
/// OpenAPI extensions: an existing permission table must keep resolving.
#[test]
fn endpoint_guid_is_a_stable_rfc4122_v5_derivation() {
    let guid =
        EndpointGuid::from_request_id("docbit_contracts::blog::CreateBlogRequest").to_string();

    assert_eq!(guid, "c316a5b8-d7f5-52a8-872b-2eb152b7d58d");
    assert_eq!(guid.len(), 36);
    assert_eq!(&guid[14..15], "5", "version 5, got {guid}");
    assert_eq!(&guid[19..20], "8", "RFC 4122 variant, got {guid}");
}
