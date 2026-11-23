#![no_main]

//! Fuzz target: strict config binding (`try_bind_root` / `try_bind_config`).
//! Logic and invariants live in `fuzz/src/lib.rs` (unit-tested on stable).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rust_webx_fuzz::fuzz_config_bind(data);
});
