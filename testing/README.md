# testing/ — non-published test harnesses

`crates/` is the **published** framework surface (every package there is released
to crates.io in lockstep). Everything in `testing/` is a development-time
harness: `publish = false`, never linked by the framework, and never part of the
release. `scripts/check-crate-layout.py` enforces both directions.

| Package | Purpose | Gate |
|---|---|---|
| `alloc-probe` | Measures the framework allocation tax against a bare-hyper floor (`compare <requests> <tax-budget>`). It needs a `GlobalAlloc` wrapper, which the framework crates forbid. | PR CI `alloc` job; evidence in [`docs/design/alloc-budget.md`](../docs/design/alloc-budget.md) |
| `contract-tests` | Reusable behavior-contract suites for framework ports (`IObjectStore`, `IMediaPlaneAdapter`, `IEventPublisher`). Adapter crates depend on it as a dev-dependency. | PR CI tests of the adapters and of this crate |
| `upgrade-smoke` | N-2 source-compatibility smoke: the same app compiled against the previous-but-one published release (`rust-webx = "=2.7.0"` for the 2.9 line) and against the current workspace path. | PR CI `upgrade` job; policy in [`docs/design/compatibility.md`](../docs/design/compatibility.md) |
| `load-probe` | End-to-end load harness: the same requests over a real socket against a `rust-webx` host and a bare-hyper service, reporting req/s and p50/p90/p99 plus the framework tax. | Manual/release evidence (see [`load-probe/README.md`](load-probe/README.md)); shared CI runners are too noisy for an absolute throughput gate |

Rules for this directory:

- a package here must set `publish = false`;
- do not add a package here that is actually a framework capability (it belongs
  in `crates/` or a domain group);
- keep each harness self-contained: it may use workspace dependencies but must
  not be linked by published crates except as a `[dev-dependencies]` entry.

The `fuzz/` directory at the repository root is a separate, standalone
`cargo-fuzz` workspace (nightly/libFuzzer); it is intentionally outside this
workspace and outside the publish surface.
