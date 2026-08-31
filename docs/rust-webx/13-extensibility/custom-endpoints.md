# 自定义 Endpoint

## IEndpoint 接口

```rust
#[async_trait]
pub trait IEndpoint: Send + Sync {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()>;
}
```

## 内置实现

| 类型 | 用途 |
|------|------|
| `RequestEndpoint` | 标准 IRequest → Handler 调度 |
| `ControllerEndpoint` | 自定义函数式 endpoint（接受闭包） |
| `StaticJsonEndpoint` | 返回固定 JSON |
| `StaticHtmlEndpoint` | 返回固定 HTML |

## 自定义 Endpoint 示例

```rust
pub struct WebhookEndpoint {
    secret: String,
}

#[async_trait]
impl IEndpoint for WebhookEndpoint {
    async fn handle(&self, ctx: &mut dyn IHttpContext) -> Result<()> {
        let signature = ctx.request().header("X-Signature").unwrap_or("");
        if !verify_signature(signature, &self.secret) {
            ctx.response_mut().set_status(401);
            return Ok(());
        }
        // 处理 webhook payload
        ctx.response_mut().set_status(200);
        Ok(())
    }
}
```

## 注册自定义 Endpoint

在 `register()` 中手动添加到 Router（高级用法，通常不需要）。

## 何时使用

| 场景 | 推荐 |
|------|------|
| 标准 REST API | `RequestEndpoint`（默认，无需自定义） |
| Webhook 验签 | 自定义 `IEndpoint` |
| 固定响应（健康检查） | `StaticJsonEndpoint` |
| 服务端渲染 HTML | `StaticHtmlEndpoint` |

## 小结

大多数场景使用默认 `RequestEndpoint` 即可；特殊协议需求可自定义 `IEndpoint`。

下一节：[样式与约定封装](style-patterns.md)
