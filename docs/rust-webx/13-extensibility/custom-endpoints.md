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
| `StubEndpoint` | 由路由宏在编译期注册，绑定请求并调用 Handler |

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

`Host::build()` 会在内部构建路由表，`HostBuilder` 目前**没有**公开往路由器里追加
`IEndpoint` 的接口（`Host::router` 是私有字段）。因此自定义 `IEndpoint` 主要作为
框架内部扩展点，而不是应用层 API。

应用层需要「完全掌控响应」时，用下面两种方式之一，二者都是框架的一等能力：

1. **返回 `ResponseData`**（推荐）：路由仍然是普通的 `#[get]` / `#[post]` 端点，
   Handler 通过 `ResponseData` 决定状态码、响应头与响应体。
   见[文件上传与下载](../05-request-pattern/file-upload-download.md)。

   ```rust
   #[get("/api/files/{id}/download")]
   impl IRequest<ResponseData> for DownloadFileRequest {}
   ```

2. **写中间件**：`IMiddleware` 拿得到 `&mut dyn IHttpContext`，可以在路由之前
   短路请求、直接写响应体，例如 `/robots.txt`、`/sitemap.xml` 这类非 API 路径。

## 何时使用

| 场景 | 推荐 |
|------|------|
| 标准 REST API | `RequestEndpoint`（默认，无需自定义） |
| 文件下载 / 自定义状态码与响应头 | `ResponseData` |
| Webhook 验签 | 自定义 `IEndpoint`（框架内扩展）或中间件 |
| 固定响应（健康检查） | `StaticJsonEndpoint` |
| 服务端渲染 HTML | `StaticHtmlEndpoint` 或中间件 |

## 小结

大多数场景使用默认 `RequestEndpoint` 即可；需要掌控响应时优先返回 `ResponseData`，
需要在路由之前拦截时写中间件。

下一节：[样式与约定封装](style-patterns.md)
