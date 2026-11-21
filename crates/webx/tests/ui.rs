//! Compile-fail snapshots for the route/handler macros.
//!
//! A misplaced attribute must fail **at compile time** with an actionable
//! message. Before 2.2 the route macro silently accepted a wrong impl and the
//! handler macro silently ignored an unknown trait, so the mistake only showed
//! up at startup (orphan route) or as a missing registration.

#[test]
fn macro_misuse_is_a_compile_error() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
