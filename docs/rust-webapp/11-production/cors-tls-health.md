# CORS、TLS 与健康检查

## CORS

```rust
Host::builder().use_cors(CorsConfig::default())
```

或从 `appsettings.json` 自动加载 `Cors` 节。

## TLS

```json
{
  "Tls": {
    "CertificatePath": "certs/server.pem",
    "KeyPath": "certs/server-key.pem"
  }
}
```

框架使用 `rustls` + `tokio-rustls` 提供 TLS 支持。

## 健康检查

```
GET /health
```

返回 JSON 健康状态。可注册自定义检查：

```rust
HealthCheckRegistry::new()
    .add("database", || async { check_db().await })
```

适用于 Kubernetes liveness/readiness 探针。

## 小结

CORS、TLS、健康检查是生产部署的三大基础设施，框架均提供内置支持。

下一节：[缓存与速率限制](caching-rate-limit.md)
