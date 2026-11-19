# OpenAI 兼容端点

`rust-webx-ai-gateway` 把 `rust-webx-ai` 的执行器暴露成 OpenAI 兼容的
`POST /v1/chat/completions`。它只做 HTTP 适配：路由、provider、配额、用量仍在
`rust-webx-ai`。

## 装配

```rust
use std::sync::Arc;
use webx::*;
use webx_ai::{GatewayExecutor, OpenAiErrorWriter};

webx_ai_gateway::install(); // 把路由 inventory 链入二进制

Host::builder()
    .add_authentication()                 // 或 add_authentication_scheme(ApiKeyScheme…)
    .add_error_writer(Arc::new(OpenAiErrorWriter::new()))
    .register(move |svc| svc.instance(Arc::new(executor)))
    .build()
    .run()
    .await?;
```

`executor` 是应用按 [模型目录与路由](model-routing.md) 装配好的 `GatewayExecutor`。
请求 DTO 用 `#[fromclaim] user: IdentityUser` 取租户/key id；没有 tenant 返回 403。

## 模型列表

`GET /v1/models` 返回当前目录的公共别名（需要鉴权）：

```json
{
  "object": "list",
  "data": [
    { "id": "fast", "object": "model", "created": 0, "owned_by": "gateway" }
  ]
}
```

只暴露别名，不泄露上游 provider/model 名；目录热替换后下一次请求即看到新别名。

## 非流

```bash
curl http://localhost:5000/v1/chat/completions \
  -H "authorization: Bearer $TOKEN" \
  -H "content-type: application/json" \
  -d '{"model":"fast","messages":[{"role":"user","content":"hi"}]}'
```

返回：

```json
{
  "id": "chatcmpl-1",
  "object": "chat.completion",
  "created": 1730000000,
  "model": "fast",
  "choices": [{
    "index": 0,
    "message": { "role": "assistant", "content": "hello" },
    "finish_reason": "stop"
  }],
  "usage": {
    "prompt_tokens": 3,
    "completion_tokens": 2,
    "total_tokens": 5,
    "prompt_tokens_details": { "cached_tokens": 0 },
    "completion_tokens_details": { "reasoning_tokens": 0 }
  }
}
```

## 成本预演（dry-run）

请求体加 `"dry_run": true`，只返回 token/成本估算，不调用上游、不预扣配额：

```json
{ "model": "fast", "messages": [{ "role": "user", "content": "hi" }], "max_tokens": 10, "dry_run": true }
```

```json
{
  "object": "gateway.estimate",
  "model": "fast",
  "prompt_tokens": 1,
  "max_completion_tokens": 10,
  "estimated_cost_micros": 1500
}
```

成本由 `GatewayExecutor` 的 `ICostCalculator` 计算；未配置价格时为 0。估算不是承诺，
真实结算以 usage 为准。

## 流式

请求体加 `"stream": true`，返回 `text/event-stream`：
```text
data:{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}],...}

data:{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}],...}

data:{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[],"usage":{...}}

data:{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],...}

data:[DONE]
```

- `model` 始终回显客户端请求的**公共别名**，不泄露上游模型名。
- 上游 `Usage` 会作为 `choices: []` 的 chunk 下发。
- 流开始后不再切换 provider；失败会先发一个 error chunk，再以 `[DONE]` 结束。
- 客户端断开由 `GatewayStream` 的 `Drop` 结算为 `ClientGone` 并释放并发槽。

## 错误

```json
{"error":{"message":"model not found","type":"not_found_error","param":null,"code":"not_found"}}
```

- Handler 的业务错误（路由 404/503、配额 429、上游 5xx）由 `OpenAiErrorResult` 直接写；
  配额限额拒绝返回 `type/code=insufficient_quota`，其他 429 为 `rate_limit_error`，并带 `Retry-After`。
- 绑定、鉴权、404 等框架错误由注册的 [错误响应写入器](../13-extensibility/error-writers.md)
  接管。
- 5xx 只返回固定文案，内部 detail 只进日志。

## 范围

- 已支持：`/v1/chat/completions`（文本/图片/tool call/tool result、tools、tool_choice、
  response_format、采样参数、stream、dry_run）与 `/v1/models`。
- 未支持：embeddings、images、audio、batch、fine-tuning。
- 未知顶层字段进入 `CanonicalRequest.extensions`，由 provider 适配器决定是否透传。
