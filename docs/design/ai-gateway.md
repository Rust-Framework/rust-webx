# 基于 rust-webx 的 AI 大模型中转聚合服务：框架能力补齐规划

> 状态：**提案（未冻结）**，供框架维护者与网关开发者评审。
> 基线：本仓库 `crates/`（workspace 1.4.0）源码；P0–P2 框架能力已按本文落地。
> 定位：本文回答"要做一个 OpenAI 兼容的模型聚合网关，webx 还缺什么、按什么顺序补、补完怎么让业务代码变简单"。

---

## 0. 结论摘要

1. **缺口不在 CRUD，而在"流 + 外呼 + 计量"**。路由、绑定、身份、授权、EF、后台任务、健康检查足以支撑控制面；但数据面缺三块地基：**可取消的流式响应**、**可流式读取的上游 HTTP 客户端**、**按 token/并发计量的配额与结算**。
2. **通用原语进框架，AI 语义进领域包**。SSE、出站客户端、认证 scheme、限流/计量端口、错误写入器、指标与追踪端口、热配置属于 Web 平台能力，应进 `webx`；provider 适配器、统一 IR、模型路由、usage/计费、OpenAI 表面属于领域能力，应进可选的 `rust-webx-ai`。**绝不把 `openai`/`anthropic` 字样写进 core。**
3. **流式必须是一等公民**。当前只能用 `FileResult::stream(AsyncRead)` 凑出 SSE：没有 `SseResult`，没有断连/完成回调，没有 `Cache-Control: no-store` 与"禁止压缩"保护，没有长流停机策略。框架应把"流"提升为与 JSON、文件并列的响应形态。
4. **可靠性规则要写进抽象，而不是靠记忆**。只允许在**未产生首字节**前重试/切换候选；流一旦开始，重试等于重复输出与重复计费。这个约束应由 `RoutePlan` 与执行器表达，并固化成框架文档与审计规则。
5. **计量必须"预扣-结算"**。入口按估算预扣配额与并发槽；流结束（完成 / 客户端断开 / 上游失败）统一结算。否则超卖、漏计费、断连坏账必然发生。
6. **性能目标是可量化的**：代理开销 p99 ≤ 3ms、TTFT 增量 p99 ≤ 5ms、单节点 ≥ 2 万条并发 SSE、热路径零数据库读、零全量缓冲、每流内存 O(1)。

**一句话**：把"流式传输、出站调用、凭据与配额、计量结算、可观测"做成框架原生能力；把"provider 协议差异"收敛到领域包的一层适配器；业务项目只剩下"配置 + 定价 + 租户模型"。

### 0.1 落地进度（滚动更新）

| ID | 能力 | 状态 | 证据 |
|---|---|---|---|
| G1 | 流式响应/SSE | ✅ 已实现（流式响应 + 测试 + 手册；审计规则/发布待接入） | `crates/core/src/stream.rs`、`crates/webx/tests/sse_test.rs`、手册 `05-request-pattern/streaming-sse.md` |
| G2 | 出站 HTTP 客户端 | ✅ 已实现（`rust-webx-client`：池化、流式、SSE 解码、超时、显式 `CONNECT` 代理、企业 CA、hop-by-hop 剥离、`traceparent` 注入、SSRF 私网/元数据守卫；`HostBuilder::add_http_client` 注册 registry/factory） | `crates/client/src/{client,registry,sse}.rs`、`crates/client/tests/http_client_test.rs`、`crates/webx/tests/http_client_wiring_test.rs` |
| G3 | 认证 scheme + API Key | ✅ scheme 链 + `ApiKeyScheme`（含 `with_failure_limit` 失败限流 429）+ `IApiKeyStore`/`MemoryApiKeyStore`/`EfApiKeyStore`（`rust-webx-ai-ef`，`ai_api_keys`）+ `CachedApiKeyStore`（TTL、`invalidate`）+ `StoreApiKeyValidator`（HMAC、吊销/过期即时生效） | `crates/host/src/auth.rs`、`crates/ai/src/{api_key,api_key_store}.rs`、`crates/ai-ef/src/api_key.rs`、`crates/gateway/src/auth.rs` |
| G4 | 配额预扣-结算 | ✅ 已实现 `IQuotaStore`/`MemoryQuotaStore`/`StaticQuotaPolicy`（RPM/TPM/TPD/并发/预算、租约、幂等结算）；Redis/分布式实现待补 | `crates/ai/src/quota.rs` |
| G5 | 计量与计费 | ✅ `Usage`/`UsageEvent`/`IUsageSink`/`MemoryUsageSink`/`ITokenizer`/`HeuristicTokenizer`/`rollup_events` + `rust-webx-ai-ef` 的 `EfUsageSink`/outbox/rollup + `UsageOutboxWorker`/`UsageRollupWorker`（`PeriodicBackgroundService`） | `crates/ai/src/usage.rs`、`crates/ai-ef/src/{lib,worker}.rs` |
| G6 | 模型目录与路由 | ✅ 已实现 `ModelCatalog`/`ModelSpec`/`Alias`/`CatalogRouter`/`HealthRegistry` + `GatewayExecutor`（首字节规则、重试、配额/计量接线）；租户级路由待补（热替换见 G13） | `crates/ai/src/{provider,routing,executor}.rs` |
| G7 | 协议适配 | ✅ IR + `ChatProvider` 端口 + OpenAI 兼容适配器 + `rust-webx-ai-gateway` 的 `/v1/chat/completions`（JSON/SSE）与 `/v1/models`；Anthropic/Gemini 转译 lane、SDK conformance 待补 | `crates/ai/src/{ir,provider,openai}.rs`、`crates/gateway` |
| G8 | 错误模型/OpenAI 信封 | ✅ 已实现 `IErrorResponseWriter` + `HostBuilder::add_error_writer` + `OpenAiErrorWriter`（404/绑定/鉴权都覆盖）；1.x 仍用 `Error::Status(502/504)`，typed 变体留 2.0 | `crates/core/src/error.rs`、`crates/host/src/server.rs`、`crates/ai/src/error.rs`、`crates/webx/tests/error_writer_test.rs` |
| G9 | 韧性 | ✅ `RetryPolicy`（指数退避 + jitter + `Retry-After`）、`CircuitBreaker`、`Bulkhead`，已接执行器；入站 deadline（`Server.RequestTimeoutSeconds` → 504）；`GatewayHealth` 熔断/冷却进 `/health/ready`；对冲待补 | `crates/ai/src/resilience.rs`、`crates/ai/src/executor.rs`、`crates/gateway/src/health.rs`、`crates/host/src/server.rs` |
| G10 | 凭据/密钥 | ✅ `ISecretStore` + `SecretValue`（Debug 脱敏、Drop zeroize）+ `SecretRef` + `EnvSecretStore`/`FileSecretStore`（0600、拒绝符号链接/穿越）/`ChainSecretStore`，`OpenAiProvider::from_secret_store` 已接；KMS/信封加密、轮换与 key pool 待补 | `crates/core/src/secrets.rs`、`crates/ai/src/openai.rs`、`docs/rust-webx/09-auth-security/secret-store.md` |
| G11 | 缓存与单飞 | ✅ 精确缓存键（tenant+model+完整请求、`temperature==0`、非流）+ `MemoryGatewayCache` + 单飞（并发同键共享一次上游）+ 命中/加入指标；Redis/SSE 重放待补 | `crates/ai/src/cache.rs`、`crates/ai/src/executor.rs` |
| G12 | 可观测 | ✅ core `IMeter`/`NoopMeter`/`MemoryMeter` + 执行器请求/TTFT/token/成本/上游错误/配额/active streams/熔断状态指标 + 结构化日志 + W3C `traceparent` 采纳/出站注入 + `GatewayHealth` 就绪聚合；上游子 span/OTel exporter/`tracestate` 待补 | `crates/core/src/metrics.rs`、`crates/ai/src/{executor,cost}.rs`、`crates/gateway/src/health.rs`、`crates/host/src/request_tracing.rs`、`crates/client/src/client.rs` |
| G13 | 动态配置 | ✅ `CatalogHandle`（`ArcSwap`）+ `CatalogRouter::from_handle` + `GatewayExecutor::new_with_handle`；版本回退拒绝；`IOptionsMonitor<T>` + `HostBuilder::add_options_monitor` / `watch_options_file`；`CatalogFileWatcher` + `CatalogManifest`（models/aliases 文件热加载、校验失败保留旧快照）；多实例版本轮询/canary/配额价格 key 轮换待补 | `crates/ai/src/{provider,routing,executor,catalog_file}.rs`、`crates/core/src/options_monitor.rs`、`crates/host/src/options_watch.rs` |
| G15 | 长流停机 | ✅ `Server.DrainTimeoutSeconds` / `StreamDrainTimeoutSeconds` 分离；连接级 SSE 活动跟踪选择预算；`ShutdownToken` + `SseResult::on_server_shutdown` 发送 `event:server_shutdown` 后正常结束；后端 drain 协调待补 | `crates/core/src/config.rs`、`crates/core/src/{hosted,stream}.rs`、`crates/host/src/server.rs` |
| G17 | 成本模拟 | ✅ `GatewayExecutor::estimate` + `/v1/chat/completions` 的 `dry_run`（不调 provider/不预扣） | `crates/ai/src/executor.rs`、`crates/gateway/src/handler.rs` |
| G16 | 测试工具链 | ✅ `MockUpstream`（JSON/SSE/status/延迟/请求录制）+ 文件式 `ReplayFixture`（版本协商、敏感头默认脱敏、原始保存）+ OpenAI 适配器真实 HTTP/SSE e2e + SSE 任意 chunk 边界回归；官方 SDK conformance、真实 fuzz/压测待补 | `crates/client/src/{testing,sse}.rs`、`crates/ai/tests/openai_provider_test.rs` |
| G14 | 请求体流式 | ✅ 入站 `body_stream`（默认回退 + host 实时流 + 413）与出站 `OutboundRequest::body_stream`（boxed hyper body）；`#[max_body_size(..)]` 按路由覆盖非 multipart 上限；出站 chunked 策略/背压调优待补 | `crates/core/src/{http,route/scan}.rs`、`crates/macros/src/endpoint.rs`、`crates/host/src/{context,endpoint}.rs`、`crates/client/src/client.rs` |

`rust-webx-ai` 已作为 workspace 成员建立（`crates/ai`，`version.workspace = true`），发布脚本与
workflow 已把它纳入依赖顺序；它永远与 `rust-webx` 使用同一个版本号。OpenAI 兼容 HTTP 表面放在
独立的 `rust-webx-ai-gateway`（`crates/gateway`），持久化用量账本放在 `rust-webx-ai-ef`
（`crates/ai-ef`），避免领域包反向依赖 host/hyper。P0 的 `/v1/chat/completions` 已能端到端跑通
（JWT/API key → 执行器 → provider → JSON/SSE），且可用 EF 账本落 usage。

本仓库门禁：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo test`、`bash scripts/check-docs.sh`。`rust_xazgo_audit` 的默认扫描目标是 Travel 仓库的
`server/`（本仓库没有该目录），其 SQL 迁移告警与本改动无关。

---

## 1. 业务形态与第一性约束

### 1.1 三类平面

| 平面 | 职责 | 典型端点 | 对框架的压力 |
|---|---|---|---|
| **数据面** | 接收 OpenAI 兼容请求，选路，转发，流转发/转译，计量 | `POST /v1/chat/completions`、`/v1/embeddings`、`/v1/models` | 长连接、上游依赖、首字节延迟、断连 |
| **控制面** | provider / 模型 / 密钥 / 别名 / 配额策略的 CRUD 与生效 | `/api/providers`、`/api/models`、`/api/keys` | 普通 CRUD，EF 足够 |
| **经济面** | 用量事件、成本、账单、预算、对账、报表 | `/api/usage`、`/api/budgets` | 追加写、幂等、批量聚合、异步结算 |

三者的事务边界、一致性要求、延迟预算完全不同，**不能揉进一个"万能 Service"**。

### 1.2 与普通 CRUD 服务的六个本质差异

| # | 差异 | 后果 |
|---|---|---|
| 1 | 响应是**长时流**（秒级到分钟级），不是一次性 JSON | 需要流式写、flush、心跳、断连感知、可取消 |
| 2 | 每次请求都**依赖外部上游**，上游会 429/5xx/超时/半途断开 | 需要连接池、超时预算、重试/熔断/切换，且**只能首字节前重试** |
| 3 | 计费单位是 **token/秒/并发**，不是"请求数" | 需要多维配额、预扣-结算、usage 归一、断连口径 |
| 4 | 请求体是**厂商协议**，字段随版本漂移 | 需要"未知字段透传"、能力声明、协议适配层 |
| 5 | 失败会**产生真实成本**（上游已计费） | 需要幂等、结算状态机、对账，而不是简单 500 |
| 6 | 客户端可能是官方 SDK，**错误/流格式必须兼容** | 需要 OpenAI 形状的错误信封与 SSE 帧，而非 problem+json |

### 1.3 从差异推导能力要求

| 差异 | 框架层能力 | 领域层能力 |
|---|---|---|
| 长时流 | `SseResult`、断连钩子、心跳、停机排空、`no-store`/禁压缩 | 流事件映射、usage 提取 |
| 外部上游 | 命名 HTTP 客户端、连接池、超时、重试、熔断、traceparent 注入 | provider 适配器、候选切换策略 |
| token 计费 | 配额端口（分区键 + 权重 + 预扣/结算）、tokenizer 端口 | 维度定义、价格表、账本、rollup |
| 协议漂移 | 未知字段透传（`#[serde(flatten)]`）、按路由错误写入器 | 统一 IR、能力矩阵、版本化 fixture |
| 真实成本 | 幂等键、outbox、typed 上游错误 | 结算状态机、对账、预算 |
| SDK 兼容 | 可插拔错误格式、SSE 帧编码 | OpenAI 表面与错误码映射 |

---

## 2. 现状盘点（带证据）

> 只列与聚合网关直接相关的能力；"够用"指控制面，不代表数据面够用。
> **本节是提案时的基线快照**；已落地/剩余项以 §0.1 进度表和各能力小节为准。

| 能力 | 现状 | 证据 | 判定 |
|---|---|---|---|
| 路由 | matchit 静态路由，编译期扫描注册；无通用 fallback（SPA 除外） | `crates/host/src/router.rs`、`crates/core/src/route/scan.rs` | 够用：OpenAI 表面是固定路径 |
| 请求绑定 | `#[frombody]` + `#[serde(flatten)]` 可捕获未知字段；全量缓冲 | `crates/macros/src/request_meta.rs`、`crates/host/src/context.rs` | 够用（聊天请求 <1MB）；大请求需流式体 |
| 流式响应 | 仅 `FileResult::stream(AsyncRead)` → chunked；无 SSE 抽象 | `crates/core/src/action.rs`、`crates/host/src/context.rs` | **缺**：能跑 demo，不能产品化 |
| 响应默认头 | 文件流默认 `Cache-Control: public, max-age=0, must-revalidate` | `crates/host/src/context.rs` `default_cache_control` | **风险**：SSE 必须显式改 `no-store` |
| 压缩 | 只压字节体，排除 image/video/audio；未排除 `text/event-stream` | `crates/host/src/compression.rs` | 小缺口：加一条排除即可 |
| 请求体 | `body_bytes` 全量读入，`App.MaxBodySize` 默认 10MB | `crates/core/src/http.rs`、`crates/host/src/context.rs` | 大 batch/audio 不可代理（P1） |
| 出站 HTTP | **没有**客户端；`reqwest` 仅 dev-dependency | `crates/host/Cargo.toml`、`crates/webx/Cargo.toml` | **缺 P0** |
| 韧性 | 无重试/熔断/舱壁/超时策略 | 全仓库 | **缺 P0/P1** |
| 认证 | 只有 JWT（`add_authentication`）；`IAuthenticationHandler` 存在但无 scheme 组合 | `crates/host/src/auth_jwt.rs`、`crates/host/src/server.rs` | **缺**：无法做 `sk-` 数据面密钥 |
| 限流 | 全局按 IP、请求数 token bucket、进程内 HashMap | `crates/host/src/rate_limit.rs` | **缺**：不能按 key/模型/token 计量 |
| 配额/计费 | 无 | — | **缺 P0** |
| 模型目录/路由 | 无 | — | **缺 P0** |
| 协议适配 | 无 | — | **缺 P0** |
| 错误模型 | 无 502/504 变体；错误响应全局 problem+json | `crates/core/src/error.rs`、`crates/host/src/server.rs` | **缺**：OpenAI SDK 不兼容、无法表达可重试 |
| 凭据 | 仅 `Jwt.SecretFile` | `crates/core/src/config.rs` | **缺**：上游 key 无统一存储/轮换 |
| 缓存 | `IDistributedCache` 抽象 + `MemoryCache` | `crates/core/src/cache/*` | 基础够；缺响应缓存/单飞 |
| 可观测 | 4 个全局计数器 + `x-trace-id` | `crates/host/src/metrics.rs`、`request_tracing.rs` | **缺**：无标签/直方图/TTFT/上游维度 |
| 配置 | 启动期 `AppOptions` + `add_options`，无热更新 | `crates/core/src/config.rs` | **缺**：改 provider/key 要重启 |
| 优雅停机 | 固定 30s 排空（开发 5s），长流会被中断 | `crates/host/src/server.rs` `drain_connections` | **缺**：SSE 需独立预算 |
| 网络细节 | 未设置 `TCP_NODELAY` | `crates/host/src/server.rs` | **缺**：SSE 小包延迟可达 40ms |
| 测试 | `testing` feature：free_port/spawn | `crates/host/src/testing.rs` | 缺 mock 上游/SSE fixture |

**结论**：webx 是一个**优秀的请求/响应型 WebApi 框架**，而聚合网关是一个**长连接代理 + 计量系统**。两者重叠的部分（路由、DI、EF、后台任务）可以直接复用；不重叠的部分必须补，且不能靠业务项目各自补。

---

## 3. 能力差距矩阵与优先级

优先级定义：**P0** = 没有它就无法做出可上线的 MVP；**P1** = 生产级必备（稳定性/运维/成本）；**P2** = 行业领先的差异化。

| ID | 能力 | 缺口 | 影响 | 优先级 | 落位 |
|---|---|---|---|---|---|
| G1 | 流式响应/SSE | `SseResult`、断连/完成钩子、心跳、`no-store`、禁压缩、停机预算 | 无法稳定代理流 | P0 | core + host |
| G2 | 出站 HTTP 客户端 | 命名客户端、连接池、HTTP/2、超时、代理、流式响应、traceparent | 无法调用上游 | P0 | 新 crate `client` |
| G3 | 认证 scheme + API Key | scheme 组合、`sk-` 校验、前缀索引、HMAC、常量时间 | 无法发数据面密钥 | P0 | core + host |
| G4 | 配额（预扣/结算） | 多维、分区键、权重、分布式、429 语义 | 超卖/漏计费 | P0 | core 端口 + ai 实现 |
| G5 | 计量/计费 | Usage、tokenizer、幂等结算、outbox、rollup | 无法计费对账 | P0 | ai |
| G6 | 模型目录/路由 | provider/model registry、别名、优先级/权重/兜底、健康 | 每家自写路由 | P0 | ai |
| G7 | 协议适配 | 统一 IR、流事件映射、未知字段透传、能力矩阵 | 每厂商一套转换 | P0 | ai |
| G8 | 错误模型 | 502/504、可重试分类、OpenAI 信封、按路由错误写入器 | SDK 不兼容 | P0 | core + host |
| G9 | 韧性 | 重试（首字节前）、熔断、舱壁、对冲、deadline | 上游故障放大 | P1 | client |
| G10 | 凭据/密钥 | `ISecretStore`、信封加密、轮换、key pool 健康 | 密钥散落/难轮换 | P1 | core + ai |
| G11 | 缓存/单飞 | 精确缓存、流式重放、single-flight | 重复请求打满上游 | P1 | ai（复用 cache） |
| G12 | 可观测 2.0 | 带标签直方图、TTFT、token 吞吐、上游维度、OTel | 无法定位/定价 | P1 | core + host |
| G13 | 动态配置 | `IOptionsMonitor`、目录快照（ArcSwap）、变更通知 | 改配置要重启 | P1 | core |
| G14 | 请求体流式 | 大 batch/audio 透传、按路由上限 | 大请求不可代理 | P1 | core + host |
| G15 | 长流停机 | 可配置 drain、流最大生命周期、结算 | 发布截断/坏账 | P1 | host |
| G16 | 测试工具链 | mock provider、SSE fixture、record/replay、conformance | 上游不可控 | P1 | testing |
| G17 | 自适应路由 | 延迟/成本/成功率自适应、p99 对冲 | 同质化 | P2 | ai |
| G18 | 语义缓存 | 向量相似命中、租户隔离、误答风控 | 成本优势 | P2 | ai |
| G19 | Batch/异步任务 | 文件上传 + job 状态机 + worker | Batch API | P2 | ai |
| G20 | Realtime/WebSocket | WS 升级、双向流转发 | 语音实时 | P2 | host + ai |

---

## 4. 架构原则与决策记录

### 4.1 八条原则

1. **关注点分离**：传输（webx）与 AI 语义（`rust-webx-ai`）分离；core 不出现任何 provider 名称。
2. **端口与适配器**：框架声明端口（`IHttpClient`、`IAuthenticationScheme`、`IQuotaStore`、`ISecretStore`、`IMeter`），默认实现可替换；业务只依赖端口。
3. **流是一等公民**：与 JSON/文件并列的结果类型；任何"先缓冲再发"的路径在审计里视为缺陷。
4. **首字节规则**：未产生首字节才允许重试/切换；该规则由执行器与文档共同固化。
5. **预扣-结算**：所有计量资源（token、并发、预算）先预留后结算；结算幂等。
6. **热路径零控制面 IO**：配额/密钥/目录走内存快照（`ArcSwap`/LRU），数据库只用于写与冷启动。
7. **单一写法**：一种流式结果、一种上游客户端、一种错误映射、一种用量事件；禁止每个业务模块各自实现。
8. **可演进**：所有框架增量**加性**（additive），不改变既有语义；新能力用 feature/新 crate；破坏性变更留到 2.0。

### 4.2 ADR（Architecture Decision Records）

| ADR | 决策 | 理由 | 代价 |
|---|---|---|---|
| ADR-1 | AI 领域包独立于 core，但作为同一 workspace 成员，`version.workspace = true` 跟随框架版本 | 避免把厂商协议塞进 Web 框架；webx-ai 与框架同版本发布，避免兼容矩阵 | 多一个 crate；发布顺序需纳入 publish 脚本 |
| ADR-2 | 出站客户端基于 `hyper-util` + `hyper-rustls`，不引入第二套 HTTP 栈 | 复用 hyper，连接池/流式/TLS 一致；体积可控 | 比 reqwest 多写少量胶水 |
| ADR-3 | 1.x 的 `SseResult` 建在 `FileBody::Stream` 之上；2.0 再引入 `ResponseBody::Stream`（`Frame` 流） | 不改公开 enum，避免 1.x 破坏性变更 | 多一层 `AsyncRead` 适配 |
| ADR-4 | 认证改为"scheme 链 + 选择器"，JWT 只是其中一种 | 数据面 `sk-` 与管理面 JWT 共存 | 认证装配 API 变复杂 |
| ADR-5 | 配额端口使用 reserve/settle，语义在 ai 包 | 流式必须两阶段；框架只提供通用并发/计数原语 | 需要租约与超时回收 |
| ADR-6 | 用量事件 append-only + outbox，`request_id` 唯一 | 幂等、可重放、可对账 | 需要 worker 与去重 |
| ADR-7 | 统一 IR 采用**中立内容块模型**，OpenAI 仅是一种 codec | 保留 Anthropic/Gemini 能力；未知字段可透传 | IR 设计成本 |
| ADR-8 | 流式分两条 lane：**透传 lane**（OpenAI 兼容）与**转译 lane** | 透传零重排、最低 TTFT；转译才付 CPU | 两条路径要各自测试 |
| ADR-9 | 错误响应可插拔（按路由前缀选择 writer） | `/v1/*` 用 OpenAI 信封，`/api/*` 用 problem+json | 多一个扩展点 |
| ADR-10 | 目录与配置走 `ArcSwap` 快照 + 版本号 | 热路径无锁、无 DB；多实例可轮询/通知 | 最终一致（秒级） |

### 4.3 什么该进框架（准入判据）

| 判据 | 说明 | 反例 |
|---|---|---|
| 跨产品重复 | 至少两个真实产品会各写一遍 | 某家 provider 的特有参数 |
| 需要框架内部状态 | 必须触碰连接、响应生命周期、路由元数据 | 纯业务定价表 |
| 正确性/安全关键 | 写错会导致越权、漏计费、协议不兼容 | 某个运营报表 |
| 有稳定契约 | 可定义端口 + ≥2 个实现 | 一次性迁移脚本 |
| 不引入领域词 | API 里不出现厂商/模型概念 | `OpenAiResult` 进 core |

不满足以上任一条的，留在应用或 `rust-webx-ai`，不要进 core。

---

## 5. 目标架构

### 5.1 分层与 crate

```text
┌────────────────────────────────────────────────────────────────┐
│ 应用（网关产品）                                                │
│  - 定价表、租户模型、运营策略、管理 UI                          │
└───────────────▲────────────────────────────────────────────────┘
                │ 依赖
┌───────────────┴────────────────────────────────────────────────┐
│ rust-webx-ai（领域包，版本跟随 workspace，可选）                 │
│  surface(OpenAI) │ provider 适配器 │ IR/流事件 │ 路由/目录      │
│  quota 维度 │ usage/账本/rollup │ key store │ admin             │
└───────────────▲────────────────────────────────────────────────┘
                │ 依赖端口
┌───────────────┴──────────────┐   ┌─────────────────────────────┐
│ rust-webx（umbrella）         │   │ rust-webx-client（新）      │
│  core: 流/认证/端口/错误      │◄──│ 出站 HTTP、连接池、韧性     │
│  host: hyper、SSE 传输、指标  │   │ 超时/重试/熔断/代理         │
└───────────────▲──────────────┘   └─────────────────────────────┘
                │
┌───────────────┴────────────────────────────────────────────────┐
│ rust-dix（DI） / rust-ef（控制面持久化，可选）                  │
└────────────────────────────────────────────────────────────────┘
```

依赖规则：

- `core` 零 HTTP 引擎依赖（保持现状），只加 **trait + 纯数据类型**。
- `client` 依赖 `core`，不依赖 `host`（可独立用于 worker/CLI）。
- `ai` 依赖 `webx`（+ 可选 `rust-ef` 适配模块），**不反向**。
- 应用只依赖 `webx`、`rust-webx-ai` 与（需要出站调用时）`rust-webx-client`，不直接依赖 hyper。

### 5.2 数据面请求生命周期

```text
client ──POST /v1/chat/completions──► host
  1. RequestId / Trace / CORS / 安全头
  2. AuthScheme 选择：Bearer sk-… → ApiKeyScheme（缓存命中，HMAC 校验）
  3. 路由匹配 → 已知 route_pattern
  4. 绑定 DTO（未知字段进 extensions）
  5. GatewayService（typed pipeline 或 handler）：
     a. 解析 model 别名 → RoutePlan（候选 + 截止时间）
     b. 配额 reserve（RPM/TPM/并发/预算）
     c. 选 provider attempt（健康/冷却/权重）
     d. 出站 client.execute（连接池、超时、traceparent）
     e. 首字节前失败 → 下一个候选；首字节后 → 不再重试
  6. 响应：
     - 非流：归一 JSON → 释放/结算 → 200
     - 流：SseResult(StreamEvent → OpenAI chunk) → 完成/断连钩子 → settle
  7. 计量：UsageEvent(request_id 唯一) → outbox → rollup worker
  8. 指标/日志：TTFT、首字节、token、成本、provider、attempt
```

### 5.3 控制面与经济面

- 控制面写：EF + `use_transaction`；写后 bump `catalog_version`。
- 其他实例：轮询版本（5s）或 Redis pub/sub 通知 → `ArcSwap` 换快照。
- 经济面：`usage_events` 追加写 → 批量 rollup → 预算检查；对账任务比对上游账单。
- 管理面端点：标准 CRUD + `OperationResult`；无需新框架能力。

### 5.4 为什么是 webx（差异化定位）

- **单二进制 + 无 GC**：聚合网关的尾延迟与内存可预测；相比 Python 网关省掉解释器与 GC 抖动。
- **编译期路由与 DI**：路由表、Handler、依赖在构建期确定；没有运行期反射/插件加载的启动成本。
- **严格的 scope 与显式生命周期**：长连接场景下，"谁持有连接/流/结算"必须可推理。
- **EF 覆盖控制面**：provider/key/价格/预算的 CRUD 与迁移不需要第二套 ORM。
- **hyper 原生流式**：不经过额外代理层；背压、取消、连接复用都能直达传输层。

**不声称**：webx 不是通用反向代理；任意协议转换、WASM 插件市场不在定位内。

---

## 6. P0 框架能力设计

### G1 流式响应与 SSE

**问题**：当前只能用文件流模拟 SSE，且默认 `Cache-Control` 与压缩行为对 SSE 是错的；没有断连/完成钩子，计费与取消无从下手。

**目标 API**（1.x 语义）：

```rust
// webx::stream（core；host 负责传输）
pub struct SseEvent {
    pub event: Option<String>,   // event:
    pub id: Option<String>,      // id:
    pub data: bytes::Bytes,      // data: 可多行
    pub retry: Option<u32>,      // retry:
}

pub struct SseResult { /* ... */ }

impl SseResult {
    pub fn new<S>(events: S) -> Self
    where S: futures_util::Stream<Item = Result<SseEvent>> + Send + 'static;

    /// 空闲时发送 `: ping`，防止中间层超时断连。
    pub fn keep_alive(self, every: Duration) -> Self;

    /// 流结束（完成/断连/失败/超时）回调；Drop 时保证恰好触发一次。
    pub fn on_close(self, hook: impl FnOnce(StreamEnd) + Send + 'static) -> Self;
}

pub enum StreamEnd {
    Completed,
    ClientDisconnected,
    Failed(webx::Error),
    Deadline,
}
```

Handler：

```rust
#[post("/v1/chat/completions")]
impl IRequest<Response> for ChatCompletionRequest {}

async fn handle(&mut self, req: ChatCompletionRequest) -> ActionResult<Response> {
    let plan = self.gateway.plan(&req).await?;
    let stream = self.gateway.execute_stream(plan, req).await?;
    Ok(Box::new(SseResult::new(stream).keep_alive(Duration::from_secs(15))))
}
```

**实现要点**：

- 1.x 用 `FileBody::stream` + `tokio_util::io::StreamReader` 桥接；不新增 `ResponseBody` 变体。core 已有 `bytes`/`tokio-util`，只需补 `futures-util`（`default-features = false`）。
- 完成钩子绑定在**流的包装器**上（随 `FileBody::Stream` 被 hyper 持有），而不是 `execute()` 时执行；因此客户端断开导致的 Drop 也能触发。
- 必须显式设置：`content-type: text/event-stream; charset=utf-8`、`cache-control: no-store`、`x-accel-buffering: no`；覆盖文件流的 `must-revalidate` 默认值。
- `CompressionMiddleware` 增加 `text/event-stream` 排除；SSE 永不 gzip。
- 断连检测：客户端断开时 hyper 丢弃 body 流 → `SseResult` 的流被 Drop。`Drop` 不能 `await`，用 `tokio::spawn` 或 mpsc 发到结算 worker（保证一次）。
- 心跳只发注释帧 `: ping\n\n`，不产生业务事件。
- 可选 `Last-Event-ID` 断点续传：仅在 provider 支持时透传。
- 传输层：`TCP_NODELAY` 默认开启；`Server.StreamDrainTimeoutSeconds` 默认 300；`SseResult::max_lifetime` 防止僵尸流。
- 审计规则新增：`WEBX-HANDROLLED-SSE`（手写 `text/event-stream` 字节拼装）、`WEBX-SSE-BUFFERED`（先 collect 再发）。

**性能**：每帧一个 `Bytes`；不做 UTF-8 重校验；心跳走独立 interval 分支；`Stream` 直接由 hyper 拉取，背压天然传导到上游。

**验收**：SDK 流式对话逐 token 可见；客户端 Ctrl+C 后上游请求在 100ms 内取消；断连结算一次；nginx 后无 60s 断流。

---

### G2 出站 HTTP 客户端（新 crate `rust-webx-client`）

**问题**：框架没有客户端，聚合服务无法调用上游；若每项目自选 reqwest/ureq，超时、池、TLS、代理、流式行为不一致。

**目标 API**（已实现，见 `rust-webx-client`）：

```rust
#[async_trait]
pub trait IHttpClient: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, request: OutboundRequest)
        -> Result<OutboundResponse, HttpClientError>;
}

pub struct OutboundResponse {
    pub status: u16,
    pub headers: http::HeaderMap,
    pub body: ResponseStream,
}

impl ResponseStream {
    /// 解码上游 SSE，保持逐帧背压；不做整段缓冲。
    pub fn sse(self) -> SseStream;
    /// 有上限地收集（错误体/非流响应）。
    pub async fn collect(self, limit: usize) -> Result<Bytes, HttpClientError>;
    /// 原始字节流（透传 lane）。
    pub fn bytes(self) -> impl Stream<Item = Result<Bytes, HttpClientError>>;
}
```

配置与注册（当前版本）：

```rust
let client = HttpClient::new("openai", HttpClientOptions {
    base_url: "https://api.openai.com".into(),
    connect_timeout: Duration::from_secs(3),
    request_timeout: Some(Duration::from_secs(120)), // 只约束响应头
    read_timeout: Some(Duration::from_secs(120)),    // 帧间空闲
    pool_max_idle_per_host: 64,
    http2: true,
    tcp_nodelay: true,
    ..HttpClientOptions::default()
})?;

let registry = HttpClientRegistry::new().with("openai", Arc::new(client));
Host::builder().register(move |svc| svc.instance(Arc::new(registry)))
```

**实现要点**：

- 基于 `hyper-util::client::legacy::Client` + `hyper-rustls`；不引入 reqwest。
- 每客户端独立连接池；`https` 走 rustls；`tcp_nodelay` 可配；HTTP/2 通过 ALPN，HTTP/1.1 仍可用。
- 响应体默认流式；`collect(limit)` 强制上限，防止恶意上游打爆内存。
- 错误分类：`Connect` / `Timeout` / `Protocol` / `Body` / `BodyTooLarge`；`is_connect()` 供 G9 判定可重试。
- **已补（P1）**：显式 `CONNECT` 代理（`proxy` + `proxy_authorization`）、企业 CA（`root_certificates`，
  PEM 包替换内置根、构造期 fail-fast）、hop-by-hop 过滤（含 `Connection` 头列出的字段）、
  `traceparent` 注入、`HostBuilder::add_http_client`（注册 `HttpClientRegistry` +
  `dyn IHttpClientFactory`）。
- 审计规则 `WEBX-HANDROLLED-HTTP` 待接入。

**验收**：本地 hyper 服务覆盖 JSON/回显/流式 SSE/请求超时/帧间读超时/体积上限/非 2xx；1k 并发复用率与代理/追踪在 P1 补测。

---

### G3 认证 scheme 与 API Key

**问题**：只有 JWT，无法给 SDK 发 `sk-` 密钥；`IAuthenticationHandler` 只能手工塞进中间件，不能与 JWT 共存/选择。

**已实现 API**（按凭证形状 first-match；路由前缀选择器留待后续）：

```rust
// core::auth
pub trait IAuthenticationScheme: Send + Sync {
    fn name(&self) -> &'static str;
    /// 只看凭证形状，不做 IO（快速选择）。
    fn matches(&self, ctx: &dyn IHttpContext) -> bool;
    async fn authenticate(&self, ctx: &mut dyn IHttpContext)
        -> Result<Option<Box<dyn IClaims>>>;
}
```

装配：

```rust
Host::builder()
    .add_authentication_scheme(Arc::new(ApiKeyScheme::new(validator))) // 数据面 sk-
    .add_authentication()                                             // 管理面 JWT，排在后面
```

选择规则：显式 scheme 先于 JWT；首个 `matches` 命中的 scheme 处理请求。JWT 只认
`a.b.c` 形状且验证失败按匿名继续；API key 命中但校验失败直接 401（不回显 key）。

**API Key 设计**：

```text
sk-{key_id}-{secret}
       │        └─ 32B CSPRNG，base62（不含 '-'；解析取最后一段）
       └─ 可含 '-'（UUID），库中只存明文 id
```

- `IApiKeyValidator`：按 presented key 查缓存/DB，返回 `ApiKeyClaims` 或 `None`；scheme 不碰存储。
- `webx_ai::api_key`：`parse_presented`（按最后一段拆 id/secret）、`hash_secret`（HMAC-SHA256）、
  `verify_secret`（常量时间比较）已实现。
- `webx_ai::api_key_store`：`IApiKeyStore`、`ApiKeyRecord`、`MemoryApiKeyStore`、
  `CachedApiKeyStore`（命中结果 TTL 缓存 + `invalidate`）已实现；
  `key_id` 唯一；`secret_hash = HMAC-SHA256(server_pepper, secret)`；**不要**用 Argon2。
- `rust-webx-ai-ef` 的 `EfApiKeyStore`（表 `ai_api_keys`）实现持久化读写（`get`/`upsert`/`revoke`）；
  secret 只存十六进制摘要，损坏摘要拒绝认证。生产组合：`EfApiKeyStore` + `CachedApiKeyStore`
  （TTL 即撤销窗口上限；本进程撤销需 `invalidate`）。
- `StoreApiKeyValidator`（`rust-webx-ai-gateway`）：解析 → 查 store → 校验状态/过期 → 常量时间比较
  → `ApiKeyClaims`；支持 pepper 轮换（`pepper_id` 存在记录上）。
- 撤销：`MemoryApiKeyStore::revoke` / `EfApiKeyStore::revoke` 后**下一请求**即失效（无缓存时）；
  缓存包装下由 `invalidate` 或 TTL 界定。认证失败限流：`ApiKeyScheme::with_failure_limit` 按
  `IP + key 前缀` 固定窗口，超限 `429` + `Retry-After`，成功即清空。
- Claims 映射：`sub = key_id`、`tenant_id`、`roles`、`permissions`、`pepper_id`；`IdentityUser` 复用。
- 认证失败限流：`with_failure_limit` 按 IP + key 前缀固定窗口；`WWW-Authenticate` 保持 RFC 6750。
- 审计规则：`WEBX-APIKEY-ARGON2`（热路径慢哈希）、`WEBX-APIKEY-RAW-STORE`（明文存 key）。

**验收**：Bearer / `x-api-key` 两种头通过；无效 key 401 且不回显；与 JWT 同 host 共存；
JWT 无效 token 仍按匿名继续（既有 401 语义不变）；HMAC/解析助手单测通过。
API Key 端到端：有效 key 通过、吊销后立即 401（网关集成测试）；validator 6 个单测 + store 3 个单测；
EF store 4 个 SQLite 测试 + 缓存 5 个单测 + 失败限流 2 个单测与 1 个端到端测试。

---

### G4 配额：预扣-结算

**问题**：现有 IP 限流无法表达"这个 key 每分钟 500 次、每分钟 20 万 token、最多 8 条并发流、每天 50 美元"。

**已实现 API**（`rust-webx-ai::quota`）：

```rust
pub trait IQuotaStore: Send + Sync {
    async fn reserve(&self, req: ReserveRequest) -> Result<Reservation, QuotaError>;
    async fn settle(&self, reservation: Reservation, usage: Usage, cost_micros: u64)
        -> Result<(), QuotaError>;
    async fn release(&self, reservation: Reservation) -> Result<(), QuotaError>;
}

pub struct ReserveRequest {
    pub key: QuotaKey,                 // tenant + key_id + model
    pub estimated_tokens: u64,
    pub estimated_cost_micros: u64,
    pub streams: u32,
    pub lease_ttl: Option<Duration>,
}

pub struct Reservation {
    pub lease_id: String,
    pub key: QuotaKey,
    pub reserved_tokens: u64,
    pub reserved_cost_micros: u64,
    pub concurrency_streams: u32,
    pub lease_ttl: Duration,
}
```

**实现要点**：

- 维度：RPM、TPM/TPD、并发流、预算（micro-USD）；`QuotaLimits` 每项 `Option` = 无限。
- 限额解析：`IQuotaPolicy`（`StaticQuotaPolicy` 支持默认值 + 按 key 覆盖）；生产实现应缓存，禁止每请求查库。
- 本地实现：`MemoryQuotaStore`（`std::sync::Mutex<HashMap>`，固定分钟窗口 + UTC 日窗口），单实例零 IO。
- 两阶段：`reserve` 扣估算（含并发槽），`settle` 用真实 usage/成本多退少补，`release` 全额退回。
- 幂等：`settle`/`release` 按 `lease_id` 去重；重复调用无操作。
- 租约：默认 60s；到期只释放并发槽，估算 token/成本保持计费（保守不低估）。
- 超限：`QuotaError::Exceeded { dimension, limit, retry_after }`；`http_status()` 给 429/503/400，
  OpenAI `insufficient_quota` 在 G8 映射。
- 分布式实现（Redis Lua / DB 事务）待补，端口不变。
- 审计规则：`WEBX-QUOTA-DB-PER-REQUEST`（每请求查库）、`WEBX-QUOTA-NO-SETTLE`（只扣不结）。

**验收**：RPM/TPM/TPD/并发/预算各自触发；结算多退少补；释放退款；重复结算幂等；租约过期释放槽位；
按 key 覆盖生效（8 个单测）。并发压测与 Redis 实现在分布式落地时补测。

---

### G5 计量与计费事件

**已实现 API**（`rust-webx-ai::usage`）：

```rust
pub struct Usage { pub prompt_tokens: u64, pub completion_tokens: u64,
                    pub cached_tokens: u64, pub reasoning_tokens: u64 }

pub trait ITokenizer: Send + Sync {
    fn count(&self, model: &str, text: &str) -> usize;
}
pub struct HeuristicTokenizer { /* bytes_per_token，兜底估算 */ }

pub trait IUsageSink: Send + Sync {
    async fn record(&self, event: UsageEvent) -> Result<(), UsageError>; // request_id 幂等
}

pub struct UsageEvent {
    pub request_id: String,       // 幂等键
    pub tenant_id: String,
    pub key_id: String,
    pub model: String,
    pub provider: String,
    pub usage: Usage,
    pub cost_micros: u64,
    pub latency_ms: u32,
    pub ttft_ms: Option<u32>,
    pub status: UsageStatus,      // Completed | ClientGone | UpstreamError
    pub created_at_unix_secs: u64, // 0 由 sink 盖章
}

pub struct UsageRollup { window_start_unix_secs, tenant_id, model, requests, usage, cost_micros }
pub fn rollup_events(events, window_secs) -> Vec<UsageRollup>;
```

**EF 账本**（`rust-webx-ai-ef`）：

```rust
let sink = EfUsageSink::from_options(Arc::clone(&db_options));
let executor = GatewayExecutor::new(catalog, router).with_usage(Arc::new(sink));
```

实体 `ai_usage_events`：`request_id` 主键 + 各 token 桶 + cost/latency/ttft/status/created_at；
每次 `record` 新建一个 `DbContext`，先查 `request_id`，重复直接返回；并发唯一冲突
（`EFError::UniqueViolation`）也按首写优先处理。

同一事务还写 `ai_usage_outbox`（默认开启，可 `without_outbox()` 关闭）：

```rust
let outbox = EfUsageOutbox::from_options(Arc::clone(&db_options));
let pending = outbox.pending(100).await?;       // delivered=false 且已到重试时间
outbox.mark_delivered("req-1").await?;
outbox.mark_failed("req-1", "billing 503").await?; // 指数退避

let rollup = EfUsageRollup::from_options(Arc::clone(&db_options));
let rows = rollup.rollup_window(window_start, 60).await?; // 幂等写入 ai_usage_rollups
```

后台 worker：

```rust
let outbox_worker = UsageOutboxWorker::new(outbox, handler, 100, Duration::from_secs(5));
let rollup_worker = UsageRollupWorker::new(rollup, Duration::from_secs(60), Duration::from_secs(30));

Host::builder().register(move |svc| {
    let outbox_worker = outbox_worker.clone();
    let rollup_worker = rollup_worker.clone();
    svc.add_background_service_with::<UsageOutboxWorker, _>(
            move |_| Arc::new(outbox_worker.clone()))
        .add_background_service_with::<UsageRollupWorker, _>(
            move |_| Arc::new(rollup_worker.clone()))
})
```

**实现要点**：

- 权威 usage 取上游最后一个 chunk；缺失才用 tokenizer 估算（当前 `HeuristicTokenizer` 为字节兜底，
  精确 `tiktoken` 表后续接入）。
- 结算不在 handler `after` 里做（那时流还没结束），而在 `GatewayStream` 完成/断连/失败时。
- `UsageEvent` 带 `created_at_unix_secs`；`MemoryUsageSink` 对 `0` 自动盖章，`EfUsageSink` 在转换时盖章。
- `record` 幂等：`request_id` 唯一索引 + 先查 + 唯一冲突兜底；`MemoryUsageSink` 首写优先。
- `rollup_events` 是纯函数（固定窗口、按 tenant+model 分组）；outbox/rollup 由 `PeriodicBackgroundService`
  worker 驱动，框架负责循环、取消与 join。
- 金额用整数 micro-USD，禁止浮点。
- 审计规则：`WEBX-USAGE-NO-IDEMPOTENCY`、`WEBX-COST-FLOAT`。

**验收**：`Usage` 各桶求和/饱和、`AddAssign`、事件 serde、estimator、sink 幂等/盖章/空 id、
rollup 分组与窗口（11 个单测）；EF 账本/outbox/rollup（6 个 SQLite 测试）；outbox worker
成功 ack/失败退避、rollup worker 上一窗口（2 个 worker 测试）。10 万事件重放与对账待补。

---

### G6 模型目录与路由

**已实现 API**（`rust-webx-ai::{provider,routing}`）：

```rust
pub struct ModelCatalog {
    pub version: u64,
    pub providers: HashMap<String, Arc<dyn ChatProvider>>,
    pub models: HashMap<ModelKey, ModelSpec>,
    pub aliases: HashMap<String, Alias>,          // Alias { strategy, targets }
}

pub trait ChatProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn complete(&self, req: CanonicalRequest) -> Result<CanonicalResponse, ProviderError>;
    async fn stream(&self, req: CanonicalRequest) -> Result<CanonicalStream, ProviderError>;
}

pub trait IRoutePlanner: Send + Sync {
    async fn plan(&self, req: &RouteRequest) -> Result<RoutePlan, RouteError>;
}

pub struct RoutePlan { pub alias: String, pub strategy: RouteStrategy,
                       pub attempts: Vec<Attempt>, pub deadline: Instant }
pub struct Attempt { pub model: ModelKey, pub timeout: Duration }
```

**策略**（已实现排序/过滤，执行器待补）：

| 策略 | 行为 | 适用 |
|---|---|---|
| Priority | 按目录顺序尝试 | 主力 + 备用 |
| Fallback | 与 Priority 同序，失败语义由执行器表达 | 可用性优先 |
| Weighted | 权重高者先（当前是确定性排序，随机加权待执行器） | 多 key/多区域均摊 |
| LeastLatency | EWMA 延迟低者先；未知延迟排最后 | 性能优先 |
| Capability | 先按能力过滤（tools/vision/json/stream…） | 避免 400 |

**实现要点**：

- 目录是不可变快照（`Arc<ModelCatalog>`），`/v1/models` 直接读快照；`ArcSwap` 热替换待补。
- 健康状态：`HealthRegistry` 每 `ModelKey` 维护连续失败、冷却截止、延迟 EWMA（默认 3 次失败/5s 冷却）；
  熔断细粒度策略在 G9。
- 候选过滤在 `plan` 阶段完成；`GatewayExecutor` 负责"按序尝试 + 首字节规则"：非流可重试，
  返回流后不再切换；配额预扣在调用前、结算/释放在成功/失败/断连后。
- 别名暂时是全局的；租户级覆盖后续进快照。
- 审计规则：`WEBX-ROUTE-NO-HEALTH`（无健康检查的盲切）。

**验收**：fallback 顺序、能力过滤、加权顺序、EWMA 排序、失败冷却恢复、未知别名 404、无候选 503、
最大尝试数截断（10 个路由单测）；执行器 6 个单测（重试、全失败、配额拒绝、流式计量、断连结算、
流创建重试）。租户级路由与热替换在后续补测。

---

### G7 协议适配与流事件

**已实现 IR**（`rust-webx-ai::ir`）：

```rust
pub struct CanonicalRequest {
    pub model: String,
    pub messages: Vec<Message>,          // Role + Vec<ContentPart>
    pub tools: Vec<ToolDef>,
    pub sampling: SamplingParams,
    pub stream: bool,
    pub extensions: serde_json::Map<String, serde_json::Value>, // 未知字段透传
}

pub enum StreamEvent {
    Start { id: String, model: String },
    Delta(Delta),                        // text / tool_call / reasoning
    Usage(Usage),
    Done(FinishReason),
    Error(ProviderError),
}
```

`Message` 支持文本/图片/工具调用/工具结果内容块；`ToolCall`、`ToolCallDelta`、`FinishReason`、
`CanonicalResponse` 已与 provider 端口对齐。

**两条 lane**：

1. **透传 lane（已实现 OpenAI 兼容适配器）**：`OpenAiProvider` 支持 OpenAI/Azure/vLLM/Ollama 的
   `/v1/chat/completions`；当前走 IR 编解码（正确性优先），真正的零重排字节透传在执行器落地时再加。
2. **转译 lane（待实现）**：Anthropic/Gemini 等。解码为 `StreamEvent`，再编码为 OpenAI chunk；
   工具调用增量、`finish_reason`、`usage` 归一。

`OpenAiProvider`：

```rust
let provider = OpenAiProvider::new(
    "openai",
    Arc::new(HttpClient::new("openai", HttpClientOptions::new("https://api.openai.com"))?),
    api_key,
);
catalog.add_provider(Arc::new(provider));
```

**实现要点**：

- 能力矩阵：`streaming`、`tools`、`vision`、`json_mode`、`reasoning`、`embeddings`、`images`、`audio`。
- 请求编码：文本/多模态/工具调用/工具结果、采样参数、`stream_options.include_usage`；`extensions` 不覆盖网关字段。
- 非流解码：`choices[0].message`（文本 + tool_calls）、`usage`（含 cached/reasoning 子桶）。
- 流解码：`Start`/`Delta`（text/reasoning/tool_calls）/`Usage`/`Done`，支持 `[DONE]` 与 EOF 兜底。
- HTTP 错误：401/403 → Unauthorized，429 → RateLimited（带 `Retry-After`），5xx → Unavailable，
  408/504 → Timeout，其余 4xx → InvalidRequest；错误体只取 `error.message`。
- 未知字段：顶层 `extensions` 捕获并透传；消息/工具级扩展后续补。
- 版本化 fixture：每 provider 的请求/响应/流样本入库，CI 回归（待补）。
- 审计规则：`WEBX-IR-VALUE-ALLOC`（热路径全量 `Value` 重排）。

**验收**：9 个适配器单测（请求编码、多模态/工具、扩展保护、非流解码、流 chunk 解码、tool delta、
状态映射、错误提取）；`rust-webx-ai-gateway` 10 个表面单测 + 7 个端到端测试（非流 JSON、SSE、
`/v1/models`、dry-run、未知模型 404、缺失鉴权 401、API key 吊销）。真实上游 SDK conformance 与
Anthropic/Gemini 在后续补。

---

### G8 错误模型与 OpenAI 兼容信封

**问题**：`Error` 没有 502/504；框架错误统一 problem+json，而 `/v1/*` 的 SDK 期望：

```json
{"error":{"message":"...","type":"rate_limit_error","param":null,"code":"insufficient_quota"}}
```

**已实现**：

1. 错误写入器端口（`webx-core`）与 `HostBuilder::add_error_writer`：按注册顺序取首个匹配；
   同时接收 `route_pattern`、`path`、`accept`，因此 `/v1/` 的 404（无路由模式）也能接管。
2. `OpenAiErrorWriter`（`webx-ai`）：输出 `{"error":{message,type,param,code}}`；5xx 不泄露内部
   detail；400/401/403/404/409/413/415/429/502/503/504 映射到 OpenAI type/code。
3. 1.x 仍用 `Error::Status(502 | 504, msg)` 表达上游错误（`Error` 公开 enum 未标
   `#[non_exhaustive]`，加变体是破坏性变更）；2.0 再增加 typed `BadGateway`/`GatewayTimeout`。
4. 端点层 `GatewayError::Quota` 的限额拒绝映射为 OpenAI
   `{"type":"insufficient_quota","code":"insufficient_quota"}`，并保留 `Retry-After`；
   普通 429 仍是 `rate_limit_error`。

```rust
pub trait IErrorResponseWriter: Send + Sync {
    fn matches(&self, route_pattern: Option<&str>, path: &str, accept: Option<&str>) -> bool;
    async fn write(&self, ctx: &mut dyn IHttpContext, error: &Error);
}
```

装配：

```rust
Host::builder()
    .add_error_writer(Arc::new(OpenAiErrorWriter::new())) // /v1/* → OpenAI 信封
    // 其余路径保持 problem+json
```

3. 上下游错误分类（供重试与前端展示）：

| 分类 | 例子 | HTTP | 可重试 | 可切候选 |
|---|---|---|---|---|
| `InvalidRequest` | 参数/上下文超限 | 400 | 否 | 否 |
| `Unauthorized` | 缺失/无效 key | 401 | 否 | 否 |
| `QuotaExceeded` | 预算/TPM 超限 | 429 | 是（按 Retry-After） | 否 |
| `UpstreamAuth` | 上游 key 失效 | 502 | 否 | 是（换 key） |
| `UpstreamRateLimited` | 上游 429 | 429/503 | 是 | 是 |
| `UpstreamUnavailable` | 连接失败/5xx | 502/503 | 是 | 是 |
| `UpstreamTimeout` | 首字节超时 | 504 | 是 | 是 |
| `ContentFiltered` | 上游内容拦截 | 400 | 否 | 否 |
| `Canceled` | 客户端断开 | — | — | — |

**验收**：自定义 writer 接管绑定错误与 404、其他路径仍 problem+json（3 个集成测试）；
`OpenAiError` 形状/5xx 脱敏/状态映射（5 个单测）；配额 `insufficient_quota` 信封（1 个单测）。
OpenAI SDK conformance 在端点落地时补。

---

## 7. P1 能力（生产级）

### G9 韧性

**已实现**：

- **重试**：`RetryPolicy`——仅对 `ProviderError::is_retryable()` 重试；指数退避 + 50–100% jitter；
  尝试上限；`Retry-After` 优先；执行器在 sleep 前检查 deadline。
- **熔断**：`CircuitBreaker`——按 `ModelKey` 连续失败阈值 → Open(cooldown) → HalfOpen(单探针)；
  执行器在调用前 `allow`，成功/失败回写。
- **舱壁**：`Bulkhead`——每 provider `Semaphore` 上限 + 最大等待；流式请求持有 permit 直到流结束/
  客户端断开，保护上游与自身。
- **入站 deadline**：`Server.RequestTimeoutSeconds`（默认 0 关闭）在路由命中后包住绑定 + Handler，
  超时取消 future 并返回 `504`；不限制已返回的流式响应体。
- **首字节规则**仍由执行器保证：非流可重试，流一旦返回不再切换。

**待补**：

- 流式首字节/空闲超时细化（出站已有 `request_timeout`/`read_timeout`）。
- 对冲（hedging）与按 p95 触发；默认关闭。
- 熔断状态已进 `ai_gateway_circuit_state` 指标与 `GatewayHealth`（`/health/ready`）。

**验收**：退避上限/`Retry-After`、可重试判定、熔断 Open/HalfOpen/恢复、舱壁限流与释放
（4 个单测）；执行器熔断短路与舱壁拒绝（2 个集成单测）。

### G10 凭据与密钥

**已实现（env/file/chain）**：

```rust
pub trait ISecretStore: Send + Sync {
    async fn get(&self, name: &str) -> Result<SecretValue, SecretError>;
}
```

- `SecretValue`：`Debug` 输出 `<redacted>`，`Drop` zeroize；不实现 `Clone`。
- `SecretRef { name, version }`：provider 配置存引用而非明文，轮换只改版本。
- `EnvSecretStore`：环境变量，空值视为 NotFound。
- `FileSecretStore`：一密钥一文件；名字白名单、拒绝符号链接/穿越、空文件 NotFound、
  Unix 强制 `0600`、trim 末尾换行。
- `ChainSecretStore`：按序尝试；`NotFound` 继续，`Unavailable` 立即失败（不隐蔽降级）。
- `OpenAiProvider::from_secret_store(..)`：解析 + 构造一步完成，失败是 typed `SecretError`。

**待补**：AES-GCM 信封加密 + KMS 主密钥、Vault 实现、自动轮换与 key pool 健康；
上游鉴权形态（Bearer / AWS SigV4 / GCP OAuth）仍由 provider 适配器实现。
- 审计规则：`WEBX-SECRET-INLINE`（配置内联明文密钥）。

### G11 缓存与单飞

**已实现**（`rust-webx-ai::cache`）：

```rust
let cache = Arc::new(MemoryGatewayCache::new(1024));
let executor = GatewayExecutor::new(catalog, router)
    .with_cache(cache)
    .with_cache_ttl(Duration::from_secs(60));
```

- 缓存键 = SHA256(`tenant_id` + `0` + 完整 `CanonicalRequest` 序列化)；
- 仅 `!stream && temperature == Some(0.0)` 可缓存；
- 租户隔离：键含 tenant，绝不跨租户复用；
- 命中直接返回 `CanonicalResponse`，不调用 provider、不预扣配额、不写 usage；
- 命中/未命中指标 `ai_gateway_cache_hits_total` / `ai_gateway_cache_misses_total`，
  请求指标 status=`cache_hit`；
- TTL 默认 60s，`MemoryGatewayCache` 有容量上限并清理过期项。
- **单飞**：`with_single_flight()` 后，同 key 的并发请求共享一次 `complete_uncached`
  （`tokio::sync::OnceCell`）；结果（成功或错误）广播给等待者，flight 完成后从 map 移除。
- 单飞只对可缓存 key 生效（非流 + `temperature==0`）；流式不参与。

**待补**：Redis 分布式缓存、按租户/模型策略开关、SSE 帧重放、单飞超时/取消指标。

### G12 可观测 2.0

**已实现**（`webx_core::metrics` + `rust-webx-ai` 执行器）：

```rust
pub trait IMeter: Send + Sync {
    fn counter(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Counter>;
    fn histogram(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Histogram>;
    fn gauge(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Gauge>;
}
```

内置 `NoopMeter`（丢弃）与 `MemoryMeter`（测试/小部署，可按 name+tags 读值）。

执行器实际发出的指标（标签基数受控，**不含** key_id/tenant）：

- `ai_gateway_requests_total{model,mode,status}`
- `ai_gateway_request_duration_ms{model,mode,status}`
- `ai_gateway_ttft_ms{model}`（流式）
- `ai_gateway_tokens_total{model,kind=prompt|completion|cached|reasoning}`
- `ai_gateway_cost_micros_total{model}`
- `ai_gateway_active_streams{model}`（gauge，流开始 +1 / 结束-断连 -1）
- `ai_gateway_upstream_errors_total{provider,retryable}`
- `ai_gateway_quota_rejections_total{model}`
- `ai_gateway_circuit_state{provider,model}`（gauge，0 closed / 1 half-open / 2 open）

装配：

```rust
let executor = GatewayExecutor::new(catalog, router)
    .with_meter(Arc::new(my_meter)) // 或 Arc::new(MemoryMeter::new())
    .with_cost_calculator(Arc::new(
        StaticCostTable::new().with_price("fast", ModelPrice::new(150_000, 600_000, 75_000, 600_000)),
    ));
```

**成本**：`ICostCalculator` 按公开别名 + `Usage` 返回 micro-USD；`StaticCostTable` 的
cached/reasoning 桶单独计价，不重复计入 base token。执行器把成本用于配额结算、`UsageEvent.cost_micros`
与 `ai_gateway_cost_micros_total`。

**结构化日志**（`tracing`）：

- 完成：`request_id`、`tenant_id`、`model`、`provider`、`mode`、`status`、`latency_ms`、
  `prompt/completion/cached/reasoning_tokens`、`cost_micros`、`trace_id`；
- 失败：同一组标识 + `error`（不含 prompt/response 文本）；
- 流：start 一条，finish/断连/失败一条（带 `ttft_ms`）；
- 上游每次 attempt 失败单独 warn（provider、retryable、error）。

**W3C traceparent**：

- 入站：`RequestTracing` 优先取 `x-request-id`；否则解析合法 `traceparent`
  （`00-32hex-16hex-2hex`），把 32-hex trace id 作为 `x-trace-id` 与
  `RequestContext::trace_id()`；非法/全零则生成新 32-hex id。
- 出站：`rust-webx-client` 在请求未显式设置 `traceparent` 时，从
  `RequestContext::trace_id()` 注入 `00-{trace_id}-{span}-01`（只接受 32-hex）；
  显式 header 优先。

**待补**：上游子 span 树、OTel exporter；`traceparent` 的 `tracestate` 透传。就绪聚合已由
`rust-webx-ai-gateway::GatewayHealth` 提供。

### G13 动态配置

**已实现**（`rust-webx-ai`）：

```rust
let handle = CatalogHandle::new(initial_catalog);
let router = Arc::new(CatalogRouter::from_handle(handle.clone()));
let executor = GatewayExecutor::new_with_handle(handle.clone(), router);

handle.reload(new_catalog); // version 不回退；热路径 current() 无锁
```

- `CatalogHandle` 用 `ArcSwap<ModelCatalog>`：构建新快照 → `reload`，旧版本拒绝。
- `CatalogRouter` 每次 `plan` 读当前快照；`GatewayExecutor` 解析 provider 也读同一 handle。
- 版本号用于诊断与防回退；provider/别名/模型/能力变更无需重启。
- 失败/回滚：保留旧快照即可；热路径只读，不查库。

**通用配置热更新（框架层）**：

```rust
Host::builder()
    .watch_options_file::<GatewayConfig>("Gateway", "config/gateway.json", Duration::from_secs(5))
    .build();
// Handler/服务注入 Arc<dyn IOptionsMonitor<GatewayConfig>>，current() 无锁读取
```

- `OptionsMonitor<T>` = `ArcSwap` 快照 + `watch` 通知；替换是整快照、不可变。
- `OptionsFileWatcher<T>` = `PeriodicBackgroundService`：内容 hash 去重、文件缺失不算错误、
  坏 JSON 返回 typed error 且**保留旧快照**（失败退化为上一版，不宕机）。
- 订阅者从当前快照开始；无订阅者时替换仍生效。

**文件热加载（CatalogFileWatcher）**：manifest 只声明 models/capabilities/aliases（provider 由代码提供），
`CatalogFileWatcher` 轮询 JSON → 校验（未知 provider/target、重复、版本回退）→ `reload`；内容 hash
去重，坏文件保留旧快照。装配见 `11-production/hot-config.md`。

**待补**：多实例版本轮询、canary 校验、配额策略/价格/key 轮换。

### G14 请求体流式

**已实现（入站 + 出站）**：

```rust
// 入站
let mut stream = ctx.request_mut().body_stream().await?;

// 出站
let request = OutboundRequest::post("/v1/files")
    .body_stream(chunk_stream);
```

- 入站 `IHttpRequest::body_stream()` 返回 `Pin<Box<dyn Stream<Item=Result<Bytes>> + Send>>`；
  默认回退 `body_bytes`，host 覆盖为实时 hyper `Incoming` 流；body 只能消费一次；
  仍按 `App.MaxBodySize` 计数，超出 413；与 `multipart()` 互斥。
- 出站 `OutboundRequest::body_stream(stream)` 接受 `Stream<Item=Result<Bytes, HttpClientError>>`；
  `rust-webx-client` 用 `UnsyncBoxBody` + `StreamBody` 逐 chunk 发送；`body(bytes)` 保持零拷贝路径。
- `OutboundRequest` 因流式 body 不再 `Clone`（仍是 `Debug`）。

**待补**：

- 出站流式请求的 content-length/chunked 策略与失败重试（首字节前）；
- multipart 的按路由预算；大 batch/audio 的背压调优与取消传播。

按路由 `#[max_body_size(..)]` 已实现（非 multipart，绑定前 413）。

### G15 长流停机

**已实现**：

```json
{
  "Server": {
    "DrainTimeoutSeconds": 30,
    "StreamDrainTimeoutSeconds": 300
  }
}
```

- 两个预算分离：普通连接用 `DrainTimeoutSeconds`，含 SSE 响应的连接用
  `StreamDrainTimeoutSeconds`；
- 主机按监听器跟踪“有 SSE 响应的连接数”（连接内首个 SSE 响应标记，连接结束时 -1）；
- `drain_connections` 在 active>0 时选 stream 预算，否则选普通预算；最小 1s；
- `ShutdownToken`（`Host::build` 注册到 DI）随所有停机路径取消；`SseResult::on_server_shutdown`
  在下一个 poll 写 `event:server_shutdown\ndata:{}` 后正常结束，`on_close` 收到
  `StreamEnd::ServerShutdown`；
- `SseResult::max_lifetime` 仍可兜底防止僵尸流。

**待补**：后端（上游请求/结算）drain 协调；按流而不是按连接的精确预算；停机事件的可配置 payload。

### G16 测试工具链

**已实现**：

```rust
let mock = MockUpstream::start().await?;
mock.json(200, &response_json);
mock.sse([r#"data: {"choices":[{"delta":{"content":"hel"}}]}"#, "data: [DONE]"]);
mock.status(429);
mock.delay_before_next(Duration::from_millis(50));

let client = HttpClient::new("mock", HttpClientOptions::new(mock.base_url()))?;
let requests = mock.requests(); // method/path/headers/body
```

- `webx_client::testing::MockUpstream`：ephemeral 端口、脚本化 JSON/SSE/状态、延迟、请求录制；
- `ReplayFixture`：`mock.fixture()` / `save_fixture(path)` / `load_fixture(path)` /
  `mock.replay(&fixture)`；fixture 含 method/path/headers/body 与 status/headers/body；
- OpenAI 适配器用真实 HTTP + SSE 跑通 `complete`/`stream`，断言请求体、Authorization、响应解析与 `[DONE]`；
- SSE 解码器覆盖**每一个字节切分点** + 100 组确定性伪随机多 chunk 切分，事件序列保持不变。

录制/回放：

```rust
mock.json(200, &response);
let fixture = mock.fixture();
mock.save_fixture("fixtures/openai.json")?;

let fixture = MockUpstream::load_fixture("fixtures/openai.json")?;
let replay = MockUpstream::start().await?;
replay.replay(&fixture);
```

**待补**：

- fixture 版本字段与向前兼容检查；敏感头默认脱敏（`save_fixture`）与显式原始保存；
- 官方 SDK conformance；
- SSE 解析 property/fuzz；
- `oha`/`k6` 压测场景与内存断言。

---

## 8. P2 差异化（行业领先）

| 能力 | 设计要点 |
|---|---|
| 自适应路由 | 在线 EWMA（延迟/成功率/成本）× 权重；探索-利用（ε-greedy）；离线回放调参 |
| 语义缓存 | 向量相似 + 租户隔离 + 阈值/人工复核；默认关闭，按 key 开启 |
| Batch/异步任务 | 上传文件 → job 状态机 → worker 调 Batch API → 结果下载；复用后台服务与 EF |
| Realtime/WS | host 增加 WebSocket upgrade；双向流转发；音频帧透传；独立并发预算 |
| 多区域 | 区域亲和 + 故障转移；目录带 region；跨区不计费重复 |
| 插件 SDK | provider 适配器 crates + conformance 套件 + 版本协商；不做 WASM 沙箱（v1） |
| 成本模拟 | 请求预演（dry-run）返回估算 token/成本，不调用上游（已实现：`GatewayExecutor::estimate` + `dry_run`） |

---

## 9. 参考应用：聚合服务能有多简单

目标：业务 `main.rs` 只做**装配与策略**，不写传输/重试/计量代码。

```rust
#[webx::main]
async fn main() -> webx::Result<()> {
    Host::builder()
        .add_options::<GatewayOptions>("Gateway")
        .add_http_clients()                      // 读 Gateway.Providers，建命名客户端
        .add_authentication()                    // 管理面 JWT
        .add_api_key_auth()                      // 数据面 sk-
        .add_openai_compatible_surface()         // /v1/chat/completions、/v1/embeddings、/v1/models
        .add_provider(OpenAiProvider::new("openai", SecretRef::new("OPENAI_API_KEY")))
        .add_provider(AnthropicProvider::new("anthropic", SecretRef::new("ANTHROPIC_API_KEY")))
        .add_alias("fast", ["openai:gpt-4o-mini", "anthropic:claude-haiku"])
        .strategy(Strategy::Fallback)
        .use_quota(QuotaPolicy::default())       // RPM/TPM/并发/预算
        .use_usage(EfUsageLedger::new())         // usage_events + outbox
        .add_background_service::<UsageRollupWorker>()
        .build()
        .run()
        .await
}
```

自定义 provider 适配器（约 40 行）：

```rust
pub struct MyProvider { http: Arc<dyn IHttpClient>, key: SecretRef }

#[async_trait]
impl ChatProvider for MyProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities::new().stream().tools()
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<CanonicalStream> {
        let upstream = self.http
            .execute(self.encode(req)?)   // 请求编码
            .await?;
        Ok(self.decode_stream(upstream.body.sse()))  // 流事件映射
    }
}
```

自定义端点（非标准厂商能力）：

```rust
#[derive(Debug, Clone, Default, WebxRequestMeta)]
pub struct RerankRequest {
    #[frombody] pub model: String,
    #[frombody] pub query: String,
    #[frombody] pub documents: Vec<String>,
    #[serde(flatten)] pub extensions: serde_json::Map<String, serde_json::Value>,
}

#[post("/v1/rerank")]
impl IRequest<RerankResponse> for RerankRequest {}
```

**"简单"的验收标准**：新增一个 provider 只需实现 1 个 trait + 注册 1 行；新增一个 OpenAI 兼容端点只需 1 个请求类型 + 1 个 handler；配额/计量/重试/流式零业务代码。

业务方**不需要写**：SSE 管道、重试/熔断循环、API key 校验与缓存、token 计数、计费落库、provider HTTP 封装、协议错误映射、TTFT/用量指标。
业务方**只需要写**：定价表、租户/权限模型、provider 接入配置、运营策略（限流档位、别名路由、预算）。

---

## 10. 性能设计与 SLO

### 10.1 目标

| 指标 | 目标 | 关键手段 |
|---|---|---|
| 代理开销 p99（非流） | ≤ 3ms | `Bytes` 零拷贝、无 DB、无全量 `Value` 重排 |
| TTFT 增量 p99（流） | ≤ 5ms | 首帧直通、`TCP_NODELAY`、不缓冲、只提取 usage |
| 单节点并发 SSE（8 vCPU） | ≥ 20k | 每流 O(1)、无每 token task、有界缓冲 |
| 每流额外内存 | ≤ 4 KiB | 复用 `BytesMut`/`Bytes`、64 KiB 传输块 |
| 上游连接复用率 | ≥ 95% | 连接池、keep-alive、HTTP/2 |
| 配额判定 p99 | ≤ 1ms 本地 / ≤ 5ms 分布式 | 本地桶 + Redis Lua |
| 计量落库 | 异步、批量 | outbox + rollup，不在请求路径 |

### 10.2 手段

- **零缓冲**：请求/响应都不落盘；流式响应逐帧写。
- **零重排**：透传 lane 不 `serde_json::from_slice` 再 `to_vec`；只解析 `usage`/`model` 关键字段。
- **背压**：下游慢 → hyper 不拉 → 上游 body 不 poll → TCP 窗口关闭；不设无界 channel。
- **有界**：并发、队列、body、token、输出长度、连接数全部有上限；超限快速 503。
- **无锁热路径**：`ArcSwap` 目录、`DashMap` key 缓存、原子计数；全局 `Mutex` 只允许在初始化/慢路径。
- **取消传播**：客户端断开 → 丢弃流 → 上游连接关闭/重置；`CancellationToken` 贯穿。
- **超时分层**：connect / TLS / 首字节 / 空闲 / 总预算，各自独立。
- **连接上限**：`App.MaxConnections`、文件描述符、内核 backlog 与目标并发对齐（20k 流按 1.2 倍预留）；反代层单独核算。
- **避免库开销**：不引入第二套 HTTP/TLS；不用 `reqwest` 包一层再转 `Bytes`。

### 10.3 反模式（审计拦截）

- 每 token `tokio::spawn`。
- 流式响应先 `collect()` 再发。
- 每请求查 DB 做配额/密钥/目录。
- 在 `after` 中间件里读流式响应体（拿不到，且语义错误）。
- gzip 压缩 `text/event-stream`。
- 流开始后重试（重复输出/重复计费）。
- 全局 `Mutex<HashMap<...>>` 做限流。

---

## 11. 数据模型与一致性

### 11.1 控制面（EF）

| 表 | 关键列 | 说明 |
|---|---|---|
| `providers` | id, kind, base_url, timeout, weight, enabled | provider 定义 |
| `provider_keys` | id, provider_id, secret_ref, status, cooldown_until | key pool |
| `models` | id, provider_id, upstream_model, capabilities, price_in/out, context | 能力与价格 |
| `model_aliases` | tenant_id, alias, strategy, targets | 路由策略 |
| `api_keys` | id, tenant_id, prefix, hash, scopes, rpm/tpm, budget, status | 数据面密钥 |
| `quotas` | tenant_id, key_id, dimension, limit, window | 配额策略 |

### 11.2 经济面

| 表 | 关键列 | 说明 |
|---|---|---|
| `usage_events` | request_id(unique), tenant, key, model, provider, tokens, cost, latency, ttft, status | 追加写，幂等 |
| `usage_rollups` | window_start, tenant, model, sums | 聚合 |
| `outbox` | id, topic, payload, attempts, next_at | 外发计费/审计 |
| `budgets` | tenant, period, limit_micros, used_micros | 预算 |

### 11.3 预扣-结算状态机

```text
reserve ──► active ──(流完成)──────────► settled(actual)
   │           │
   │           ├─(客户端断开)─────────► settled(estimated)
   │           └─(上游失败/无输出)────► released
   └─(handler 早失败)───────────────► released
   └─(租约过期)─────────────────────► expired(告警)
```

- `settle` 幂等：`request_id` 唯一；重复调用返回成功。
- 结算 worker 批量写；失败重试 + 死信。
- 对账任务：上游账单 vs `usage_events` 差异 > 阈值告警。

---

## 12. 失败语义与计费口径

| 场景 | 客户端可见 | 是否重试/切换 | 计费口径 |
|---|---|---|---|
| 上游连接失败（首字节前） | 502/504（或下一候选成功） | 是 | 不计费 |
| 上游 429 | 透传 429 + `Retry-After` | 可切候选（按策略） | 不计费 |
| 上游 401/403 | 502 `upstream_auth`（可配置透传；默认不泄露细节） | 换 key 后可切 | 不计费 |
| 上游 5xx | 502/503 | 是 | 不计费 |
| 首字节超时 | 504 | 是 | 不计费 |
| 流中途断开 | 流结束（客户端看到断流） | **否** | 按已生成估算/上游 usage |
| 客户端断开 | 无 | 取消上游 | 按已生成估算 |
| 网关配额不足 | 429 `insufficient_quota` | 否 | 不计费 |
| 请求体过大 | 413 | 否 | 不计费 |
| 内容过滤 | 400 | 否 | 按上游规则 |

**原则**：计费以"上游已产生用量"为准；网关自己的失败不转嫁给用户；断连必须有明确、可配置、可审计的估算策略。

---

## 13. 安全与滥用防护

- **密钥**：API key 只存 HMAC；provider secret 信封加密；日志/错误/追踪全链路脱敏；`SecretValue` 无 `Debug`、Drop 零化。
- **SSRF**：`rust-webx-client` 默认拒绝私网/链路本地/元数据字面地址与 `.local/.internal` 后缀
  （`ssrf_protection`/`allow_private_networks`）；回环放行以便本地开发。DNS 解析后复查与重定向
  重校验仍由应用 URL 允许列表补足。头卫生：剥离 hop-by-hop；客户端 `Authorization` 绝不转发上游；
  上游 key 绝不回显；响应头白名单。
- **租户隔离**：缓存、单飞、配额、用量全部带 tenant；跨租户复用视为严重缺陷。
- **滥用**：认证失败限流、按前缀熔断、模型枚举防护、输出长度上限、并发上限。
- **内容**：默认不记录 prompt/response；可选审核钩子（前置/后置）；保留期与删除策略。
- **管理面**：JWT + 角色；高危操作（轮换 key、改价、删 provider）审计；二次确认。

---

## 14. 交付路线图

| 阶段 | 周期 | 内容 | 门禁 |
|---|---|---|---|
| **P0 设计冻结** | 1–2 周 | 本文评审；`SseResult`/`IHttpClient`/auth scheme/错误 writer 的 API 草案；SSE 代理 spike | API 评审通过；spike 达到 TTFT 目标 |
| **P1 最小可用** | 5–6 周 | G1/G2/G3/G4/G5/G6/G7/G8；OpenAI 透传 lane；2 家 provider；本地配额；usage 落库；基础指标 | OpenAI SDK conformance；1k 并发流；审计 0 error |
| **P2 生产级** | 5–6 周 | G9–G16：韧性、Redis 配额、凭据轮换、缓存/单飞、可观测 2.0、热配置、长流停机、测试工具 | ≥10k 并发流（目标 20k）；chaos 通过；计费不变量；SLO 达标 |
| **P3 差异化** | 持续 | 自适应路由、语义缓存、Batch、Realtime、多区域、插件 SDK | 每项独立 ADR + 基准 + 回滚方案 |

**依赖关系**：G1/G2 先行（其余能力都建在流与客户端上）；G3/G4/G5 可与 G2 并行；G6/G7 依赖 G2/G8；G9 依赖 G2；G13 依赖 G4/G6 的快照形态。

**风险与对策**：

| 风险 | 对策 |
|---|---|
| 范围蔓延成"通用 API 网关" | 明确非目标；只做 AI 数据面需要的传输，不做任意协议转换 |
| 公开 enum 的破坏性变更 | 1.x 用桥接实现；`ResponseBody::Stream` 留到 2.0；新能力进新 crate |
| 上游 API 漂移 | 版本化 fixture + conformance CI；能力矩阵显式声明 |
| 断连计费争议 | 策略可配置 + 事件可审计 + 上游账单对账 |
| 性能回退 | CI 基准门槛（TTFT/开销/内存）；热路径代码评审清单 |
| 多租户越权 | 租户维度贯穿缓存/配额/用量；负例测试 |

---

## 15. 非目标

- 不做模型推理运行时（vLLM/TGI 的事）。
- 不做通用反向代理/API 网关（任意协议转换、WASM 插件市场）。
- 不做完整计费/发票系统（只产出用量事件与账本）。
- 不在 v1 做 UI；管理面 API 优先。
- 不把 provider 专有特性（如某家特有参数）提升为框架概念；用能力标志 + 扩展字段。
- 不为"未来可能需要"提前抽象；每个新端口必须有 ≥2 个真实实现或明确替换场景。

---

## 16. 验收清单

**框架层（webx）**

- [x] `SseResult` 帧/断连/完成钩子恰好一次；服务端有端到端测试。
- [x] SSE 响应头正确（`text/event-stream`、`no-store`、`x-accel-buffering: no`），不被压缩。
- [x] `IHttpClient` 支持流式响应、连接池、HTTP/2、代理、超时分层、企业 CA、SSRF 字面守卫。
      （连接复用率未做基准测量。）
- [x] 认证 scheme 可与 JWT 共存；`sk-` 撤销生效；失败限流生效。
- [x] `IErrorResponseWriter` 让 `/v1/*` 返回 OpenAI 信封，管理面仍 problem+json。
- [x] `IMeter` 输出带标签直方图；TTFT/token/成本/熔断状态可查询；`GatewayHealth` 就绪聚合。
- [x] `IOptionsMonitor` / `CatalogFileWatcher` 热更新生效；热路径无 DB。
- [x] 停机：长流按预算排空，`server_shutdown` 事件，结算由流结束/Drop 触发。
- [x] `TCP_NODELAY` 默认开启；drain 超时可配；入站 `RequestTimeoutSeconds`。
- [ ] 审计新增规则（rust_xazgo 审计的默认目标是 Travel `server/`，本仓库无该目录；规则文本待工具侧接入）。
- [x] 文档：设计页 + 手册页 + 示例与设计进度同批更新。

**网关层（rust-webx-ai）**

- [x] 新增 provider = 1 个 trait 实现 + 1 行注册。
- [x] 新增 OpenAI 兼容端点 = 1 个请求类型 + 1 个 handler。
- [x] 失败切换不改变客户端可见 model/响应形状（执行器测试）。
- [x] 配额不超卖（本地 store）；断连槽位释放；结算/释放幂等。分布式 store 属部署实现。
- [x] 用量汇总 = 明细之和；rollup 幂等。（上游账单对账属部署任务。）
- [ ] ≥10k 并发流压测达标（目标 20k）；p99 开销达标（需要独立压测环境，未在本仓库验证）。
- [x] 安全：SSRF 字面守卫、缓存/配额租户隔离、密钥/凭据脱敏负例。

---

## 附录 A：新增公开面一览（提案）

| 模块 | 新增 | 类型 |
|---|---|---|
| `webx::stream` | `SseEvent`、`SseResult`、`StreamEnd`、`SseStream` | 类型 |
| `webx::http` | `IHttpRequest::body_stream`（默认回退） | trait 方法 |
| `webx::auth` | `IAuthenticationScheme`、`AuthenticationOptions`、`SchemeSelector` | trait/类型 |
| `webx::error` | 1.x：`Status(502/504)`；2.0：`BadGateway`、`GatewayTimeout`；`IErrorResponseWriter` | 变体/trait |
| `webx::metrics` | `IMeter`、`Counter`、`Histogram`、`Gauge` | trait/类型 |
| `webx::options` | `IOptionsMonitor<T>` | trait |
| `webx_client` | `IHttpClient`、`IHttpClientFactory`、`HttpClientOptions`、`OutboundRequest/Response`、`RetryPolicy`、`CircuitBreaker` | 新 crate |
| `rust-webx-ai` | `ChatProvider`、`CanonicalRequest/StreamEvent`、`ModelCatalog`、`IRoutePlanner`、`GatewayExecutor`、`IQuotaStore`、`IUsageSink`、`ITokenizer`、`OpenAiErrorWriter`、admin | 新 crate |
| `rust-webx-ai-ef` | `EfUsageSink`、`ai_usage_events` 实体 | 新 crate |
| `rust-webx-ai-gateway` | `/v1/chat/completions` DTO、IR 转换、JSON/SSE、OpenAI 错误结果 | 新 crate |

## 附录 B：与既有框架能力的边界

| 既有能力 | 网关里怎么用 | 不要再造 |
|---|---|---|
| `IdentityUser`/`RequestUser` | API Key claims 复用；租户隔离 | 自定义 principal 结构 |
| `#[authorize]` | 管理面权限；数据面用 key 校验 | 手写权限中间件 |
| `Error`/ProblemDetails | 管理面错误；数据面经 writer 转 OpenAI | 自定义错误枚举贯穿全局 |
| `IDistributedCache` | 精确缓存/单飞状态 | 自建 Redis 客户端封装 |
| `BackgroundService` | rollup、对账、outbox、健康探测 | 手写 sleep 轮询 |
| `DbContext` + EF | 控制面/经济面 | 手写 SQL（除批量 upsert/分析） |
| `PagedRequest/PagedResult` | 管理面列表 | 自定义分页 |
| `OperationResult` | 管理面命令 | 自定义 `{success,msg}` |
| `health` | 进程/DB/Redis 就绪 | 自定义 `/api/health` |
| `trace_id` | 日志/审计关联 | 另造 request id |
