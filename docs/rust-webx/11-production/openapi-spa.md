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
1. 请求路径匹配静态文件 → 直接返回
2. 非 API 路径无匹配 → fallback 到 `index.html`
3. 支持前端 History 路由（React Router、Vue Router 等）

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
