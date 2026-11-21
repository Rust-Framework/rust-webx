# 指标（IMeter）

框架提供最小指标端口，不绑定 Prometheus/OTel；应用实现 `IMeter` 并把它交给组件。

## 内置 HTTP 指标（`GET /metrics`）

`Metrics.Enabled = true` 时框架直接暴露 `GET /metrics`（Prometheus text 0.0.4），不需要接 `IMeter`：

| 指标 | 类型 | 标签 | 含义 |
|---|---|---|---|
| `http_requests_total` / `http_responses_2xx_total` / `http_responses_4xx_total` / `http_responses_5xx_total` | counter | — | 进程级计数（2.1 兼容，保持不变） |
| `http_requests_inflight` | gauge | — | 当前在管道内的请求数（含本次 scrape） |
| `http_request_timeouts_total` | counter | — | 被入站 deadline 取消（504）的请求 |
| `http_route_requests_total` | counter | route, status | 按匹配路由模板的状态类计数 |
| `http_request_duration_seconds` | histogram | route | 请求延迟（秒；µs 桶，含 `+Inf`） |
| `http_route_label_overflow_total` | counter | — | 超过 512 个路由标签被丢弃的请求（基数保护） |
| `engine_connections_admitted_total` | counter | — | 通过全局准入的连接数 |
| `engine_connections_rejected_total` | counter | — | 等待准入超时、收到 503 的连接数 |
| `engine_admission_waiting` | gauge | — | 已 accept 但仍在等待准入槽的连接数（准入队列深度） |
| `engine_connections_active` | gauge | — | 当前打开的连接数（每个连接持有一个准入 permit；含 keep-alive 与 SSE 长连接） |
| `engine_connections_capacity` | gauge | — | 准入上限 `App.MaxConnections`，与 `active` 一起看容量水位 |

`route` 使用**匹配到的路由模板**（如 `/api/users/{id}`），未匹配请求统一为 `unmatched`；
路径参数不会成为标签，因此不会因随机 URL 造成基数爆炸。被取消的请求同样进入延迟直方图
（延迟是真实耗时），并计入 `http_request_timeouts_total`。

```text
http_route_requests_total{route="/fail",status="5xx"} 1
http_request_duration_seconds_bucket{route="/fail",le="0.005"} 1
http_request_duration_seconds_bucket{route="/fail",le="+Inf"} 1
http_request_duration_seconds_sum{route="/fail"} 0.00041
http_request_duration_seconds_count{route="/fail"} 1
```

```rust
use std::sync::Arc;
use webx_ai::GatewayExecutor;

let executor = GatewayExecutor::new(catalog, router)
    .with_meter(Arc::new(my_meter)); // 生产 exporter
```

测试/本地可以用 `webx::MemoryMeter`：

```rust
use webx::MemoryMeter;

let meter = Arc::new(MemoryMeter::new());
let executor = GatewayExecutor::new(catalog, router)
    .with_meter(Arc::clone(&meter) as Arc<dyn webx::IMeter>);

// ... request ...

assert_eq!(
    meter.counter_value(
        "ai_gateway_requests_total",
        &[("model", "fast"), ("mode", "complete"), ("status", "ok")]
    ),
    1
);
```

## OpenTelemetry 桥接（2.2-1b）

框架**不**引入 OTel 依赖（`opentelemetry` 的版本节奏与 MSRV 需单独评审）。两条零依赖桥接路径：

1. **Collector 拉取（推荐）**：`/metrics` 是标准 Prometheus text 0.0.4，
   OTel Collector 用 `prometheusreceiver` 抓取后走 OTLP 导出：

   ```yaml
   receivers:
     prometheus:
       config:
         scrape_configs:
           - job_name: rust-webx
             scrape_interval: 15s
             static_configs:
               - targets: ["app:8080"]
   exporters:
     otlp:
       endpoint: otel-collector:4317
   service:
     pipelines:
       metrics:
         receivers: [prometheus]
         exporters: [otlp]
   ```

2. **进程内自定义指标**：实现 `webx::IMeter`（counter/histogram/gauge）并通过
   `HostBuilder::use_meter` 注册，AI 执行器等组件会写入；生产实现把三类指标映射到
   `opentelemetry::metrics::Meter`。内置的 RED/直方图/连接 gauge 目前只走 `/metrics`，
   进程内 OTel exporter 属于 2.4 的 starter 家族（`otel` crate），单独立项。

## 实时（WebSocket）指标（2.3-6）

存在 WebSocket 路由时，`/metrics` 额外输出 `realtime_*` 系列（进程级聚合，
`IConnection::stats()` 的聚合版）：

| 指标 | 类型 | 含义 |
|---|---|---|
| `realtime_connections_active` | gauge | 注册表活跃连接 |
| `realtime_connections_opened_total` / `_closed_total` | counter | 会话开/关 |
| `realtime_messages_in_total` / `_out_total` | counter | 消息数 |
| `realtime_message_bytes_in_total` / `_out_total` | counter | 消息字节 |
| `realtime_messages_dropped_total` | counter | 背压丢弃 |
| `realtime_rate_limited_total` | counter | 入站限流触发 |
| `realtime_connection_closes_total{code}` | counter | 关闭码分布 |
| `realtime_message_dispatch_seconds` | histogram | 消息即请求分发延迟 |

## 执行器指标

| 指标 | 类型 | 标签 |
|---|---|---|
| `ai_gateway_requests_total` | counter | model, mode, status |
| `ai_gateway_request_duration_ms` | histogram | model, mode, status |
| `ai_gateway_ttft_ms` | histogram | model |
| `ai_gateway_tokens_total` | counter | model, kind |
| `ai_gateway_cost_micros_total` | counter | model |
| `ai_gateway_active_streams` | gauge | model |
| `ai_gateway_upstream_errors_total` | counter | provider, retryable |
| `ai_gateway_quota_rejections_total` | counter | model |
| `ai_gateway_circuit_state` | gauge | provider, model（上游 model） |

- `mode` = `complete` / `stream`；`status` = `ok` / `error` / `client_gone` / `upstream_error`。
- `kind` = `prompt` / `completion` / `cached` / `reasoning`；零值不写。
- `ai_gateway_cost_micros_total` 只累加非零成本；价格由 `ICostCalculator` 提供。
- `active_streams` 在流创建时 +1，在完成/断连/失败时恰好 -1。
- `ai_gateway_circuit_state`：`0` = closed、`1` = half-open、`2` = open；每次熔断判定/成功/失败后刷新。
  告警可用 `max by (provider,model) == 2`。
- 配额拒绝发生在调用 provider 之前，只记 quota 指标，不产生 upstream/token 指标。

## 成本

```rust
use webx_ai::{GatewayExecutor, ModelPrice, StaticCostTable};

let costs = StaticCostTable::new()
    .with_price("fast", ModelPrice::new(150_000, 600_000, 75_000, 600_000))
    .with_default_price(ModelPrice::ZERO);

let executor = GatewayExecutor::new(catalog, router)
    .with_meter(meter)
    .with_cost_calculator(std::sync::Arc::new(costs));
```

价格单位是 micro-USD / 百万 token。cached prompt 与 reasoning 桶单独计价，并从 base token 中扣除，
不会重复计费。成本同时进入配额结算、`UsageEvent.cost_micros` 和
`ai_gateway_cost_micros_total`。

## 结构化日志

执行器用 `tracing` 输出完成/失败/流日志，字段：

`request_id`、`tenant_id`、`model`、`provider`、`mode`、`status`、`latency_ms`、
`prompt/completion/cached/reasoning_tokens`、`cost_micros`、`trace_id`；流日志额外带 `ttft_ms`。
上游每次 attempt 失败单独 warn（provider/retryable/error）。**不要**记录 prompt/response 文本。

### W3C traceparent

- 入站：`RequestTracing` 优先 `x-request-id`；没有时解析 `traceparent`
  （`00-32hex-16hex-2hex`），把 trace id 写入 `x-trace-id` 与 `RequestContext`。
- 出站：`rust-webx-client` 在未显式设置 `traceparent` 时，用当前
  `RequestContext::trace_id()` 注入 `00-{trace_id}-{span}-01`；显式 header 优先。
- 只有 32-hex trace id 才会注入；`x-request-id` 是任意字符串时跳过。

## 标签纪律

- 只放低基数维度：model、provider、status、kind、mode。
- **不要**放 `key_id`、`tenant_id`、`request_id`、prompt/response 文本；这些会打爆基数或泄露数据。
- 指标名用静态字符串；动态 name 会让 exporter 无法预注册。

## 就绪探针（GatewayHealth）

`/health/ready` 应回答“还有没有可用的上游”。`rust-webx-ai-gateway` 的 `GatewayHealth` 把
`HealthRegistry`（冷却）与 `CircuitBreaker`（开路）聚合成 host 健康检查：

```rust
let health = Arc::new(
    GatewayHealth::new(router.health().clone())
        .with_catalog(handle.clone())          // 用目录全集判断“全部不可用”
        .with_circuit_breaker(circuit.clone()),
);
Host::builder()
    .add_health_check("gateway-upstreams", health.check())
    .build();
```

| 状态 | 条件 |
|------|------|
| `pass` | 没有任何观测，或至少一个已知模型可用 |
| `warn` | 部分模型冷却/熔断开路，仍有可用候选 |
| `fail` | 所有已知模型（或目录内全部模型）都不可用 |

detail 只含数量与 `provider/model`，不含 tenant/key/prompt。检查是同步闭包，读内存快照，不查库。
熔断冷却已到期（尚未探测）按“可探测”算可用，避免 `Open` 过期后 readiness 永久失败。

## 实现自己的 meter

```rust
use std::sync::Arc;
use webx::{Counter, Gauge, Histogram, IMeter};

struct PrometheusMeter { /* ... */ }

impl IMeter for PrometheusMeter {
    fn counter(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Counter> { /* ... */ }
    fn histogram(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Histogram> { /* ... */ }
    fn gauge(&self, name: &'static str, tags: &[(&str, &str)]) -> Arc<dyn Gauge> { /* ... */ }
}
```

实现必须非阻塞、可并发；不要每次请求查数据库或做网络 IO。`NoopMeter` 用于关闭指标，
`MemoryMeter` 仅用于测试/小部署。

## 边界

- 已有：端口、Noop/Memory、执行器请求/TTFT/token/成本/上游错误/配额/active streams/熔断状态指标、
  结构化完成/失败/流日志，以及 `GatewayHealth` 的 `/health/ready` 聚合。
- 未做：租户档位标签、上游子 span、OTel exporter、`tracestate`。
- 指标失败不得影响请求；exporter 由应用注册。
