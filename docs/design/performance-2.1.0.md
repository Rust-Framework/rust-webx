# rust-webx 2.1.0 Performance Report

> Measured for the `v2.1.0` release candidate on the same host, back-to-back
> against `v2.0.0`. This is a local reference measurement, not an absolute
> capacity guarantee: the host is shared, so RPS varies with co-tenant CPU load.
> The allocation number is deterministic for this request path and is the hard
> regression signal used for this release.

## Environment

| Item | Value |
|---|---|
| CPU | Intel Core Ultra 7 155H, 16 cores / 22 logical |
| OS | Windows 11 (10.0.26200) |
| Rust | 1.98.1 (benchmark build; MSRV remains 1.95) |
| Profile | `opt-level=3`, `lto="thin"`, `codegen-units=1` |
| Server | `MODE=prod`, 4 workers, loopback, default MaxConnections |
| Load | 16 keep-alive connections, 3 s per round, 5 alternating rounds |
| Load generator | in-repo sibling `rw-bench` loadgen, `/hello` JSON endpoint |
| Response | 20-byte JSON DTO (`{"message":"Hello, World!"}`) |

## Throughput and latency

Alternating runs, 2.0 and 2.1 on the same machine and same harness:

| Round | 2.0 RPS | 2.1 RPS | 2.0 p50/p99 (µs) | 2.1 p50/p99 (µs) |
|---:|---:|---:|---:|---:|
| 1 | 120,190 | 104,395 | 137 / 332 | 151 / 355 |
| 2 | 138,619 | 152,199 | 115 / 341 | 109 / 242 |
| 3 | 168,842 | 162,512 | 95 / 212 | 99 / 229 |
| 4 | 159,876 | 150,206 | 105 / 226 | 104 / 272 |
| 5 | 158,092 | 154,946 | 99 / 250 | 107 / 268 |
| **Median** | **158,092** | **152,199** | **105 / 250** | **107 / 268** |

A separate 32-connection burst run reached 249,764 RPS (2.0) and 245,923 RPS
(2.1) in the first round; later rounds on the same host were dominated by
co-tenant load, so those numbers are not used as a capacity claim.

**Interpretation:** median latency is effectively unchanged, and the 3.7% RPS
difference is inside the observed ±20% run-to-run variance on this shared host.
There is no evidence of a meaningful throughput regression. The 2.1 change adds
one indirect route-encoder call per direct JSON response, but preserves the
allocation-free direct path.

## Allocations

The `/allocs` endpoint counts every global allocation (`alloc`, `alloc_zeroed`,
`realloc`) while serving 1-connection sequential requests for 5 s. Two
back-to-back runs per version:

| Version | Run 1 allocs/request | Run 2 allocs/request |
|---|---:|---:|
| 2.0.0 | 23.07 | 23.06 |
| 2.1.0 | **22.06** | **22.06** |

**Result: 2.1.0 removes 1.01 allocations per request, about a 4.4% reduction**
on the direct JSON path. This is the concrete performance improvement of this
release; the route-owned encoder avoids the `Box<dyn Any>` that the simpler
"encode after dispatch" design would have required.

## What changed in the hot path

- `#[handler]` still generates the direct bridge and binds/constructs without a
  mediator or `Box<dyn Any>` request/response.
- Response serialization is now a route-owned function pointer passed to the
  bridge; the response is encoded by reference, not boxed.
- The direct bridge is reached with one `RouteTarget` trait object; the former
  host-only double `IEndpoint` adapter and duplicate route table are gone.
- No per-request allocation was added by the refactor; the measured reduction
  comes from the 2.1 engine/lifecycle refactor as a whole.

## Quality checks for the release candidate

| Check | Result |
|---|---|
| `cargo test --workspace --locked` | 995 passed, 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo doc --workspace --no-deps` with `-D warnings` | pass |
| `cargo +1.95.0 check --workspace --all-targets` | pass |
| `cargo check -p rust-webx --all-features/--no-default-features` | pass |
| `cargo build --release --workspace` | pass |
| `cargo audit` | no vulnerabilities; 2 unmaintained warnings (`chrono`, `rustls-pemfile`) |
| `scripts/check-docs.sh` | pass, version 2.1.0 |
| Host bench smoke (router/pipeline) | pass |

## Limits of this report

- The host is not dedicated; absolute RPS is not reproducible enough to enforce
  a CI threshold. CI keeps the allocation/contract tests and bench smoke, while
  this report records the local release-candidate comparison.
- `chrono` and `rustls-pemfile` are currently only "unmaintained" advisories,
  not vulnerabilities; re-evaluate on the next dependency refresh.
