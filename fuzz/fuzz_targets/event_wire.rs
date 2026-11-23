#![no_main]

//! Fuzz target: broker event wire envelope (`EventMessage::from_wire`).
//! Logic and invariants live in `fuzz/src/lib.rs` (unit-tested on stable).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rust_webx_fuzz::fuzz_event_wire(data);
});
