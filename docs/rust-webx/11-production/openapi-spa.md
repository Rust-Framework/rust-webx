# OpenAPI 与 SPA 托管

## OpenAPI 自动生成

框架从编译时收集的路由元数据自动生成 OpenAPI 3.0 规范：

```
GET /openapi.json    → OpenAPI 规范 JSON
GET /swagger         → Swagger UI 页面
```

```rust
use rust_webx::{generate_openapi_spec, APIUI_HTML};
```

## SPA 静态托管

```rust
Host::builder().use_spa("wwwroot")
```

`SpaMiddleware` 行为：
1. 请求路径匹配静态文件 → 交给宿主流式发送
2. 非 API 路径无匹配 → fallback 到 `index.html`
3. 支持前端 History 路由（React Router、Vue Router 等）

静态文件不是整份读进内存，而是以文件响应发送，因此自带：

| 能力 | 行为 |
|------|------|
| MIME 类型 | 按扩展名推断（完整扩展名表） |
| `ETag` / `Last-Modified` | 自动生成 |
| `Range` | 支持 `206` 断点续传 |
| `HEAD` | 与 GET 相同头部，无响应体 |
| `Cache-Control` | `/assets/**` 为 `public, max-age=31536000, immutable`；其余为 `public, max-age=0, must-revalidate`（覆盖默认的 `no-store`） |

> `wwwroot` 下的文件**不经过授权检查**。需要鉴权的文件请放在可写目录，用
> [`ResponseData::file`](../05-request-pattern/file-upload-download.md) + `#[authorize]` 提供。

### 目录结构

```
wwwroot/
├── index.html
├── app.js
├── app.css
└── assets/
```

## 全栈单体部署

```rust
Host::builder()
    .use_spa("wwwroot")
    .add_authentication()
    .build()
    .run()
    .await?;
```

一个二进制同时服务 API + 前端 + Swagger 文档。

## 小结

OpenAPI 从类型信息自动生成，SPA 托管让全栈单体部署成为可能。

下一节：[优雅关闭与可观测性](graceful-shutdown.md)
