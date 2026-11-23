#![no_main]

//! Fuzz target: keyset cursor decoding (`webx_core::keyset::Cursor::decode`).
//! Logic and invariants live in `fuzz/src/lib.rs` (unit-tested on stable).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rust_webx_fuzz::fuzz_cursor_decode(data);
});
