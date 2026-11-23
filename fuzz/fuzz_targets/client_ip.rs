#![no_main]

//! Fuzz target: forwarded-header client resolution (`IpCidr`,
//! `client_from_forwarded_for`, `resolve_client_info`).
//! Logic and invariants live in `fuzz/src/lib.rs` (unit-tested on stable).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rust_webx_fuzz::fuzz_client_ip(data);
});
