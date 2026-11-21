# 稳定性门禁（soak / chaos / fuzz / p99 / alloc / 安全）

> 快速门禁在 PR CI；本页描述的慢测试默认 `#[ignore]`，由
> `.github/workflows/stability.yml` 每天 03:00 UTC 运行，也可手动触发。

## 1. Soak（`crates/webx/tests/stability_soak_test.rs`）

持续对 `/echo` 施加 4 条 keep-alive 连接的请求流，检查：

- 全部请求 200（任何失败即失败）；
- 吞吐 sanity（每秒至少 25 个请求）；
- soak 结束后服务器仍能服务，`http_requests_inflight` 回到基线（scrape 自身算 1）；
- **Linux** 上 RSS 增长不超过 `SOAK_MAX_RSS_GROWTH_MB`（默认 256MB）；
- 调用 `ServerHandle::shutdown()` 后端口在 5 秒内停止接受新连接。

```text
SOAK_SECONDS=300 cargo test -p rust-webx --test stability_soak_test -- --ignored --nocapture
SOAK_SECONDS=86400 SOAK_MAX_RSS_GROWTH_MB=512 \
  cargo test -p rust-webx --test stability_soak_test -- --ignored --nocapture   # 24h
```

RSS 仅在 Linux 断言（读 `/proc/self/statm`）；其他平台跳过该项。
nightly 默认 `SOAK_SECONDS=300`；30min/1h/24h 通过 `workflow_dispatch` 指定。

## 2. Chaos（`crates/webx/tests/stability_chaos_test.rs`，5 场景）

| 场景 | 断言 |
|---|---|
| `chaos_connection_storm_is_shed_with_503`：`MaxConnections=4` + 24 条并发连接 | 至少一条被 `503` 卸载；状态只允许 `200/503/0`；风暴结束后恢复服务 |
| `chaos_graceful_shutdown_under_load`：带负载调用 `shutdown()` | 在途请求完成、**无 5xx**；端口停止接受连接 |
| `chaos_silent_upstream_times_out`：上游 accept 后永不响应 | `HttpClient` 在 5 秒内返回错误（`request_timeout`/`read_timeout` 生效） |
| `chaos_handler_panic_is_contained`：15 个并发请求打到 `panic!` handler | 每个请求得到 typed `500`（problem+json）；worker 存活，后续 `/echo` 200 |
| `chaos_oversized_body_is_shed_and_connection_survives`：超过 `#[max_body_size]` 的 body | `413` 且同一 keep-alive 连接随后仍可用（`200`） |

```text
cargo test -p rust-webx --test stability_chaos_test -- --ignored --nocapture --test-threads=1
```

## 3. Fuzz

**属性测试（PR 门禁）**：`crates/webx/tests/property_test.rs`，路由匹配、动态段捕获、
分页边界、`ProblemDetails` 往返各 128 用例。

**线路级 fuzz-lite（PR 门禁）**：`crates/webx/tests/fuzz_lite_test.rs`——固定种子 PRNG
生成 240 条畸形请求（混乱 method/path/header/body、纯二进制垃圾），断言：

- 每个输入要么得到格式合法的 HTTP 状态（100–599），要么被服务端关闭连接；
- 不允许挂起、不允许畸形状态行；
- 扫描结束后 `/echo` 仍返回 200。

CI 以 fuzz-lite + proptest 作为可重复的零 panic 门禁；完整 `cargo-fuzz` 语料库属于后续增强。

## 4. 线路延迟 p99（`crates/webx/tests/wire_p99_test.rs`）

keep-alive localhost 连接上 100 次预热 + 500 次采样的 `/hello` 请求，断言
`p99 ≤ WIRE_P99_BUDGET_MS`（nightly release 设为 **5ms**）。本地实测：

| 构建 | p50 | p99 |
|---|---:|---:|
| debug | 161.2µs | 254.1µs |
| release | 113.2µs | **165.3µs** |

```text
WIRE_P99_BUDGET_MS=5 cargo test --release -p rust-webx --test wire_p99_test -- --ignored --nocapture
```

## 5. 分配门禁（PR CI）

`cargo run -p alloc-probe --release --locked -- compare 500 12`：同机比较 framework
与 hyper 地板，只对**框架税**设限；当前实测 tax **10.42**，门禁 12。细节与 ≤8 路径见
[`alloc-budget.md`](alloc-budget.md)。

## 6. 安全审计

- PR CI `audit` job：`cargo install cargo-audit --locked && cargo audit`（RustSec 漏洞库），
  当前 0 vulnerabilities / 0 warnings；
- 依赖来源：仅 crates.io + 锁定 `Cargo.lock`；`unsafe_code = "forbid"` 覆盖全部框架 crate
  （`alloc-probe` 为独立开发工具，显式说明其 `GlobalAlloc` 用途）；
- 安全相关回归门禁：`session_revocation_test`、`csrf`/`cookie` 测试、`constant_time_eq`
  单测、`OneTimeSecret` 单测、auth p99 预算（`auth_budget_test`）。

## 7. 与其他门禁的关系

| 门禁 | 触发 | 内容 |
|---|---|---|
| tests / clippy / fmt / MSRV / docs / coverage | PR | 正确性与可维护性 |
| semver（vs v2.1.0） | PR | 公开 API 加性演进（4 包） |
| upgrade（N-2） | PR | 2.0.0 与当前 workspace 同源码编译 |
| alloc | PR | 框架税 ≤12 |
| stability：soak / chaos / wire-perf / auth-budget / auth-interop | nightly | 长跑与性能预算 |
| audit | PR + 定期 | RustSec 漏洞 |

## 8. 失败时先看什么

1. soak 失败：先看 RSS 增长是否异常（泄漏）或吞吐是否塌陷（死锁/阻塞）；
2. storm 失败：检查 `engine_connections_rejected_total` 与 `AdmissionTimeoutMs`；
3. shutdown 失败：检查 `Server.DrainTimeoutSeconds` 与是否有长响应（SSE）占用排空预算；
4. silent upstream 失败：检查 `connect_timeout`/`request_timeout`/`read_timeout` 是否被显式关闭；
5. panic 场景失败：检查该请求是否 500 + problem+json，且后续请求是否 200（worker 存活）；
6. p99 失败：先确认是否 debug 构建/共享 CI runner 噪声，再看是否引入了每请求额外分配或阻塞；
7. fuzz-lite 失败：保存最小输入（测试会打印探测结果），先修解析边界再跑全量。
