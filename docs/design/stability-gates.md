# 稳定性门禁（soak / chaos / fuzz / p99 / alloc / 安全 / 互操作）

> 快速门禁在 PR CI；本页描述的慢测试默认 `#[ignore]`，由
> `.github/workflows/stability.yml` 每天 03:00 UTC 运行，也可手动触发。
> 完整 cargo-fuzz 语料由独立的 `.github/workflows/fuzz.yml` 每天 04:00 UTC
> 运行（fuzz workspace 与仓库 workspace 分离，见 §3.3）。

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

### 3.1 PR 门禁：属性测试 + fuzz-lite

**属性测试（PR 门禁）**：`crates/webx/tests/property_test.rs`，路由匹配、动态段捕获、
分页边界、`ProblemDetails` 往返各 128 用例。

**线路级 fuzz-lite（PR 门禁）**：`crates/webx/tests/fuzz_lite_test.rs`——固定种子 PRNG
生成 240 条畸形请求（混乱 method/path/header/body、纯二进制垃圾），断言：

- 每个输入要么得到格式合法的 HTTP 状态（100–599），要么被服务端关闭连接；
- 不允许挂起、不允许畸形状态行；
- 扫描结束后 `/echo` 仍返回 200。

### 3.2 Nightly：cargo-fuzz 语料库

完整 `cargo-fuzz`（libFuzzer 覆盖引导）由 `.github/workflows/fuzz.yml` 每日运行，
每个 target 限时（默认 60s，`workflow_dispatch` 可调），语料与崩溃样本作为 artifact 上传。

fuzz crate 是**独立 workspace**（`fuzz/Cargo.toml` 自带空 `[workspace]` 表），
不进入仓库 workspace：nightly、sanitizer 与第三方 `unsafe`（libFuzzer）只影响
`fuzz/`，不改变已发布 crate 的构建、MSRV、clippy 与根 `Cargo.lock`。仅依赖
`crates/core`（path 依赖），因此只覆盖**公开、无网络/DB 副作用**的解析入口。
运行方式、目录结构与“新增 target”清单见 [`fuzz/README.md`](../../fuzz/README.md)。

| target | 公开入口 | 不变量 |
|---|---|---|
| `cursor_decode` | `keyset::Cursor::decode` | 成功解码的游标 `encode` 后必须再解出相同分量 |
| `config_bind` | `config::try_bind_root` / `try_bind_config` | 任意 JSON 不 panic；绑定结果确定；根绑定与分区绑定一致 |
| `client_ip` | `client_info::{IpCidr, client_from_forwarded_for, resolve_client_info}` | 解析出的客户端地址必须来自转发链，可信 peer 必须原样暴露链路 |
| `problem_details` | `problem::ProblemDetails` serde | 序列化往返保 `status`/`title`；`complete()` 必填 `code`/`retryable` |
| `file_name_sanitize` | `form::sanitize_file_name` | 非空、≤200 字节、无分隔符/控制字符/保留设备名、幂等 |
| `event_wire` | `mediator::event::EventMessage::{from_wire, to_wire}` | 接受的帧必须逐字节往返 |

```text
# 无 libFuzzer 也能跑的普通测试（稳定工具链）
cargo test --manifest-path fuzz/Cargo.toml --lib

# 真 fuzz（nightly + cargo-fuzz；Linux/macOS）
cd fuzz && cargo +nightly fuzz run cursor_decode -- -max_total_time=60
```

语料策略：`fuzz/corpus/<target>/` 提交少量手写种子（合法输入 + 已知边界：路径穿越、
Windows 设备名、坏版本字节、未知框架配置键等）；nightly 运行会扩充语料并回传 artifact，
只有确认有价值的输入才手工提交。逻辑函数同时带 `#[test]` 冒烟与确定性随机冒烟，
所以 Windows/MSVC（libFuzzer 无入口点）上也能 `cargo check --manifest-path fuzz/Cargo.toml
--all-targets` + `cargo test --lib` 验证，真正的链接与运行证据来自 Linux nightly CI。

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
  当前 **0 vulnerabilities** + 1 条 `unmaintained` 警告（`chrono` 经 `rskafka`/`rust-ef`
  传递；TLS PEM 解析已迁到维护中的 `rustls-pki-types`，`rustls-pemfile` 不再在依赖图中）；
  该警告为允许级，跟踪上游迁移，不阻塞发布；
- 依赖来源：仅 crates.io + 锁定 `Cargo.lock`；`unsafe_code = "forbid"` 覆盖全部框架 crate
  （`alloc-probe` 为独立开发工具，显式说明其 `GlobalAlloc` 用途）；
- 安全相关回归门禁：`session_revocation_test`、`csrf`/`cookie` 测试、`constant_time_eq`
  单测、`OneTimeSecret` 单测、auth p99 预算（`auth_budget_test`）。

## 7. 互操作 / 一致性门禁（conformance）

`stability.yml` 的 `conformance` job（Ubuntu）把框架当作**黑盒 HTTP 服务器**验证，
外部客户端只通过真实 socket 访问，不经过进程内 `TestHost`。入口是
`scripts/conformance.sh`（可本地跑），它构建并启动
`crates/webx/examples/auth_quickstart.rs`（`APP_ENV=Development`，固定端口 5000），
然后跑 curl 矩阵：

| 维度 | 覆盖 |
|---|---|
| 协议 | HTTP/1.1；HTTP/2 **h2c prior knowledge** |
| 成功路径 | `GET /health/live` 200；`POST /auth/login` 200 + bearer token；带 token 的 `GET /me` 200 |
| 认证 | 无凭据 `GET /me` 401 + `WWW-Authenticate: Bearer` challenge |
| 错误信封 | 401/400/413/415 均为 `application/problem+json`，含 `status`、`code`、`retryable` |
| 请求边界 | `Content-Type: text/plain` → 415；畸形 JSON → 400；声明 body 超过 `App.MaxBodySize` → 413 |
| HTTP/2 wire 一致性 | 安装 h2spec 时对同一 listener 跑 `h2spec http2 hpack generic`（h2c，`-P /health/live`） |

```text
bash scripts/conformance.sh                      # 本地 / CI
H2SPEC_MAX_FAILURES=0 bash scripts/conformance.sh  # 基线收紧后
```

失败即退出（curl 矩阵硬门禁）。h2spec 是**有界**门禁：当前基线为
146 tests / 145 passed / **1 failed**（`Sends invalid connection preface`：服务器直接关闭
TCP 连接而不是先发 `GOAWAY(PROTOCOL_ERROR)`），因此 `H2SPEC_MAX_FAILURES` 默认 5，
既容忍该已知偏差又能在明显回归时失败；修掉该偏差后应下调到 0（或 1 作为抖动余量）。
h2spec 未安装/下载失败时脚本打印 SKIP，curl 矩阵仍是强制项（job 不会因安装失败直接变红）。

TLS 上的 **ALPN h2 协商**不在该脚本里：示例场景没有 TLS listener。同一 job 先跑
`cargo test -p rust-webx-host --test http2_test`，覆盖 h2c prior knowledge、
HTTP/1.1 与真实 TLS ALPN 三条路径。

## 8. 与其他门禁的关系

| 门禁 | 触发 | 内容 |
|---|---|---|
| tests / clippy / fmt / MSRV / docs / coverage | PR | 正确性与可维护性 |
| semver（vs v2.7.0） | PR | 公开 API 加性演进（4 包） |
| upgrade（N-2） | PR | rust-webx 2.7.0 与当前 workspace 同源码编译（`testing/upgrade-smoke`） |
| alloc | PR | 框架税 ≤12 |
| stability：soak / chaos / wire-perf / auth-budget / auth-interop / conformance | nightly | 长跑、性能预算与 HTTP 互操作 |
| fuzz：cargo-fuzz（`fuzz.yml`） | nightly | 公开解析入口的覆盖引导 fuzz |
| audit | PR + 定期 | RustSec 漏洞 |

## 9. 失败时先看什么

1. soak 失败：先看 RSS 增长是否异常（泄漏）或吞吐是否塌陷（死锁/阻塞）；
2. storm 失败：检查 `engine_connections_rejected_total` 与 `AdmissionTimeoutMs`；
3. shutdown 失败：检查 `Server.DrainTimeoutSeconds` 与是否有长响应（SSE）占用排空预算；
4. silent upstream 失败：检查 `connect_timeout`/`request_timeout`/`read_timeout` 是否被显式关闭；
5. panic 场景失败：检查该请求是否 500 + problem+json，且后续请求是否 200（worker 存活）；
6. p99 失败：先确认是否 debug 构建/共享 CI runner 噪声，再看是否引入了每请求额外分配或阻塞；
7. fuzz-lite 失败：保存最小输入（测试会打印探测结果），先修解析边界再跑全量；
8. cargo-fuzz 失败：按 artifact 里的崩溃输入复现
   `cd fuzz && cargo +nightly fuzz run <target> artifacts/<target>/<crash>`，
   先跑 `cargo test --manifest-path fuzz/Cargo.toml --lib` 确认不变量测试也复现；
9. conformance 失败：先看 `conformance-logs` artifact 的 `server.log`，区分
   「状态码/信封断言失败」（框架回归）与「连接/端口失败」（环境问题）；
   h2spec 超阈值时按 `Failures:` 段落逐节定位，并确认 `H2SPEC_MAX_FAILURES` 基线是否过期。
