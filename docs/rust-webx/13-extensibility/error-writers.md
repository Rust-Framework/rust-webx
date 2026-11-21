# 错误响应写入器

框架默认把错误写成 RFC 7807 `application/problem+json`。当某个 API 表面必须使用别的信封时
（典型是 OpenAI 兼容的 `/v1/*` 要 `{"error":{...}}`），注册一个 `IErrorResponseWriter`，
不要把错误格式塞进每个 Handler。

```rust
use std::sync::Arc;
use webx::*;

struct MyWriter;

#[async_trait::async_trait]
impl IErrorResponseWriter for MyWriter {
    fn matches(&self, route_pattern: Option<&str>, path: &str, _accept: Option<&str>) -> bool {
        route_pattern
            .map(|pattern| pattern.starts_with("/v1/"))
            .unwrap_or(false)
            || path.starts_with("/v1/")
    }

    async fn write(&self, ctx: &mut dyn IHttpContext, error: &Error) {
        let status = error.status_code();
        let message = if status >= 500 {
            "an unexpected error occurred".to_string()
        } else {
            error.public_detail().unwrap_or("request failed").to_string()
        };
        let body = serde_json::json!({
            "error": { "message": message, "type": "invalid_request_error", "param": null, "code": "invalid_request" }
        });
        ctx.response_mut().set_status(status);
        ctx.response_mut().set_header("content-type", "application/json");
        let _ = ctx.response_mut().write_bytes(serde_json::to_vec(&body).unwrap_or_default()).await;
    }
}

Host::builder()
    .add_error_writer(Arc::new(MyWriter))
    .build();
```

## 规则

- **首个匹配生效**：按注册顺序；没有匹配则回退 `problem+json`。管理面与 `/v1/` 可以共存。
- **path 也参与匹配**：`/v1/` 的 404 没有路由模式，必须用 path 前缀判断。
- **5xx 脱敏**：只写固定文案，内部 detail 只进日志。
- **writer 拥有响应**：状态码、header、body 都由它决定；不要只改 body 忘了状态。
- **不要 panic**：`write` 无返回值；写失败由 transport 兜底。
- 只处理**框架错误路径**（绑定、鉴权、路由、Handler 返回 `Err`）；流式数据与成功响应不走这里。

## 现成的 OpenAI 信封

`rust-webx-ai` 提供 `OpenAiErrorWriter`：

```rust
use webx_ai::OpenAiErrorWriter;

Host::builder()
    .add_error_writer(Arc::new(OpenAiErrorWriter::new())) // 默认 /v1/
    .build();
```

它把 400/401/403/404/409/413/415/429/502/503/504 映射为 OpenAI 的
`invalid_request_error`/`authentication_error`/`permission_error`/`not_found_error`/
`rate_limit_error`/`server_error` 等，并保证 5xx 不回显内部消息。

测试覆盖：自定义 writer 接管绑定错误与 404、其他路径仍 `problem+json`
（`crates/webx/tests/error_writer_test.rs`）；信封形状与状态映射在 `crates/ai/core/src/error.rs`。
