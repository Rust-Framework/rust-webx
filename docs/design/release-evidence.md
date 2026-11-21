# 版本交付证据（2.2 / 2.3 / 2.4）

> 本文件汇总三条功能线（仓库发布线 2.5.0-rc.1）的**可复现证据**：交付物、验收测试与
> CI 门禁结果。详细台账见 [`framework-constitution.md`](framework-constitution.md)。

## 0. 复现命令

```text
cargo test --workspace --no-fail-fast           # 全量测试（CI `tests`）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo doc --workspace --no-deps --locked         # RUSTDOCFLAGS=-D warnings
cargo run -p alloc-probe --release --locked -- compare 500 12
cargo test -p rust-webx --test stability_chaos_test -- --ignored --test-threads=1
cargo test -p rust-webx --test fuzz_lite_test
WIRE_P99_BUDGET_MS=5 cargo test --release -p rust-webx --test wire_p99_test -- --ignored
SOAK_SECONDS=300 cargo test -p rust-webx --test stability_soak_test -- --ignored
cargo audit                                    # RustSec
cargo semver-checks --package <core|engine|host|webx> --baseline-rev v2.1.0
cargo build --manifest-path compat/upgrade-smoke/{stable,current}/Cargo.toml
cargo test -p rust-webx-contract-tests
cargo test -p imbit --tests                     # IM 参考应用端到端
```

## 0.1 CI 等价门禁复核（2026-11-21，本地 Windows）

| 门禁 | 命令 | 结果 |
|---|---|---|
| 全量测试 | `cargo test --workspace --no-fail-fast` | **EXIT=0**（全部 test binary 0 failed；含 core 180 / host 51 / engine / webx / docbit / dmbit / imbit / contract-tests） |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | EXIT=0 |
| 格式 | `cargo fmt --all --check` | 0 diff |
| Rustdoc | `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps --locked` | EXIT=0（修复 4 处失效/冗余文档链接） |
| MSRV 1.95 | `cargo +1.95.0 check --workspace --locked` | EXIT=0 |
| SemVer | `cargo semver-checks --package <core|engine|host|webx> --baseline-rev v2.1.0` | **4/4** `no semver update required` |
| N-2 升级 | `cargo build --manifest-path compat/upgrade-smoke/{stable,current}/Cargo.toml` | 2.0.0 与当前 workspace 均 EXIT=0 |
| 分配 | `cargo run -p alloc-probe --release --locked -- compare 500 12` | framework=16.42 / baseline=6.00 / **tax=10.42 ≤ 12** |
| 安全 | `cargo audit --no-fetch` | **0 vulnerabilities / 0 warnings** |
| 稳定性（nightly 集） | chaos 5 场景 / fuzz-lite / wire p99 / soak | chaos 5 passed；fuzz-lite passed；release p99 **165µs ≤ 5ms**；soak 短时通过（RSS 断言） |

> 过程说明：本地复核时曾因 E: 盘 0 字节可用导致 link.exe 失败（LNK1140/LNK1285，损坏
> PDB）；清理 semver 缓存/升级烟测 target/incremental 与损坏 PDB 后重跑全绿——属于开发机
> 磁盘环境问题，不是代码回归。

## 0.2 当前工作树补充（2026-11-21 晚）

上表复核完成后，并行工作线合入了 2.5 starter 适配器初版（`crates/redis`、`crates/nats`，
`add_redis`/`add_nats` 形态）；随后复跑 `cargo check --workspace` 与
`cargo fmt --all --check` 均通过。本文件记录的 2.2/2.3/2.4 门禁结果对应三条功能线交付
时的树状态；2.5 适配器门禁由其自身的 2.5-7 台账条目跟踪。

## 1. v2.2「运行信任」

| 交付 | 证据 |
|---|---|
| 观测 RED + 直方图 | `http_requests_total`/inflight/status 指标 + 延迟直方图（`crates/host/src/metrics.rs`、`metrics_status_test`、`request_timeout_test`） |
| Error 2.0 | `ProblemDetails` 字段/错误映射（`problem_details_test`、`api_stability_test`；分页删除类型审计） |
| 过载保护 | 全局准入信号量 + `503` 卸载（`chaos_connection_storm_is_shed_with_503`） |
| 分配预算 | `alloc-probe compare 500 12`：framework tax **10.42**（2.2 出口按当时 ≤18 口径，2.4-9 后收紧到 ≤12） |
| 宏/配置诊断 | trybuild UI 快照、路由编译期校验、配置 section 绑定与 `APP__` 覆盖测试 |
| Cookie + 连接票据 | Cookie 认证/会话（`websocket_session_test`、`session_revocation_test`）；WS 升级鉴权走同一认证管线 |
| 参考实现合规 | docbit/dmbit 参考应用（`cargo test -p docbit-hosts` 等随 workspace 全量） |
| CI 门禁 | tests/lint/fmt/MSRV/audit/docs/coverage/benches/alloc/semver/upgrade 全绿（见 `ci.yml`） |

## 2. v2.3「实时执行模型」

| 交付 | 证据 |
|---|---|
| WS 执行模型 | 引擎 `serve_connection_with_upgrades` + `websocket_session_test`（回显/426/作用域释放） |
| 连接作用域 | 每连接 DI scope + `RequestContext::connection()`；`websocket_message_request_test` |
| 消息即请求 | `use_websocket_request`（Mediator 同管线、结构化错误帧、帧绑定路径）——`websocket_message_request_test` 2 用例 |
| `IConnectionRegistry` / 跨节点扇出 | 扇出/presence TTL：`realtime_registry_test`；跨节点扇出走事件传输：`realtime_fanout_test`（移除重复的 `IPubSub` 端口） |
| 背压与连接级限流 | 有界出站队列 + Drop/Disconnect + 1s 滑窗：`websocket_limits_test` 3 用例 |
| IM 参考应用端到端 | `imbit/`：登录/建房/消息/幂等重试/重连补拉/presence——`imbit/tests/e2e_im_test.rs` 2 用例 |

## 3. v2.4「媒体控制面与稳定收官」

| 交付 | 证据 |
|---|---|
| 信令/房间/TURN 凭据 | `webx::SignalingMessage` + `TurnRestConfig`（coturn REST 已知向量测试）；TURN 配置走 `AppOptions.turn` 并在 Host 启动期校验（`turn_config_test` 4 项）；`imbit/tests/signaling_e2e_test.rs`（1:1 offer/answer、ICE 广播、非成员 forbidden、TURN 端点） |
| 录制元数据 + 对象存储 | `IObjectStore`（内存/文件系统两实现、分片续传）+ `RecordingMeta`；`object_store_contract`；`imbit/tests/recording_e2e_test.rs`（上传/重试不膨胀/回放/非成员 403） |
| 媒体面适配契约 | `IMediaPlaneAdapter` + `media_*` QoS 摄入；`media_plane_contract`；`crates/core/src/media.rs` 契约测试 |
| 中间件统一 `Next` | `NextStage` + 默认桥接 + `TimeoutMiddleware`；`crates/webx/tests/middleware_next_test.rs` 4 用例 |
| TestHost / 可控时钟 / chaos 钩子 | `webx::TestHost` + `IClock`/`TestClock` + `ChaosControl`；`crates/webx/tests/test_host_test.rs` 5 用例 |
| 语义稳定与 LTS | `compatibility.md`（稳定面/弃用窗口/LTS/例外流程）、`deprecations.md`、`upgrade-to-2.4.md`；N-2 冒烟（2.0.0 + 当前）双通过；semver 4/4 |
| 稳定性门禁 | chaos 5 场景、线路 fuzz-lite（240 输入零 panic）、wire p99（release p99 **165µs** ≤ 5ms）、短时 soak（RSS 断言）、`cargo audit` **0/0**、alloc tax **10.42 ≤ 12** |
| Starter 契约脚手架 | `crates/contract-tests` 2 套件（object-store/media-plane），内存参考实现通过；四个 broker 适配器 env-gated 集成测试已接入 nightly `broker-integration`（redis/nats/rabbitmq/kafka）；见 [`starters.md`](starters.md) |
| 事件传输契约（2.5） | `crates/contract-tests` 的 `EventTransportContract`（PR CI loopback 自测 2 项通过）+ 四适配器 `tests/integration.rs` 接入同一入口；`broker-integration` nightly job + `docker-compose.brokers.yml` |
| 架构复核（单一执行模型/去重复） | ADR [`mediator-handler-architecture.md`](mediator-handler-architecture.md)；移除重复 `IPubSub`、跨节点扇出统一走事件传输（`realtime_fanout_test`）、`IMeter` 增量镜像（单测 + `realtime_metrics_test`）、测试基座合并；分层/重复抽象/事件语义/公开 API 核验记录见 ADR §9 |

## 4. 已知偏差与后续

| 项 | 状态 |
|---|---|
| 2.2 分配预算"总分配 ≤12" | hyper 地板实测 6.0，改为按**框架税**计量；2.2 出口 ≤18、2.4 收紧到 ≤12（实测 10.42） |
| 2.4 `/hello` ≤8 allocs 拉伸目标 | 未达（10.42）；剩余路径（RespBody 枚举化、poll ABI、按值交接）排入 2.5/3.0 |
| starter 真实 SDK 适配器 | 契约脚手架已交付；真实 Redis/NATS/S3/OTel/Postgres 适配器排入 2.5（nightly service container） |
| rust-ef `webx` feature 仅支持 1.x | 参考应用保留 `map_ef()` 适配器，等上游发布 2.x 后移除（`compatibility.md` §5） |
