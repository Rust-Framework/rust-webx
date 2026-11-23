# 传输端口（Transport Port）设计

> 状态：设计稿，基线 rust-webx 2.8.0。**2.x 只交付本设计，不引入 QUIC/HTTP/3 运行时代码**；
> 真正的 HTTP/3 服务端排入 3.0，或由真实需求触发（见 §7）。取舍依据：
> [框架设计准则](framework-constitution.md) §6「2.x 只做加性变更」与
> [兼容性](compatibility.md)。现有 TCP/hyper 生命周期以
> [请求生命周期](../rust-webx/04-architecture/request-lifecycle.md) 为权威落点，本文不重复描述当前行为。

---

## 1. 现状：传输能力全部收敛在 engine，但收敛到了 hyper

以下事实均来自当前仓库代码，是端口设计的边界条件：

| 层 | 当前实现 | 位置 |
|---|---|---|
| 监听 | `Listener { listener: std::net::TcpListener, tls: Option<Arc<rustls::ServerConfig>> }`；`Listener::plain` / `Listener::tls`；每个 listener 一个 acceptor | `crates/engine/src/server.rs` |
| 连接交接 | `TcpListener::accept()` → 全局准入 governor → `stream.into_std()` → 有界 `mpsc` → worker `TcpStream::from_std()` | `server.rs`、`worker.rs` |
| 协议栈 | 每 worker 一个 hyper `auto::Builder`（HTTP/1.1 + HTTP/2）、`serve_connection_with_upgrades`；TLS 由 `tokio_rustls::TlsAcceptor` 在 worker 内握手（10s 超时） | `worker.rs` |
| 请求头 | 引擎持有 `hyper::Request<Incoming>`，拆成 `http::request::Parts` + `hyper::body::Incoming` | `crates/engine/src/context.rs` |
| 请求 body | `BodyState::Pending(Option<Incoming>)`；`body::read_body` / `stream_body` / `parse_multipart` 都以 `Incoming` 为输入 | `context.rs`、`crates/engine/src/body.rs` |
| 响应 | `EngineContext::finalize(self) -> hyper::Response<RespBody>`；`RespBody = UnsyncBoxBody<Bytes, io::Error>`；流式响应经 `StreamBody(ReaderStream)` 以 64 KiB 帧写出 | `crates/engine/src/response.rs` |
| 升级 | `upgrade: Option<hyper::upgrade::OnUpgrade>`；`finalize` 里等 `OnUpgrade`，包成 `hyper_util::rt::TokioIo` 后交给 `IUpgradeSession::run(UpgradedTransport, ConnectionMeta)` | `context.rs`、`crates/core/src/realtime.rs` |
| 排空 | 每个连接 `graceful_shutdown()`；预算 `Server.DrainTimeoutSeconds` / SSE 的 `Server.StreamDrainTimeoutSeconds`，SSE 连接由 `ActiveStreamGuard` 单独计数 | `worker.rs`、`shared.rs` |

两条已经存在的「传输中立」事实，决定端口该切在哪：

1. **应用面早已传输无关。** 中间件/Handler 只看到 `IHttpContext` / `IHttpRequest` /
   `IHttpResponse`、`RequestBodyStream`、`ResponseBody`/`FileBody`/`FileSource`
   （`webx_core::http`），实时面只看到 `IUpgradeSession` / `UpgradedTransport` /
   `IConnection`（`webx_core::realtime`）。因此新端口应切在 **engine ↔ hyper**，
   而不是再包一层应用契约。
2. **2.8.0 已有字节流级逃生舱。** `Server::serve_stream<I>(&self, io, peer)` 接收任意
   `AsyncRead + AsyncWrite` 流，复用生产 hyper auto 配置、路由、中间件、错误写入与排空，
   但不占准入槽、不置单次运行标志（`crates/engine/src/server.rs`；`TestHost::connect_wire`
   用它做无端口 wire 测试，见 [TestHost](test-host.md)）。它解决的是「换字节流来源」，
   不解决「换帧协议」——HTTP/3 的 QUIC 传输与 QPACK 不在 hyper 能力内，所以
   `serve_stream` 不能直接承载 H3，但它是 `IHttpTransport` 之前的第一个加性接缝。

---

## 2. 缺口：为什么不能再加一个 `serve_stream` 就够

| 维度 | 当前 | H3/WebTransport 需要 | 是否已有中立类型 |
|---|---|---|---|
| 监听 | TCP `std::net::TcpListener` + rustls accept | UDP socket + QUIC endpoint + ALPN `h3`；`App.Urls` 只有 `http/https` scheme | 否 |
| 请求 head | `hyper::Request<Incoming>` / `http::request::Parts` | `Parts`（method/uri/headers）可复用，body 类型不可 | `http::request::Parts` 可复用 |
| 请求 body | `hyper::body::Incoming` 贯穿 `BodyState` 与 `body.rs` | 任意字节帧流 | `webx_core::http::RequestBodyStream` 已存在 |
| 响应 body | hyper `Response<RespBody>` + `RespBody` 具体类型 | h3 send stream（`send_data` 帧 + 完成语义） | `ResponseParts` / `ResponseBody` 已中立 |
| 升级 | h1 `101 + Upgrade` → `OnUpgrade` | h3 extended CONNECT（RFC 9220）、WebTransport（RFC 9297） | `IUpgradeSession`/`UpgradedTransport` 已中立，握手语义未中立 |
| 连接生命周期/排空 | hyper `graceful_shutdown` | h3 GOAWAY + 在途 stream 等待；QUIC `CONNECTION_CLOSE` 兜底 | `drain_timeout`/`stream_drain_timeout` 可复用 |
| 准入 | 每个 TCP 连接 1 个 permit（`ConnectionPermit`） | 每个 QUIC 连接 1 个 permit；stream 并发上限是另一维度 | `AdmissionStats`/`ServerStats` 可复用 |
| 指标/日志 | 连接级 gauge 无 transport 标签 | 需要 `transport="h1|h2|h3"` 维度 | `TransportKind` 公开枚举（扩展属 breaking，见 §7） |

---

## 3. 端口草案

### 3.1 两层端口：监听/连接 vs 请求

HTTP/1.1 是「连接 ≈ 请求序列」，HTTP/2、HTTP/3 是「连接 = 多路复用 stream」。端口必须
把这个差异留在传输内部，engine 只看到「一条连接，按顺序或并发地提交请求/响应」：

```rust
/// 一个监听端点上的传输后端：负责 accept、协议握手与连接级排空。
///
/// 2.x 不新增该 trait；此处是 3.0 的契约草案。
pub trait IHttpTransport: Send + Sync {
    /// 稳定标签，进指标/日志：`"h1"` / `"h2"` / `"h3"` / `"tls+h2"`。
    fn kind(&self) -> &'static str;

    /// 在 `runtime.shutdown` 触发后停止 accept，并排空本端点上已建立的连接。
    fn run(&self, runtime: TransportRuntime) -> BoxFuture<'static, Result<()>>;
}

/// 引擎交给传输后端的运行时句柄（2.x 内部结构，3.0 公开）。
pub struct TransportRuntime {
    /// 路由、中间件、DI、配置与关闭信号都在这里。
    pub shared: Arc<Shared>,
    /// 全局准入：每个 TCP/QUIC **连接**占一个 permit，多 listener/多传输共享。
    pub governor: Arc<Semaphore>,
    pub shutdown: watch::Receiver<bool>,
}
```

```rust
/// 一条已建立的传输连接（HTTP/1.1、HTTP/2 或 HTTP/3 connection）。
#[async_trait]
pub trait IRequestTransport: Send {
    /// 取下一个请求（head + body 流 + 可能的升级）；`None` = 连接结束。
    async fn accept_request(&mut self) -> Result<Option<TransportRequest>>;

    /// 提交响应并驱动写出；返回时该请求的字节已交给传输层（背压体现在 await）。
    async fn respond(&mut self, request: RequestId, response: TransportResponse) -> Result<()>;

    /// h1：不再复用连接；h2/h3：发送 GOAWAY 并等待在途 stream。
    fn graceful_shutdown(&mut self);

    /// 连接计数/耗时等传输层状态，供 `/metrics` 与日志使用。
    fn stats(&self) -> TransportStats {
        TransportStats::default()
    }
}
```

```rust
/// 传输中立请求：head 用 http crate，body 用 core 已有的帧流，升级用 future。
pub struct TransportRequest {
    pub parts: http::request::Parts,
    pub peer: Option<SocketAddr>,
    pub body: RequestBodyStream, // webx_core::http::RequestBodyStream
    /// h1：Upgrade 后的字节流；h3：extended CONNECT / WebTransport 会话。
    pub upgrade: Option<TransportUpgrade>,
}

pub struct TransportUpgrade {
    /// 协议名：`"websocket"` / `"webtransport"`。
    pub protocol: &'static str,
    /// 响应提交后解析；成功时交出的字节流与 `IUpgradeSession::run` 的入参同型。
    pub io: BoxFuture<'static, Result<UpgradedTransport>>,
}

/// 传输中立响应：状态/头/体都是 core 已有类型，传输层负责编码。
pub struct TransportResponse {
    pub status: u16,
    pub headers: http::HeaderMap,
    pub body: ResponseBody, // webx_core::http::ResponseBody::{Bytes, File}
}
```

### 3.2 `EngineContext` 需要的改造

保持 2.8 公开签名不变（3.0 再收口），把内部数据换成中立类型：

| 位置 | 当前 | 改造后 |
|---|---|---|
| `EngineContext::new` | `hyper::Request<Incoming>` 入参 | 保留为 **hyper 适配器**：拆出 `Parts`，把 `Incoming` 映射为 `RequestBodyStream`，把 `hyper::upgrade::on` 映射为 `TransportUpgrade`，再调用 `from_transport` |
| 新构造 | — | `EngineContext::from_transport(request: TransportRequest, limits, dispatch) -> Self`（3.0 公开） |
| `from_parts` | `(parts, body: Bytes, limits, peer, dispatch)` | 签名不变（测试/进程内构造）；内部等价于 `from_transport` 的已缓冲 body 分支 |
| `BodyState::Pending` | `Option<Incoming>` | `Option<RequestBodyStream>`；`body::read_body` / `stream_body` / `parse_multipart` 改为消费帧流（multer 本身吃 `Stream`，改造可行） |
| `upgrade` 字段 | `Option<hyper::upgrade::OnUpgrade>` | `Option<TransportUpgrade>`；`with_upgrade` 是 hyper 适配器的私有入口 |
| `finalize` | 直接产出 `hyper::Response<RespBody>` | 内部先产出 `TransportResponse`；hyper 适配器保留现有 `ResponseParts::assemble`（HEAD/204/304/安全头逻辑不变），h3 适配器把 `ResponseBody` 编码为 h3 数据帧 |

由此新增 engine 运行入口（加性）：

```rust
impl Server {
    /// 2.8 已有：TCP listener 入口保持行为不变。
    pub async fn run_listeners(self, listeners: Vec<Listener>) -> Result<()>;

    /// 3.0 新增：任意传输后端，共享同一 worker pool 与全局准入。
    pub async fn run_transport(self, transport: Arc<dyn IHttpTransport>) -> Result<()>;
}
```

worker 的 `Handoff` 从「`std::net::TcpStream` + TLS 配置」泛化为
「`Box<dyn IRequestTransport>` + permit + 连接 gauge」：worker 在自己的
current-thread runtime 上驱动传输连接的请求循环，但对每个请求仍调用**同一个**
`connection::serve` 管道入口（路由/中间件/错误写入/panic 边界只有一份实现）。
HTTP/3 的 QUIC 事件循环因此落在现有 worker 模型里，不新增线程模型。

---

## 4. 响应流、背压、排空、升级的映射

| 语义 | 当前（TCP + hyper） | H3/WebTransport 映射 | 复用点 |
|---|---|---|---|
| 响应流 | `ResponseBody::File` → `ReaderStream(AsyncRead)`，`STREAM_BUF_SIZE = 64 KiB` 一帧 | h3 `RequestStream::send_data(Bytes)` 逐帧发送，循环仍是同一个 `AsyncRead`；帧完成语义由 `finish()` 显式表达 | `webx_core::http::FileSource`、`STREAM_BUF_SIZE` |
| 背压 | 客户端不读 → hyper poll 不到 frame → `ReaderStream` 不再 poll 源流（`SseResult` 文档承诺的「慢客户端压回源」） | quinn `SendStream` 的容量窗口 → `send_data` 等待；同样体现为源流暂停 | 不变式不变：**不缓存整个响应** |
| 排空 | 连接 `graceful_shutdown()`；普通/SSE 两档预算；SSE 连接由 `ActiveStreamGuard` 计数 | 连接级 GOAWAY → 等待在途 request stream 完成 → 预算耗尽后 `CONNECTION_CLOSE` 兜底；每 stream `RESET_STREAM` | `Server.DrainTimeoutSeconds`、`Server.StreamDrainTimeoutSeconds` 配置语义 |
| 首请求/头部超时 | `first_request_timeout` 看门狗 + hyper h1 `header_read_timeout`/h2 keepalive | QUIC idle timeout + 每请求 head deadline；`first_request_timeout` 语义照旧 | 同名配置项，无需新配置 |
| 升级 | h1 `101` 响应 + `OnUpgrade` → `TokioIo` → `Box<dyn Transport>` | h3 extended CONNECT（WebSocket over h3）/ WebTransport session；握手字节由传输协议决定，不是 h1 的 `101+Upgrade` | `IUpgradeSession::run(UpgradedTransport, ConnectionMeta)` 不变 |
| mTLS | rustls 握手拿 client cert → SHA-256 → `x-webx-client-cert-sha256` 头（只允许 TLS 终结器写） | QUIC TLS 同样由 rustls/BoringSSL 验证 client cert；指纹映射不变 | `CLIENT_CERT_FINGERPRINT_HEADER` |
| 准入 | 每 TCP 连接 1 permit（含 keep-alive/SSE/WS） | 每 QUIC 连接 1 permit；stream 并发用传输层上限限制（类 h2 `SETTINGS_MAX_CONCURRENT_STREAMS`），**不是**乘进 `MaxConnections` | `AdmissionStats` / `ServerStats` / `engine_*` gauge |

一个必须写进契约的细节：**h1 的 `101 + Upgrade` 响应头是 hyper 传输层的握手细节**。
3.0 的 WebSocket 握手需要由传输后端生成（h1 写 101，h3 走 extended CONNECT 的伪头），
`crates/host/src/websocket.rs` 目前直接写 101/`Sec-WebSocket-Accept` 的中间件要改成
「声明要升级 + 交给传输端口完成握手」两段式，会话实现本身不变。

---

## 5. H3/WebTransport 如何复用实时端口

现有实时面已经按「字节流 + 会话」分层，H3 不需要新会话模型：

- **WebSocket over h3**：传输后端完成 extended CONNECT 后，把双向流包成
  `Box<dyn Transport>`，直接交给现有 `IUpgradeSession::run`；`IConnection`、
  `IConnectionRegistry`、`WebSocketLimits`、presence 观察者全部复用。
- **WebTransport**：bidi/unidi streams + datagrams 无法无损映射成单个
  `AsyncRead + AsyncWrite`。3.0 建议新增 `IWebTransportSession`
  （打开 bidi stream、收 datagram、关闭原因），而 **不** 改造 `IUpgradeSession`；
  `IConnection` 之上的消息语义（text/binary/close、连接级统计）可以继续复用，
  由适配器把 stream/datagram 映射成 `RealtimeMessage`。
- **断线重放**：`IRealtimeEventStore` 以 subscription + cursor 为键，与传输无关；
  WS ↔ WebTransport 切换后按 `ResumeRequest` 重放即可，`truncated` 语义不变
  （`crates/core/src/realtime/event_store.rs`）。
- **TransportKind 扩展是 breaking 的**：`webx_core::realtime::TransportKind` 是普通
  `pub enum`（未标 `#[non_exhaustive]`），2.x 加 `Http3`/`WebTransport` 变体会破坏下游
  exhaustive match；现在补 `#[non_exhaustive]` 同样是 breaking。所以该扩展只能随 3.0
  一起做；2.x 的 `/metrics` 新维度放在 engine 内部的 `&'static str` 标签里，不碰公开枚举。

---

## 6. quiche 与 quinn 取舍

结论先行：**3.0 首选 quinn + rustls + h3；quiche 作为备选（QNS 级验证或 BoringSSL 合规
要求触发），不双栈。** 判据是本仓库的既有约束，不是实现偏好：

| 维度 | quiche（Cloudflare） | quinn（+ h3 / h3-quinn） |
|---|---|---|
| 实现形态 | Rust API + C/C++ 的 BoringSSL 经 FFI 链接 | 纯 Rust（quinn-proto / quinn） |
| TLS | 自带 BoringSSL，证书/会话策略随 quiche 版本 | rustls；与当前仓库 `rustls = 0.23`（ring provider）同栈，证书加载/`TlsConfig` 可复用 |
| 构建链 | 需要 C 工具链 + cmake（Windows 还要 nasm）；三平台 CI/发布链新增原生构建 | 纯 Rust 依赖，跨平台 CI 轻；MSRV 跟随依赖，需验证 ≤1.95 |
| IO 模型 | sans-IO：调用方自己驱动 timer/socket 收发包（与 worker 模型契合，但集成代码量更大） | tokio 集成：`quinn::Endpoint` 自带驱动与定时器 |
| 协议能力 | 自带 `h3` 模块 + QPACK，HTTP/3 成熟度高 | HTTP/3 在独立 `h3`/`h3-quinn` crate，版本联动与覆盖率需要评估 |
| 观测/验收工具 | qlog 输出、h3i（HTTP/3 互操作客户端）、QNS（QUIC Network Simulator，均属 quiche 工具链） | qlog 走社区 crate；无 QNS 等价物，可用 h3i 做客户端互操作、netem/toxiproxy 做网络仿真 |
| 安全/审计 | BoringSSL 由 Google 维护，更新节奏跟随 quiche 发布；`cargo audit` 不覆盖 C 依赖 | rustls 全 Rust；`unsafe_code = "forbid"` 只约束本仓库 crate（依赖内部 unsafe 仍存在，但无 C 构建产物） |
| 维护成本 | 原生构建 + FFI 版本矩阵 | 纯 Rust 依赖树，与现状一致 |

选 quinn 的核心理由：仓库是 **21 个 crate 精确钉版本 + 依赖序发布 + 三平台 CI + MSRV 1.95
+ `unsafe_code = "forbid"`** 的纯 Rust 体系（见 [框架设计准则](framework-constitution.md) §3），
引入 BoringSSL/cmake/nasm 会同时改变 `check-publish-order.py`、三平台构建矩阵与安全审计
范围；而 rustls 已经是生产 TLS 栈，证书与 provider 配置可以直接复用。quiche 的优势
集中在互操作与 QNS 级验证，属于「选型第二阶段」的证据来源，不需要同时进依赖树。

---

## 7. 2.x 只加性：哪些变更必须等到 3.0

2.8.0 的交付是**本文档**与既有加性接缝（`Server::serve_stream`），不含任何 QUIC/h3
依赖、配置项或公开传输 API。以下变更都触及 semver 主版本，不能进 2.x：

1. `EngineContext::new` / `finalize` 的签名（改成中立入参/返回值，或加泛型 body）——
   2.x 可以「新增 `from_transport`/`finalize_transport` 并行」，但会造成两套上下文
   构造的概念税；3.0 一次性收口。
2. `TransportKind` 增加 `Http3`/`WebTransport` 变体（enum 未 `#[non_exhaustive]`；
   现在补标注本身也是 breaking）。
3. `ConnectionMeta` 增加传输专属字段（pub 字段 + 结构体字面量构造是公开用法）。
4. `Listener` / `App.Urls` 的协议语义扩展：HTTP/3 通常复用 `https` URL + `Alt-Svc`
   通告 + 独立 UDP 端口，需要新的配置契约，不能偷偷塞进现有 `Listener`。
5. 准入语义从「每 TCP 连接」明确为「每 QUIC 连接 + stream 并发上限」——可加性实现，
   但指标/文档契约变化，应与 3.0 一起公告。
6. `RespBody`（`UnsyncBoxBody<Bytes, io::Error>`）等 engine 公开响应类型收口。

因此：**真正的 HTTP/3 服务端 = 3.0 里程碑，需求驱动**。触发条件建议写进里程碑
（大规摸实时推送、弱网/移动端、明确的多路复用收益），而不是「上游有了就做」。
3.0 之前 2.x 只做已验证的加性预留：不新增依赖、不改本文件列出的公开面。

---

## 8. 验收清单（3.0 实现时逐条勾选）

**端口契约**

- [ ] `IHttpTransport` / `IRequestTransport` 有 ≥2 个实现（hyper h1/h2 适配器、quinn h3
      适配器）+ 一个进程内 fake，且 fake 与真实实现跑同一组契约测试（原则 P9）。
- [ ] 端口只在连接/构造层做动态分发；请求热路径不新增 `Box<dyn ...>` 分配
      （对照 [分配预算](alloc-budget.md) 的框架税口径）。
- [ ] `Server::run_listeners`（TCP）与 `Server::serve_stream`（字节流）行为、错误文案、
      指标名不变；新增 `Server::run_transport` 是加性入口。

**功能与回归**

- [ ] `cargo test -p rust-webx-engine`、`cargo test -p rust-webx-host` 全绿，
      `TestHost::connect_wire` 的 h1/h2 wire 测试不需要修改。
- [ ] h3 端到端：GET/POST（含 `#[max_body_size]` → 413）、SSE 帧与慢客户端背压、
      多 stream 并发、GOAWAY 排空、WebSocket over h3（extended CONNECT）、
      WebTransport echo（streams + datagrams）、mTLS 指纹头。
- [ ] 同类准入/排空场景在 h3 上复用现有断言：连接风暴 503、graceful shutdown 无 5xx、
      超时熔断、panic 隔离、超大 body 后连接仍可用（见 [稳定性门禁](stability-gates.md) §2）。

**门禁**

- [ ] `cargo clippy --workspace --all-targets -- -D warnings`、`cargo +1.95.0 check --workspace --locked`。
- [ ] `bash scripts/check-docs.sh`（链接/INDEX/版本一致）。
- [ ] `cargo semver-checks --package rust-webx-core --baseline-rev <上一个 2.x tag>` 等四个包通过。
- [ ] `cargo run -p alloc-probe --release --locked -- compare 500 12`：h1/h2 框架税不退化。
- [ ] `WIRE_P99_BUDGET_MS=5 cargo test --release -p rust-webx --test wire_p99_test -- --ignored`：
      h1/h2 p99 不退化；h3 单独设 steady-state 预算（QUIC 握手成本不计入稳态 p99）。
- [ ] stability：soak（QUIC 连接 RSS）、5 个 chaos 场景 + fuzz-lite 增加 h3 变体。

**安全**

- [ ] QUIC 只启用 TLS 1.3、ALPN 固定 `h3`；证书/client CA 复用
      `crates/engine/src/tls.rs` 的加载路径与 fail-closed 校验。
- [ ] 0-RTT 默认关闭；若开启，只允许幂等读路径或经幂等端口保护（重放风险写入安全文档）。
- [ ] `Alt-Svc` 只在显式配置时通告；UDP 端口/防火墙/LB 前置条件写入部署文档。
- [ ] 客户端证书指纹只来自传输层，沿用 `CLIENT_CERT_FINGERPRINT_HEADER` 的「先删后写」。

**可观测与文档**

- [ ] `/metrics` 增传输维度指标（加性）：`engine_transport_connections_active{transport=...}`、
      QUIC handshake 失败计数、GOAWAY 计数；`ServerStats` 已 `#[non_exhaustive]`，可加字段。
- [ ] [指标](../rust-webx/11-production/metrics.md) 表、[优雅停机](../rust-webx/11-production/graceful-shutdown.md)、
      [WebSocket 会话](../rust-webx/05-request-pattern/websocket.md)、`CHANGELOG.md`、
      `docs/design/upstream-gaps.md` 同批更新；能力矩阵与审计覆盖同步。

---

## 9. 风险

| 风险 | 触发条件 | 缓解 |
|---|---|---|
| semver 破坏 | `EngineContext`/`TransportKind`/`ConnectionMeta`/`Listener` 公开形状 | 全部排入 3.0；2.x 不承诺 h3；semver-checks 基线门禁 |
| 原生构建链 | 选 quiche（BoringSSL + cmake + nasm） | 选 quinn + rustls；三平台 CI 先验证；不把原生依赖塞进 2.x 发布链 |
| 依赖/发布顺序 | 新增 h3 适配 crate | 独立 crate（如 `rust-webx-h3`）按精确钉版本 + `check-publish-order.py` 发布，不进 core |
| alloc/p99 退化 | 端口抽象把动态分发带进每请求路径 | 端口只在连接/构造层；热路径保持具体类型；alloc-probe + wire p99 同批门禁 |
| QUIC CPU/吞吐 | 用户态加密、UDP 无 GRO/TSO | 需求驱动、基准先行；不承诺「比 h1/h2 快」，只承诺弱网/移动端收益 |
| 安全 | 0-RTT 重放、QUIC amplification、BoringSSL 漏洞节奏 | 0-RTT 默认关；anti-amplification/TLS 1.3 强制；审计与 RustSec 覆盖说明 |
| 运维 | UDP 被防火墙/LB 拦截、Alt-Svc 缓存 | h3 默认关闭且显式配置；保留 h1/h2 回退；回退策略写入部署文档 |
| 维护成本 | 双协议栈 + WebTransport 流/数据报新端口 | 端口契约测试 + 单一 pipeline；quiche/quinn 不双选；WebTransport 端口独立评审 |
| MSRV 漂移 | quinn/h3 的 MSRV 高于 1.95 | 选型时 `cargo +1.95.0 check` 验证；必须抬高时随 3.0 公告 |
| 验收工具缺口 | 选 quinn 后没有 QNS | h3i 做客户端互操作 + netem/toxiproxy 做网络仿真；QNS 结果作为 quiche 备选阶段的证据 |

---

## 10. 决策记录

| # | 决策 | 理由 |
|---|---|---|
| T1 | 2.8.0 只交付设计，不实现 HTTP/3 | 2.x 加性约束；公开形状收口是 3.0 范围（§7） |
| T2 | 端口边界切在 engine ↔ hyper | 应用面已传输无关；hyper 是当前唯一协议栈（§1） |
| T3 | H3 复用 `IUpgradeSession`/`IConnection`/`IRealtimeEventStore`，WebTransport 新增 stream/datagram 端口 | 字节流升级可复用；datagram 无法无损映射为 `AsyncRead+AsyncWrite`（§5） |
| T4 | 3.0 首选 quinn + rustls + h3，quiche 仅作备选 | 纯 Rust 依赖链、三平台 CI、rustls 已在栈内（§6） |
| T5 | `Server::serve_stream` 保持「h1/h2 自定义字节流」定位，不冒充 H3 | 它复用 hyper 协议栈；H3 需要 QUIC/QPACK（§1） |
| T6 | 准入沿用全局 governor；QUIC 连接 = 1 个 permit，stream 并发单独限制 | `MaxConnections` 的语义保持「连接上限」不被协议放大（§4） |
