# 内置中间件一览

## 框架自带中间件

| 中间件 | 启用方式 | 职责 |
|--------|---------|------|
| 异常处理 | 自动 | Error → HTTP 响应 |
| RequestId | 自动 | 生成 X-Request-Id |
| RequestTracing | 自动 | 请求日志 |
| Timing | 自动 | Server-Timing 头 |
| SecurityHeaders | 自动 | X-Frame-Options 等 |
| CORS | `use_cors(config)` | 跨域 |
| JWT Auth | `add_authentication()` | Bearer Token 认证 |
| Resource Auth | `add_authentication()` 自动 | 基于路由的授权 |
| RateLimit | 配置启用 | 请求速率限制 |
| Compression | 自动 | Gzip 压缩 |
| SPA | `use_spa("wwwroot")` | 静态文件托管 |

## CORS

```rust
use rust_webx::CorsConfig;

Host::builder()
    .use_cors(CorsConfig {
        origins: vec!["http://localhost:3000".into()],
        ..Default::default()
    })
```

或从 `appsettings.json` 的 `Cors` 节自动加载。

## JWT 认证

```rust
Host::builder().add_authentication()
```

自动注册 `jwt_middleware`，从 `Jwt.Secret` 配置读取密钥。

## SPA 托管

```rust
Host::builder().use_spa("wwwroot")
```

非 API 路径 fallback 到 `index.html`，支持前端 History 路由。

## 速率限制

```rust
svc.add_middleware::<RateLimitMiddleware>()
```

基于 IP 或自定义 key 限制请求频率。

## 小结

大部分生产中间件通过 `HostBuilder` 方法一行启用，无需手动组装。

下一节：[自定义中间件](custom-middleware.md)
