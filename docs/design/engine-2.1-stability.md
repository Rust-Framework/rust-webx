# Engine 2.1 — 生产稳定性收口设计

> 状态：`stability/2.1` 分支实现记录。目标不是再堆功能，而是把 2.0 引擎的
> **准入、生命周期、错误语义和验证**提升到长期运行标准。

## 1. 准入模型：一个全局 governor + 共享 worker pool

2.0 的 per-worker `MaxConnections / workers` 预算在 `workers > max` 时会放大
上限，而且 Host 每个 URL 启一套 worker pool，http+https 会成倍放大线程和连接。
2.1 改为：

```text
Host::run_on
  └── one webx_engine::Server
        ├── one global Semaphore(App.MaxConnections)
        ├── one worker pool (Server.Workers, default available parallelism)
        └── N acceptor tasks (http and https listeners, https carries its own
            Arc<rustls::ServerConfig> in Listener)
```

- 连接在 accept 后、交给 worker 前获取全局 permit；permit 随 `Handoff` 移动，
  连接任务结束/被丢弃时释放。没有 per-worker floor，也没有多 listener 乘法。
- per-worker channel 仍用于负载均衡和背压，但不再承担连接上限。
- `Router::try_insert` 让 Host 在重复/非法路由（包括内置 `/health` 冲突）时
  **启动失败**，而不是静默保留先注册者。

## 2. 首请求期限与 HTTP/2 idle

`auto::Builder` 在嗅探 h2 preface 前没有 h1 header timer；只发 TCP 不发字节的
连接会永久占用 permit。2.1 在 worker 的 connection future 上增加：

- 首请求 watchdog：service closure 第一次被调用时 `Notify`；在
  `Server.FirstRequestTimeoutSeconds`（默认 10，0 回退到默认）内没有请求则
  关闭连接、释放 permit。
- h2 `keep_alive_interval` / `keep_alive_timeout` 使用 header timeout，避免
  upgrade 后空闲 h2 连接永久占槽。
- TLS 握手仍独立 10s，并 race shutdown；握手失败释放 permit。

## 3. 生命周期：单次运行、可取消、可安全 dispose

- **Host**：Built → Running → Disposed/Failed 原子状态；并发/重复 `run` 返回
  typed `Error::State`。启动失败回滚 hosted services 后仍可重试；运行期错误
  不 dispose DI（可能仍有阻塞 worker）。
- **HostRunGuard**：`run` future 被 drop/abort 时设置 host watch/token、触发引擎
  immediate shutdown；signal listener / shutdown bridge / hosted watcher 都有
  明确生命周期。
- **ServerRunGuard**：引擎 run future 被丢弃时设置 `immediate_shutdown`，worker
  在 drain 中立即 abort 连接，而不是继续服务 300s。
- **反向 shutdown bridge**：引擎内部失败先取消 host watch，hosted services 在
  连接 drain 前停止。
- `Shared.started` 防止同一个 engine state 被第二个 `Server` 复用。

## 4. 错误响应语义

- Pipeline 新增 `execute_with_error`：forward/terminal 失败先写入 context，
  **再执行逆序 after hooks**；metrics/tracing/compression 看到真实状态。
- 单一错误 writer 先 strip `cache-control`/`content-encoding`/`content-length`/
  `content-range`/`content-disposition`/`etag`/`last-modified`/
  `accept-ranges` 等表示头，保留 `Allow`/CORS/`Vary`；416 的
  `Content-Range: bytes */len` 例外保留。
- 文件响应恢复默认策略：有 validator → `public, max-age=0, must-revalidate`；
  无 validator 的流 → `no-store`；byte/None → `no-store`（304 除外）。
- 204/1xx 不再带 `Content-Length`，204/304 永不发送 body。
- 自定义 `IErrorResponseWriter` panic 被请求边界捕获并回退固定 500。

## 5. 流与 worker

- SSE drain 计数改为 **per-worker**：其他 worker 的 SSE 不会让本 worker 的
  普通连接等待 `StreamDrainTimeoutSeconds`。
- `SseBodyStream` 轮询一个 `cancelled_owned()` future；quiet stream 也会被
  shutdown token 唤醒并写出 `event:server_shutdown`。
- `active`/`pending` 计数改为 RAII guard：abort/panic 不泄漏；channel-full
  路径不再有 send/recv 双计数竞态。

## 6. 绑定与 handler 分工

- JSON binder 在读取 body 前校验 media type：非 `application/json`/`+json`
  返回 415（空 body/未声明 content-type 保持兼容）。
- 删除构造期全局 `Content-Length` 预检；在路由解析后由 route target 设置
  `#[max_body_size]` 再校验，因此 route 可以**收紧或放宽**全局上限，404/405
  路径仍按全局上限拒绝超大声明。
- `#[handler(in_process)]` 明确区分“只有 mediator、没有 HTTP 路由”的 handler：
  不生成 direct dispatch/Serialize 要求，并注册 `InProcessHandler` marker，
  使 Host 启动诊断不把合法 mediator handler 当 orphan route。
- `#[handler]` 默认仍要求 HTTP 响应可序列化；`#[handler(in_process)]` 是
  2.0 非 Serialize 进程内 handler 的迁移路径。

## 7. 验证矩阵

新增 wire-level 测试：

- `crates/engine/tests/admission.rs`：静默连接释放 permit、跨 listener 的全局
  MaxConnections、shutdown 释放未发请求的连接。
- `crates/engine/tests/tls.rs`：rustls 客户端真实握手 + HTTP 请求。
- `crates/engine/tests/http2.rs`：h2c prior-knowledge 多请求。
- `crates/webx/tests/metrics_status_test.rs`：handler 失败计入 5xx、错误响应
  清除表示 headers。
- `crates/webx/tests/media_body_contract_test.rs`：415、route body limit 放宽。
- `crates/webx/tests/error_writer_panic_test.rs`：writer panic 回退 500。
- `crates/webx/tests/in_process_handler_test.rs`：无路由 mediator handler 可构建 Host。
- `crates/host/tests/lifecycle_test.rs`：Host 单次运行。
- SSE quiet shutdown 测试在 `crates/webx/tests/sse_test.rs`。

## 8. 明确不在本次范围

- 自定义 HTTP/1 codec、`!Send`/`Rc` 引擎：可能继续推高峰值持续吞吐，但会改变
  `Send` handler/DI 契约，另立产品决策。
- 绝对 RPS CI 阈值：仓库外 `rw-bench` 仍受同机 co-tenant 负载影响；本次优先
  固化 wire-level 正确性和可重复的 startup/admission 测试，性能基线单独记录。
- 旧 `webx_host::HttpContext`/`HttpRequest`/`HttpResponse` 具体类型删除后的
  source migration（2.1 迁移说明见 CHANGELOG）。
