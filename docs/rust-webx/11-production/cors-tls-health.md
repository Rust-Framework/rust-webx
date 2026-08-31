# CORS、TLS 与健康检查

## CORS

```rust
Host::builder().use_cors(CorsConfig::default())
```

或从 `appsettings.json` 自动加载 `Cors` 节。

Production 环境应避免使用 `Cors.Origins: ["*"]`，请通过 `Cors.Origins` 或环境变量 `APP__Cors__Origins` 设置明确白名单。

## TLS

```json
{
  "Tls": {
    "CertPath": "certs/server.pem",
    "KeyPath": "certs/server-key.pem"
  },
  "App": {
    "Urls": ["https://0.0.0.0:443"]
  }
}
```

框架使用 `rustls` + `tokio-rustls` 提供 TLS 支持；`App.Urls` 含 `https://` 时自动启用 TLS。

## 健康检查

| 端点 | 说明 |
|------|------|
| `GET /health` | 运行时探针聚合，任一 fail → HTTP 503 |
| `GET /health/ready` | 就绪探针（同 `/health`） |
| `GET /health/live` | 存活探针，进程存活即 pass |
| `GET /healthz` | `/health` 别名 |

响应格式遵循 RFC 8407 `application/health+json`。

可注册自定义检查：

```rust
Host::builder()
    .add_health_check("database", || {
        if db_ping() {
            HealthStatus::pass()
        } else {
            HealthStatus::fail("db unreachable")
        }
    })
    .build();
```

适用于 Kubernetes liveness/readiness 探针。

## 小结

CORS、TLS、健康检查是生产部署的三大基础设施，框架均提供内置支持。

下一节：[缓存与速率限制](caching-rate-limit.md)
