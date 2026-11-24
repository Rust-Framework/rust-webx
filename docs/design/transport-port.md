# 传输端口（Transport Port）设计

> 面向维护者与架构评审：engine 与线上协议之间的接缝怎么切、为什么这样切、边界在哪。
> 当前基线 rust-webx 2.9.0。约束来自 [框架设计准则](framework-constitution.md)（P1 一个关注点
> 一种写法、P9 端口而非插件、P10 小核大生态）与 [兼容性政策](compatibility.md)（2.x 只做加性变更）。
> 应用公开面（`IHttpContext`/`IRequest`/中间件/DI/配置/错误）保持不变；HTTP/3 是**可选 crate**
> `rust-webx-h3`，默认构建不链接 QUIC。当前请求生命周期以
> [请求生命周期](../rust-webx/04-architecture/request-lifecycle.md) 为准，本文不重复。
> HTTP/3 的下游缺口处置与互操作基线见 [upstream-gaps.md](upstream-gaps.md)。

---

## 1. 边界：端口切在 engine ↔ 线上协议

传输现状（全部在 `rust-webx-engine`，这是端口设计的边界条件）：

| 层 | 实现 | 位置 |
|---|---|---|
| 监听 | `Listener`（`std::net::TcpListener` + 可选 rustls 配置）；每 listener 一个 acceptor | `server.rs` |
| 连接交接 | accept → 全局准入 governor → `into_std()` → 有界 `mpsc` → worker | `server.rs`、`worker.rs` |
| 协议栈 | 每 worker 一个 hyper `auto::Builder`（HTTP/1.1 + HTTP/2）；TLS 在 worker 内握手 | `worker.rs` |
| 请求 | `hyper::Request<Incoming>` → `http::request::Parts` + `BodyState` | `context.rs` |
| body | `read_body` / `stream_body` / `parse_multipart` 消费帧流 | `body.rs` |
| 响应 | 中立 `ResponseParts`/`ResponseBody` 组装 → hyper `Response<RespBody>` | `response.rs`、`context.rs` |
| 升级 | hyper `OnUpgrade` → `IUpgradeSession::run(UpgradedTransport, ConnectionMeta)` | `context.rs`、`webx_core::realtime` |
| 排空 | 连接 `graceful_shutdown()`；普通/SSE 两档预算；`ActiveStreamGuard` 计数 | `worker.rs`、`shared.rs` |

两个已有事实决定端口切点：

1. **应用面早已传输无关。** 中间件/Handler 只看到 `IHttpContext`/`IHttpRequest`/`IHttpResponse`、
   `RequestBodyStream`、`ResponseBody`/`FileBody`/`FileSource`；实时面只看到 `IUpgradeSession`/
   `UpgradedTransport`/`IConnection`。因此新端口切在 **engine ↔ 线上协议**，不再包一层应用契约。
2. **字节流接缝已存在。** `Server::serve_stream<I: AsyncRead + AsyncWrite>` 复用生产 hyper 配置、
   路由、中间件、错误写入与排空，但不占准入槽（`TestHost::connect_wire` 用它做无端口 wire 测试，
   见 [test-host.md](test-host.md)）。它换的是**字节流来源**，不换**帧协议**：HTTP/3 的 QUIC/QPACK
   不在 hyper 能力内，所以 H3 走下面的中立端口，而不是再包一层 `serve_stream`。

---

## 2. 端口形状：监听后端 + 每请求入口

HTTP/1.1 是「连接 ≈ 请求序列」，HTTP/2、HTTP/3 是「连接 = 多路复用 stream」。端口把这个差异
留在传输内部：后端拥有连接与协议，engine 只看到「一个已解码的请求 → 一个响应」。

```rust
/// 监听端点上的传输后端：accept、协议握手、连接级排空。
#[async_trait::async_trait]
pub trait IHttpTransport: Send + Sync {
    /// 稳定标签，进指标/日志：`"h1"` / `"h2"` / `"h3"` / `"tls+h2"`。
    fn kind(&self) -> &'static str;
    /// Host 等待本后端停机的预算；后端拥有自己协议级排空的最坏值。
    fn stop_budget(&self) -> Duration; // 默认 30s
    /// `runtime.subscribe_shutdown()` 触发后停止 accept，并排空本端点已建立的连接。
    async fn run(self: Arc<Self>, runtime: TransportRuntime) -> Result<()>;
}

/// engine 交给传输后端的运行时句柄：唯一准入 governor、shutdown 信号、指标与 `Server` 入口。
pub struct TransportRuntime { /* shared + governor + shutdown；见 transport.rs */ }
```

后端把每个解码后的请求交给 `Server::serve_transport_request(request)`：
h3 每个 request stream 一个任务；测试用 fake listener 直接循环调用。只有**一层**端口，
因为真实的非 hyper 后端都是多路复用的；顺序连接型后端同样可以按请求调用该入口，
不需要额外的连接级 trait（超集即够用）。

请求/响应/升级的中立类型（`crates/engine/src/transport.rs`）：

- `TransportRequest`：`http::request::Parts` + `peer` + `TransportRequestBody` + 可选
  `TransportUpgrade` + 可选的 client-cert SHA-256；
- `TransportRequestBody`：`Empty`、`Buffered(Bytes)` 或 `Streaming(TransportRequestBodyStream, exact_len)`；
  帧为 `TransportBodyFrame::{Data, Trailers}`（`Send + Sync` 帧流，保持 `EngineRequest: Sync`）；
- `TransportResponse`：`status` + `HeaderMap` + `TransportResponseBody::{None, Bytes, Stream}`；
- `TransportUpgrade`：提交响应后解析出的 `UpgradedTransport` future。

**不变量**

1. **只有一份 pipeline。** 路由、绑定、鉴权、中间件、错误写入、panic 边界都在
   `connection::serve` / `Server::serve_transport_request` 里；传输后端只做帧编解码与连接管理。
   新增传输不允许复制管道。
2. **只有一个准入 governor。** `TransportRuntime::admit_connection()` 与 TCP acceptor 共享
   `App.MaxConnections` 与 `engine_connections_*` 计数；QUIC 连接 = 1 permit，stream 并发是
   另一维度（受传输层上限约束），不乘进 `MaxConnections`。
3. **动态分发只在连接/构造层。** hyper 热路径不新增 trait-object 分发；请求体适配器只装箱一次
   （`EngineRequest: Sync` 的公开 auto trait 保持不变，适配器用帧级互斥满足 `Sync`）。
   端口对象在连接建立时构造一次。
4. **协议差异留在传输内部。** 握手（`101+Upgrade`、extended CONNECT、WebTransport
   `:protocol`）、流控、GOAWAY/`CONNECTION_CLOSE`、首请求与请求体超时由后端按 engine 配置执行；
   engine/host 不出现 `if h3`。

当前实现归属：

| 端口 | 生产实现 | 测试实现 |
|---|---|---|
| `IHttpTransport` | `rust-webx-h3` 的 `H3Transport` | `FakeListener`（`crates/webx/tests/transport_port_test.rs`） |
| `serve_transport_request` | hyper 路径直出（不经该入口）；h3 每 stream 调用 | `TestHost::serve_transport_request` |

---

## 3. EngineContext 的接缝

`EngineContext` 的内部数据是中立类型；hyper 构造/终结入口只是适配器：

| 位置 | 形态 |
|---|---|
| `EngineContext::new(hyper::Request<Incoming>, ..)` | 2.8 公开构造（签名保留）：经 `hyper_compat` 拆 `Parts`、映射 body/upgrade，再走 `from_transport` |
| `EngineContext::from_transport(TransportRequest, ..)` | 中立入口（生产 hyper acceptor 与 `serve_transport_request` 都走它） |
| `from_parts(parts, Bytes, ..)` | 进程内/测试构造，等价于 buffered body 分支 |
| `BodyState` | 统一消费 `TransportRequestBody`；`body.rs` 的读取/流式/multipart 只有一份实现 |
| `finalize_transport()` | 产出中立 `TransportResponse`；hyper 适配器再组装为 `Response<RespBody>` |
| `finalize()` | hyper 兼容入口，行为与字节输出保持既有语义（HEAD/204/304、安全头、SSE） |

传输后端入口：

```rust
impl Server {
    pub async fn run_listeners(self, listeners: Vec<Listener>) -> Result<()>; // TCP + hyper
    pub async fn run_transport(self, transport: Arc<dyn IHttpTransport>) -> Result<()>;
}
```

`HostBuilder::use_http_transport(Arc<dyn IHttpTransport>)` 把后端接入 Host 生命周期
（启动/停止预算与 hosted services、DI dispose 的顺序见
[优雅停机](../rust-webx/11-production/graceful-shutdown.md)）。HTTP/1.1+2 仍走 worker 池；
`Server.Workers` 不影响非 hyper 传输——后端在自己的调用方 runtime 上驱动连接。

---

## 4. 语义映射

| 语义 | TCP + hyper | 非 hyper 后端（HTTP/3） | 复用点 |
|---|---|---|---|
| 响应流 | `ResponseBody::File` → 64 KiB 帧 | h3 `send_data(Bytes)` 逐帧发送，`finish()` 结束 | `FileSource`、`STREAM_BUF_SIZE` |
| 背压 | 客户端不读 → 源流暂停 | QUIC `SendStream` 容量窗口 → `send_data` 等待 | 不缓存整个响应 |
| 排空 | 连接 `graceful_shutdown()` + `DrainTimeoutSeconds`/`StreamDrainTimeoutSeconds` | GOAWAY → 等在途 stream → 预算耗尽 `CONNECTION_CLOSE`；h3 当前用固定有界预算（10s 在途 + 5s WS bridge + 5s endpoint idle），见 §8 | 有界停机语义 |
| 超时 | 首请求 watchdog + 请求超时（`Server.FirstRequestTimeoutSeconds`/`RequestTimeoutSeconds`） | QUIC idle timeout + 同名的首请求 deadline 与请求体/分发 deadline | 同名配置项（h3 读取 engine 配置） |
| 升级 | `101+Upgrade` → `OnUpgrade` | extended CONNECT / WebTransport session | `IUpgradeSession::run` 不变 |
| mTLS | rustls client cert → SHA-256 头（只允许 TLS 终结器写） | 同 rustls 验证 + 同一头 | `CLIENT_CERT_FINGERPRINT_HEADER` |
| 准入 | 每 TCP 连接 1 permit | 每 QUIC 连接 1 permit | 全局 governor |

**h1 的 `101 + Upgrade` 是 hyper 传输层的握手细节。** HTTP/3 的 WebSocket 由 h3 适配器把
RFC 9220 extended CONNECT 改写为标准 upgrade 形态（含缺失时合成 `sec-websocket-key`、把
engine 的 `101` 翻译为 extended CONNECT 的 `200`），因此 `crates/host/src/websocket.rs` 不需要
感知传输协议。更彻底的「声明升级 + 传输完成握手」两段式属于后续
transport-neutral 握手协商，会话实现本身不变。

---

## 5. 实时面复用

实时面已经是「字节流 + 会话」分层，H3 不需要新会话模型：

- **WebSocket over h3**：extended CONNECT 后把双向流映射为 `UpgradedTransport`，直接交给现有
  `IUpgradeSession::run`；`IConnection`、`IConnectionRegistry`、`WebSocketLimits`、presence
  观察者全部复用。
- **WebTransport**：bidi/unidi streams + datagrams 无法无损映射成单个 `AsyncRead + AsyncWrite`。
  当前交付 **adapter-level handler 端口**：`H3Config::with_webtransport_handler` 把具体
  `WebTransportServerSession` 交给应用，`IWebTransportHandler::authorize(&info)` 是 pre-accept
  钩子（**fail-closed：默认拒绝**；失败 → 403，先于任何 `200`）。transport-neutral 的 `IWebTransportSession` 端口
  （engine trait + host 路由/DI/指标）尚未建立；**不**改造 `IUpgradeSession` 去承载 datagram。
- **断线重放**：`IRealtimeEventStore` 以 subscription + cursor 为键，与传输无关；
  `truncated` 语义不变（`crates/core/src/realtime/event_store.rs`）。
- **`TransportKind` 不在 2.x 扩展**：`webx_core::realtime::TransportKind` 是普通 `pub enum`，
  加 `Http3`/`WebTransport` 变体会破坏下游 exhaustive match；补 `#[non_exhaustive]` 同样是
  breaking。传输标签在 engine 内部用 `&'static str`，公开枚举随下一个 major 处理。

---

## 6. 传输后端选型

当前采用 **quinn + rustls + h3**；quiche 仅作备选（QNS 级验证或 BoringSSL 合规要求触发），
不双栈。判据来自本仓库约束：32 个发布 crate 精确钉版本、依赖序发布、三平台 CI、MSRV 1.95、
`unsafe_code = "forbid"`。

| 维度 | quiche（Cloudflare） | quinn + h3 |
|---|---|---|
| 实现形态 | Rust API + BoringSSL（C/C++ FFI） | 纯 Rust（quinn-proto/quinn） |
| TLS | 自带 BoringSSL，策略随 quiche 版本 | rustls，与仓库既有 TLS 栈一致 |
| 构建链 | C 工具链 + cmake（Windows 还需 nasm） | 纯 Rust，跨平台 CI 轻 |
| IO 模型 | sans-IO，调用方驱动 timer/socket | tokio 集成，`quinn::Endpoint` 自带驱动 |
| 协议能力 | 自带 h3/QPACK，成熟度高 | HTTP/3 在独立 h3/h3-quinn crate |
| 验收工具 | qlog、h3i、QNS | qlog 走社区 crate；h3i 可做客户端互操作 |
| 安全审计 | `cargo audit` 不覆盖 C 依赖 | 全 Rust 依赖树 |

HTTP/3 适配层依赖 **MIT h3 fork**：`trev-h3`/`trev-h3-quinn`/`trev-h3-webtransport` 精确钉
`=0.3.0`（传递 `trev-h3-datagram 0.3.0`），补上上游 `h3 0.0.8` 无法表达的 RFC 9220
`:protocol: websocket`。fork 隔离在 `rust-webx-h3` 内：核心/umbrella 不链接；RustSec advisory
按 crate 名匹配，`trev-*` 不会命中上游 `h3`/`h3-quinn` 的 advisory，需要人工跟踪上游修复与
fork 维护。下游缺口判定与互操作基线（quiche-client / h3i / h3spec）见
[upstream-gaps.md](upstream-gaps.md)。

---

## 7. 2.x 公开面约束（红线）

1. **应用公开面零变化**：`webx_core`/`webx_host` 的 HTTP/实时契约、`IRequest`/`IMiddleware`/
   `IHostedService`、DI 宏、`AppOptions`、`Error`/`problem+json`、路由/绑定/校验/授权语义不变。
2. **engine 只加性**：`EngineContext::new`/`finalize` 保留签名与行为（成为 hyper 适配器）；
   新增 `from_transport`/`finalize_transport` 与 `transport` 模块。以下公开形状不因传输扩展而改：
   - `TransportKind` 不加变体（breaking）；
   - `ConnectionMeta` 不加传输专属 pub 字段；
   - `Listener` 不加字段；`App.Urls` 的 `http/https` 语义不变；HTTP/3 走独立 crate +
     `HostBuilder::use_http_transport`；
   - `RespBody` 不换公开类型；中立响应类型只在 `transport` 模块。
3. **默认构建不变**：HTTP/3 是独立可选 crate（不新增 `webx` feature）；默认依赖树不含
   quinn/h3；MSRV、`unsafe_code = "forbid"`、三平台 CI 在引入依赖时同批验证。
4. **旧路径先证明零回归**：hyper 路径的 keep-alive/h2/SSE/WS/排空/alloc/p99 门禁先全绿，
   才允许新传输后端接入。
5. **公开形状被迫改动时，该改动降级到下一个 major**，当前 minor 只发布已验证的加性部分。

---

## 8. 已知边界

- **h3 请求体在 neutral 端口缓冲**：上限 `H3Config::with_max_request_body_bytes`（默认 8 MiB）；
  流式上传不适用于 h3。该上限独立于 `App.MaxBodySize`/`Form.MaxRequestSize`，因此 8–256 MiB 的
  上传在 h1/h2 可能成功、在 h3 会得到 413；响应流式与 SSE 不受影响。
- **h3 不使用 worker 池**：`Server.Workers` 只影响 HTTP/1.1+2；h3 每连接/stream 的并发由
  h3 stream 上限（默认 16 bidi / 16 uni，可配）与全局准入共同约束。
- **h3 排空预算固定**：在途请求 10s + active WS bridge 5s + endpoint idle ≤5s，内部常量；
  `IHttpTransport::stop_budget()` 向 Host 声明该最坏值（h3 = 60s），Host 按后端预算有界等待。
  mTLS 与排空预算尚未进 `appsettings` 配置节（当前用 `H3Config::with_client_auth` 等显式配置）；
  QUIC keep-alive / idle timeout / stream limits 已是 `H3Config` 字段。
- **WebTransport datagram 无 e2e**：API 已暴露，e2e 只覆盖 client-opened bidi stream echo 与
  `authorize` 拒绝路径。
- **WS over h3 的 host 关闭路径无专门 e2e**：排空会等待 active bridge 会话（≤5s），但
  host 关闭时的 WS close frame 尚不宣称与 h1/h2 等价；鉴权/限流走 host 同一中间件路径，
  但无单独 e2e。
- **0-RTT 保持关闭**（`max_early_data_size = 0`）：需先完成幂等重放设计。
- **h3spec 的失败项属于 pinned fork 基线**：网络互操作以 h3spec/h3i/quiche-client 为回归
  基线，不宣称与 h1/h2 完全对等。
- **连接风暴/soak 的 h3 变体未建立**：QUIC 无法在握手前写 503，准入拒绝以关闭连接表达。

---

## 9. 风险

| 风险 | 触发条件 | 缓解 |
|---|---|---|
| semver 破坏 | `EngineContext`/`TransportKind`/`ConnectionMeta`/`Listener` 公开形状被改 | §7 红线；semver-checks 基线门禁；改动降级到下一个 major |
| 双路径漂移 | 新传输与 hyper 路径各写一份 pipeline/错误处理 | 单一 pipeline/错误写入；fake 与 hyper parity 契约测试 |
| alloc/p99 退化 | 端口抽象把动态分发带进每请求路径 | 端口只在连接/构造层；hyper 请求体一次装箱、无逐帧锁；alloc-probe + wire p99 门禁 |
| 依赖/发布顺序 | 新增 h3 适配 crate | 独立 crate 按精确钉版本 + `check-publish-order.py` 发布，不进 core |
| 原生构建链 | 选 quiche | 选 quinn + rustls；三平台 CI 先验证 |
| 上游停更 | `trev-*` fork 停止维护 | 精确钉版 + 本地回归；跟踪上游 h3 进展，可评估切回 |
| MSRV 漂移 | quinn/h3 的 MSRV 高于 1.95 | 选型时 `cargo +1.95.0 check`；必须抬高时显式升 MSRV 并公告 |
| QUIC CPU/吞吐 | 用户态加密、UDP 无 GRO/TSO | 需求驱动、基准先行；只承诺弱网/移动端收益 |
| 运维 | UDP 被防火墙/LB 拦截、Alt-Svc 缓存 | h3 默认不链接；Alt-Svc 仅显式配置时通告；保留 h1/h2 回退 |

---

## 10. 决策记录

| # | 决策 | 理由 |
|---|---|---|
| T1 | 端口边界切在 engine ↔ 线上协议 | 应用面已传输无关；hyper 是当前唯一协议栈（§1） |
| T2 | 单层端口：监听后端（`IHttpTransport`）+ 每请求入口（`serve_transport_request`） | 真实非 hyper 后端都是多路复用的；连接级 trait 只会有测试实现，发布它等于冻结一个无生产者的 API（§2） |
| T3 | hyper 路径保持直出，不做表示层回环 | 零回归 + hyper 响应路径无新增动态分发；中立端口只服务非 hyper 后端（§2） |
| T4 | `serve_stream` 保持「h1/h2 自定义字节流」定位，不冒充 H3 | 它复用 hyper 协议栈；H3 需要 QUIC/QPACK（§1） |
| T5 | 准入沿用全局 governor；QUIC 连接 = 1 permit，stream 并发单独限制 | `MaxConnections` 语义不因协议放大（§4） |
| T6 | WebSocket 复用 `IUpgradeSession`；WebTransport 用 adapter-level handler 端口 | 字节流升级可复用；datagram 不能无损映射为 `AsyncRead+AsyncWrite`（§5） |
| T7 | HTTP/3 作为独立可选 crate 交付（不新增 `webx` feature） | 默认构建、MSRV、alloc/p99、三平台 CI 不受影响（§7） |
| T8 | `TransportKind` 在 2.x 不扩展 | 普通 `pub enum` 加变体是 breaking（§5） |
| T9 | 选 quinn + rustls + h3，quiche 仅备选 | 纯 Rust 依赖链、三平台 CI、rustls 已在栈内（§6） |
