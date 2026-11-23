# TestHost：进程内测试基座

## 目标

让每个框架能力都有**不依赖端口**的测试形态：不起 TCP 服务、不做 HTTP 编解码，直接
通过真实的 HandlerCache + Mediator + DI scope 路径分发请求；时间可控、故障可注入、
运行时确定（current-thread / paused time）。

## API

```rust
use webx::{TestHost, TestClock, IClock, Host};

let clock = Arc::new(TestClock::from_unix(1_700_000_000));
let host = TestHost::from_builder_with_clock(
    Host::builder().register(|svc| svc.add_mediator()),
    Arc::clone(&clock),
);

// 匿名分发（走与 HTTP 相同的 dispatch::dispatch 路径）
let reply = host.dispatch(MyRequest { .. }).await?;

// 带身份（RequestContext::identity() 在 handler 内可见）
let reply = host.dispatch_as(identity, request).await?;

// 可控时钟
clock.advance(Duration::from_secs(60));

// 故障/延迟注入
host.chaos().fail_next(1);                       // 下一次 dispatch 返回 typed 503
host.chaos().delay_next(Duration::from_millis(20)); // 下一次 dispatch 先睡 20ms

host.shutdown();                                  // 停止 hosted services 并 dispose provider
```

| 能力 | 说明 |
|---|---|
| `TestHost::from_builder(builder)` | 构建 host（不 bind 端口），注入默认 `TestClock` |
| `TestHost::from_builder_with_clock(builder, clock)` | 显式可控时钟 |
| `TestHost::new(host)` | 包装已构建的 host（使用其注册时钟） |
| `dispatch` / `dispatch_as` | 经 `Mediator::send` 分发，每次一个 DI scope（Scoped 服务按次新建） |
| `connect_wire()` | 打开一条**无端口**的内存双端 wire 连接，由真实 engine/hyper 处理字节 |
| `clock()` | 返回 handler 注入的同一个 `Arc<dyn IClock>` |
| `chaos()` | `fail_next(n)` / `delay_next(d)` / `reset()` / `dispatched()` |
| `shutdown(self)` | 同 `Host::shutdown` |

## Wire 仿真（`connect_wire`，2.8.0）

`dispatch` 覆盖 Handler/DI/Mediator 层，但无法表达 keep-alive、HTTP/2、SSE 帧字节、
畸形请求这些**协议字节级**行为。`connect_wire` 用 `tokio::io::duplex` 在进程内打开一对
管道，客户端拿到可读写的字节流，服务端交给当前 Host 的 engine `serve_stream`：走的仍是
生产用的 hyper auto builder、路由、中间件、错误写入器和排空逻辑，但不 bind 端口、不经过
内核 socket，因此可重复、可注入、无端口冲突。

```rust
let host = TestHost::from_builder(Host::builder().mode(AppMode::Development).no_spa());
let mut conn = host.connect_wire().await;
conn.write(b"GET /hello HTTP/1.1\r\nhost: test\r\nconnection: close\r\n\r\n").await?;
let response = conn.read_at_most(64 * 1024).await?;
assert_eq!(status_of(&response), 200);
conn.close().await; // 等待 engine 连接任务结束
```

覆盖方式：

| 测试目标 | 做法 |
|---|---|
| HTTP/1.1 keep-alive | 同一 `WireConnection` 连续写两个请求，按 `Content-Length` 读两个响应 |
| h2c prior knowledge | `WireConnection { stream, task } = conn`，把 `stream` 交给 `hyper::client::conn::http2::handshake` |
| 畸形请求/恢复 | 写非法字节，断言 400 或连接关闭；再开一条 wire 连接断言仍可服务 |
| SSE 字节 | 带 `accept: text/event-stream` + `connection: close`，读到 EOF 断言行格式 |
| 超时/排空 | 注入延迟后 `close()`/`shutdown()`，断言连接任务在预算内结束 |

参考实现：`crates/webx/tests/wire_harness_test.rs`；engine 入口
`crates/engine/src/server.rs` 的 `Server::serve_stream`。


## 时钟端口（`webx::IClock`）

- 生产：`SystemClock`（Host 默认注册；`HostBuilder::use_clock` 可替换）；
- 测试：`TestClock` 固定起点，`advance`/`set` 才移动，`now_unix`/`now_unix_millis` 无墙钟漂移；
- Handler 通过 `#[inject] clock: Arc<dyn IClock>` 注入，**不要**直接 `SystemTime::now()`；
- 超时/重试/退避等**单调时间**不走 `IClock`：仍是 tokio 定时器，用
  `#[tokio::test(start_paused = true)]` 确定性推进。

## 覆盖面与边界

**零端口可测**：Handler 构造与依赖注入、typed pipeline behavior、Mediator 分发、
Scoped 生命周期、请求身份上下文、时钟、确定性故障注入。

**仍需 HTTP 路径**：路由匹配、参数绑定、状态码/响应头、CORS/压缩/限流等字节级行为——
用 `webx::spawn`（`TestServer`，临时端口 + 健康就绪）覆盖。

**WS 路径**：消息即请求本身走 Mediator，可用 TestHost 直接分发 `ImRequest` 验证
handler/connection 上下文；升级握手与帧行为用现有 WS 端到端测试。

## 确定性运行时建议

```rust
#[tokio::test]                              // current-thread：无跨线程调度抖动
#[tokio::test(start_paused = true)]          // 需要 tokio 定时器时自动推进虚拟时间
```

## 验收

`crates/webx/tests/test_host_test.rs`（5 项）：零端口分发、身份上下文、可控时钟
（前进 90s 无漂移）、Scoped 每 dispatch 新鲜、chaos 注入 503/延迟且 handler 不被执行。
实现：`crates/host/src/testhost.rs`、`crates/core/src/time.rs`。
