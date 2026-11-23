#![no_main]

//! Fuzz target: upload filename sanitisation (`sanitize_file_name`).
//! Logic and invariants live in `fuzz/src/lib.rs` (unit-tested on stable).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rust_webx_fuzz::fuzz_file_name_sanitize(data);
});
