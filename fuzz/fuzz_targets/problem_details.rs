#![no_main]

//! Fuzz target: RFC 9457 `ProblemDetails` JSON round-trip.
//! Logic and invariants live in `fuzz/src/lib.rs` (unit-tested on stable).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rust_webx_fuzz::fuzz_problem_details(data);
});
