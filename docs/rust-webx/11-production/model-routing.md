# 模型目录与路由（provider 端口）

AI 聚合网关的核心是把"客户端要的模型名"映射到"某个上游 provider + 上游模型"，并在故障/延迟/
能力不满足时切换。`rust-webx-ai` 把这层拆成三块：

```text
CanonicalRequest（统一 IR）
        │
        ▼
ChatProvider（厂商适配器） ── 能力矩阵
        │
        ▼
ModelCatalog（快照） + CatalogRouter（过滤/排序） + HealthRegistry（健康/延迟）
        │
        ▼
RoutePlan { attempts, deadline }  → 执行器（首字节规则）
```

## Provider 端口

```rust
use webx_ai::{CanonicalRequest, CanonicalResponse, CanonicalStream, ChatProvider, ProviderCapabilities, ProviderError};

pub struct MyProvider { /* HTTP client, credentials */ }

#[async_trait::async_trait]
impl ChatProvider for MyProvider {
    fn id(&self) -> &str { "my-provider" }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_stream().tools().vision()
    }

    async fn complete(&self, req: CanonicalRequest) -> Result<CanonicalResponse, ProviderError> {
        // 把 CanonicalRequest 编码成厂商 JSON，调用上游，再解码回 CanonicalResponse
        todo!()
    }

    async fn stream(&self, req: CanonicalRequest) -> Result<CanonicalStream, ProviderError> {
        // 上游 SSE → StreamEvent::{Start, Delta, Usage, Done}
        todo!()
    }
}
```

`ProviderError` 已按路由需要分类：`InvalidRequest`/`Unauthorized`/`RateLimited`/`Unavailable`/`Timeout`/
`Protocol`/`Unsupported`，带 `is_retryable()`、`http_status()`、`retry_after()`。

## 现成适配器：OpenAI 兼容

OpenAI / Azure OpenAI / vLLM / Ollama 的 `/v1/chat/completions` 可直接用 `OpenAiProvider`：

```rust
use std::sync::Arc;
use webx_ai::{ModelCatalog, OpenAiProvider};
use webx_client::{HttpClient, HttpClientOptions};

let client = HttpClient::new(
    "openai",
    HttpClientOptions::new("https://api.openai.com"),
)?;
let provider = OpenAiProvider::new("openai", Arc::new(client), api_key);

let mut catalog = ModelCatalog::new(1);
catalog.add_provider(Arc::new(provider));
```

它把 `CanonicalRequest` 编码成 OpenAI JSON、把非流响应解码成 `CanonicalResponse`，并把 SSE
`data:` 帧解码为 `StreamEvent`（文本/推理/工具调用、usage、finish_reason、`[DONE]`）。HTTP 错误已映射成
`ProviderError`（401/403、429 + `Retry-After`、5xx、超时、4xx）。

**注意**：`OpenAiProvider` 使用 `request.model` 作为上游模型名；执行器在调用前把路由
`Attempt.model.model` 写进去，别名不会直接透传给上游。

Anthropic/Gemini 等非 OpenAI 协议需要实现 `ChatProvider` 做转译（待补）。

## 目录与别名

```rust
use std::sync::Arc;
use webx_ai::*;

let mut catalog = ModelCatalog::new(1);
catalog.add_provider(Arc::new(MyProvider { /* ... */ }));

let key = ModelKey::new("my-provider", "chat-large");
catalog.add_model(
    ModelSpec::new(key.clone())
        .with_capabilities(ProviderCapabilities::chat_stream().tools())
        .with_context_window(128_000),
);

catalog.add_alias(
    "fast",
    Alias::new(
        RouteStrategy::Fallback,
        vec![
            RouteTarget::new(ModelKey::new("my-provider", "chat-large")).with_weight(3),
            RouteTarget::new(ModelKey::new("backup", "chat-small")),
        ],
    ),
);
```

客户端只看到 `fast`；日志/计费里保留实际 `ModelKey`。目录是不可变快照，构建后共享 `Arc`；
热替换（`ArcSwap`）在 G13 落地。

## 路由与健康

```rust
let router = CatalogRouter::new(Arc::new(catalog))
    .with_max_attempts(3)
    .with_default_timeout(Duration::from_secs(120));

let plan = router.plan(&RouteRequest::new("fast")).await?;
// plan.attempts 已按策略排好；执行器按顺序尝试，首字节后不再重试
```

| 策略 | 排序 |
|------|------|
| `Priority` / `Fallback` | 目录顺序 |
| `Weighted` | 权重高者先（当前确定性排序） |
| `LeastLatency` | EWMA 延迟低者先，未知排最后 |
| `Capability` | 先按 `required` 能力过滤，再目录顺序 |

`HealthRegistry` 维护每个 `ModelKey` 的连续失败、冷却和延迟 EWMA（默认 3 次失败 → 5s 冷却）。
成功/失败由执行器回写：

```rust
router.health().record_success(&key, elapsed);
router.health().record_failure(&key);
```

## 执行器

`GatewayExecutor` 把路由、配额、provider 调用和计量串起来：

```rust
use webx_ai::{GatewayContext, GatewayExecutor};

let executor = GatewayExecutor::new(Arc::new(catalog), Arc::new(router))
    .with_quota(quota_store)       // 可选：预扣-结算
    .with_usage(usage_sink)        // 可选：UsageEvent
    .with_health(router_health)    // 可选：成功/失败回写
    .with_default_max_tokens(1024);

let ctx = GatewayContext::new(request_id, tenant_id, key_id, "fast");

// 非流：可对可重试错误换候选
let response = executor.complete(&ctx, canonical_request).await?;

// 流：返回后不再重试；结束时自动结算/计费
let stream = executor.stream(&ctx, canonical_request.stream()).await?;
```

执行器保证：

- `reserve` 在调用上游之前；成功/失败/断连后 `settle`/`release` 恰好一次。
- 非流可重试；流一旦返回**不重试**（避免重复输出/计费）。
- 断连时按 tokenizer 估算 `ClientGone`，释放并发槽；正常结束按真实 usage 结算。
- 每个结束的请求写一条 `UsageEvent`（`request_id` 幂等）。
- `GatewayError` 已映射 route/quota/provider 的 HTTP 状态。

## 错误映射

| 错误 | `http_status()` | 说明 |
|------|-----------------|------|
| `RouteError::UnknownModel` | 404 | 别名不存在 |
| `RouteError::NoCandidate` | 503 | 全被禁用/不健康/能力不满足 |
| `RouteError::Invalid` | 400 | 请求参数错误 |

## 边界

- 已实现：IR、`ChatProvider` 端口、OpenAI 兼容适配器、目录/别名、五种策略的过滤与排序、健康/冷却、
  `IRoutePlanner`、`GatewayExecutor`（首字节规则、重试、配额/计量接线）。
- 未实现：OpenAI 兼容 HTTP 端点、非 OpenAI 适配器、租户级路由、热替换、熔断细粒度策略。
- 路由只读目录快照；不要在 plan 里查数据库。
