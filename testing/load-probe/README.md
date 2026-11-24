# load-probe — end-to-end load harness

`router_bench` / `pipeline_bench` (`crates/host/benches/`) measure router matching and the
middleware pipeline in isolation; `alloc-probe` counts allocations per request. None of them
answers the question a framework user actually asks: **what does a request over a real socket
cost, and how much of that is the framework?**

`load-probe` runs two servers in one process on loopback:

| row | server |
|---|---|
| `rust-webx (Host::builder)` | the framework app in `src/main.rs`: `#[get("/api/hello")]`, `#[post("/api/echo")]`, full pipeline (routing, binding, serde, error writer, response assembly) |
| `bare hyper 1.x` | a hand-written `hyper::server::conn::http1` service that answers `/api/hello` with the same bytes and echoes `/api/echo` verbatim |

Both rows are driven by the same keep-alive client, with the same connection count, for the same
wall-clock window, after an identical warm-up. Reported per scenario: req/s, p50, p90, p99, max
latency (µs), plus the derived framework tax.

```text
cargo run -p load-probe --release -- --connections 8 --seconds 5
cargo run -p load-probe --release -- --connections 32 --seconds 10
```

## How to read it

- Both servers share this process's CPU, so **absolute RPS is a lower bound** for a dedicated
  machine. The portable number is the ratio between the two rows: that delta is the framework
  pipeline. Record both on the same machine/commit when comparing releases.
- `echo` includes JSON binding + re-serialization on the framework side and raw echo on the
  baseline side, so its delta is the worst case (it contains serde by construction).
- Numbers are only meaningful with `--release`; debug builds measure bounds checks and a
  different allocator profile.

## Baseline record

Keep a dated line here whenever the pipeline changes materially — a regression that is invisible
in `alloc-probe` (for example an extra `spawn` or a lock in the response path) shows up here.

| date | commit | hello req/s (webx / hyper) | hello p50 (webx / hyper) | echo req/s (webx / hyper) | echo p50 (webx / hyper) |
|---|---|---|---|---|---|
| 2026-11-24 | 2.9.1 tree (16 logical CPUs, 8 conns, 5 s) | 46 443 / 27 835 | 161 / 271 µs | 39 746 / 32 868 | 187 / 227 µs |

Reading that row honestly: both servers share one process, one runtime and one CPU budget, and the
framework row is measured first, so the ratio is **not** a claim that the framework beats bare
hyper. The usable signal is the order of magnitude (tens of thousands of req/s on loopback,
sub-millisecond p99, p50 within ~100 µs of a hand-written service). An out-of-process driver and
an interleaved A/B/A/B schedule are the next step before any ratio becomes a gate.
