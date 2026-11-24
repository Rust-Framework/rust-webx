# rust-webx fuzz targets (`cargo-fuzz`)

Coverage-guided fuzzing for rust-webx's public, side-effect-free parse entry
points. The tested contract is "arbitrary input never panics and never breaks a
documented round-trip/containment invariant"; crashes are minimized by libFuzzer
and uploaded as CI artifacts.

## Layout

```text
fuzz/
├── Cargo.toml            # standalone workspace (`[workspace]`), NOT a repo member
├── src/lib.rs            # all fuzz logic + invariants, ordinary #[test] smoke tests
├── fuzz_targets/*.rs     # #![no_main] + fuzz_target! thin wrappers (one per target)
├── corpus/<target>/      # committed seed corpus (handwritten, tiny)
└── Cargo.lock            # pinned fuzz-only dependency graph
```

### Workspace exclusion

`fuzz/Cargo.toml` contains its own empty `[workspace]` table, which makes this
directory a separate workspace root. That is the exclusion mechanism this repo
chose (the alternative is a one-line `exclude = ["fuzz"]` in the root
`Cargo.toml`, which would couple the repo workspace to the fuzz build).

Consequences, by design:

- the main workspace (and its `Cargo.lock`, MSRV check, clippy, `-D warnings`,
  `unsafe_code = "forbid"` and `cargo fmt --all`) never sees this package;
- fuzz builds may use nightly, sanitizers and third-party `unsafe` code
  (libFuzzer) without changing how the published crates are built;
- path dependencies point at `../crates/core`; framework crates stay pinned by
  the main workspace's own `=2.9.0` version policy.

## Targets

| Target | Public entry point under test |
|---|---|
| `cursor_decode` | `webx_core::keyset::Cursor::decode` (keyset pagination cursors) |
| `config_bind` | `webx_core::config::try_bind_root` / `try_bind_config` on arbitrary JSON |
| `client_ip` | `webx_core::client_info::IpCidr`, `client_from_forwarded_for`, `resolve_client_info` |
| `problem_details` | `webx_core::problem::ProblemDetails` serde round-trip + `complete()` |
| `file_name_sanitize` | `webx_core::form::sanitize_file_name` (upload basename) |
| `event_wire` | `webx_core::mediator::event::EventMessage::{from_wire, to_wire}` |

`client_ip` input format is documented in `src/lib.rs`: first line is a
comma-separated trusted-proxy CIDR list, the remainder is one
`X-Forwarded-For` value.

## Run locally

```text
# Logic smoke tests: stable toolchain, no libFuzzer needed.
cargo test --manifest-path fuzz/Cargo.toml --lib

# Typecheck every wrapper (still stable; libFuzzer link happens in CI).
cargo check --manifest-path fuzz/Cargo.toml --all-targets

# Real fuzzing: nightly + cargo-fuzz.
cargo install cargo-fuzz --locked
cd fuzz
cargo +nightly fuzz run cursor_decode -- -max_total_time=60
cargo +nightly fuzz list
```

`cargo +nightly fuzz run` needs a nightly toolchain and links the libFuzzer
runtime; on Windows/MSVC the C++ runtime builds but the final link has no
libFuzzer entry point, so fuzz runs are Linux/macOS only (the nightly CI job in
`.github/workflows/fuzz.yml` is the real gate).

Each `cargo fuzz run` keeps its evolving corpus in `fuzz/corpus/<target>/`.
Only the handwritten seeds are committed (`.gitignore` covers `target/` and
`artifacts/`); a local run grows that directory, so review and commit genuinely
useful seeds by hand instead of checking in the whole evolved corpus.
Crash reproducers are written to `fuzz/artifacts/<target>/` and uploaded by CI to
reproduce with:

```text
cargo +nightly fuzz run <target> artifacts/<target>/<crash-file>
```

## Adding a target

1. add the parse logic + invariants as `pub fn fuzz_<name>(data: &[u8])` in
   `src/lib.rs`, with a `#[test]` smoke case;
2. add a thin `#![no_main]` wrapper in `fuzz_targets/<name>.rs` and a `[[bin]]`
   entry in `Cargo.toml` (`test = false, doc = false, bench = false`);
3. add seeds under `corpus/<name>/` and the target to the matrix in
   `.github/workflows/fuzz.yml`;
4. keep the fuzz surface to **public, network- and DB-free** APIs.
