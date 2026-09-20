# 内置中间件一览

## 框架自带中间件

| 中间件 | 启用方式 | 职责 |
|--------|---------|------|
| 错误处理 | 自动（host 请求循环） | Error → RFC 7807 响应 |
| RequestId | 自动 | 生成 X-Request-Id |
| SecurityHeaders | 自动 | X-Frame-Options 等 |
| CORS | `use_cors(config)` | 跨域 |
| JWT Auth | `add_authentication()` | Bearer Token 认证 |
| Resource Auth | `use_resource_authorization()` | 基于路由元数据的授权 |
| RateLimit | 配置 `RateLimit.enabled` | 请求速率限制 |
| SPA | `use_spa("wwwroot")` | 静态文件托管 |
| RequestTracing | 需手动 `use_middleware::<RequestTracing>()` | 请求日志、注入 `x-trace-id` |
| Timing | 需手动 `use_middleware::<TimingMiddleware>()` | 注入 `x-request-count` |
| Compression | 需手动 `use_middleware::<CompressionMiddleware>()` | Gzip 压缩 |

## CORS

```rust
use webx::CorsConfig;

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

`add_authentication()` 注册 `jwt_middleware` 并从 `Jwt.Secret` 读取密钥。管道顺序见
[中间件编排策略](ordering-strategy.md)，授权语义见 [认证与授权](../09-auth-security/INDEX.md)。

## SPA 托管

```rust
Host::builder().use_spa("wwwroot")
```

非 API 路径 fallback 到 `index.html`，支持前端 History 路由。

## 速率限制

在 `appsettings.json` 的 `RateLimit` 节启用即自动接入；也可手动提供配置：

```rust
Host::builder().use_middleware_with(|| {
    Arc::new(RateLimitMiddleware::from_config(&RateLimitSection {
        enabled: true,
        requests_per_second: 10.0,
        burst_size: 20,
        ..Default::default()
    })) as Arc<dyn IMiddleware>
})
```

`RateLimitMiddleware` 没有 `Default`，必须用 `new(requests_per_second, burst_size)` 或
`from_config(...)` 构造。基于 IP（可配 `trust_proxy`）限制请求频率。

## 小结

大部分生产中间件通过 `HostBuilder` 方法一行启用，无需手动组装。RequestTracing、Timing、
Compression 需要用 `use_middleware` 显式接入。

下一节：[自定义中间件](custom-middleware.md)
