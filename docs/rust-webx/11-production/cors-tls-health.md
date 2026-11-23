# CORS、TLS 与健康检查

## CORS

```rust
Host::builder().use_cors(CorsConfig {
    origins: vec!["https://app.example.com".into()],
    ..Default::default()
})
```

或从 `appsettings.json` 自动加载 `Cors` 节。CORS 默认关闭（空 `origins` 不发送任何 CORS 头）。

Production 环境禁止使用 `Cors.Origins: ["*"]`：builder 会直接 panic 硬失败，必须通过 `Cors.Origins` 或环境变量 `APP__Cors__Origins` 设置明确白名单。守卫校验的是**实际生效**的 `CorsConfig`（包括 `use_cors`），并在响应中追加 `Vary: Origin`。

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

### TLS 抓包调试（`SSLKEYLOGFILE`，仅 Development）

本机联调 TLS 时，设置标准环境变量 `SSLKEYLOGFILE` 并显式 `APP_ENV=Development`，框架会把
rustls 的会话密钥写入该文件，供 Wireshark/`tcpdump` 解密：

```text
APP_ENV=Development SSLKEYLOGFILE=/tmp/webx-keys.log cargo run -p docbit-host
```

- **Production 忽略该变量**（即使设置也只打一次 warning）：密钥日志是调试设施，生产开启等于
  把会话密钥落盘。宿主启动后再调用 `webx_engine::tls::with_key_log` 也不会改变已构建的 TLS 配置。
- 该文件等同于凭据：不要放进共享卷/镜像层，调试结束删除；不要把它接入日志采集。
- 单元测试可用 `webx_engine::tls::key_logging_allowed(mode)` 断言模式边界。
- 实现入口：`crates/engine/src/tls.rs` 的 `with_key_log` / `key_logging_allowed`，宿主在装配
  listener TLS 时统一经过它。

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

### 异步依赖探测（1.4.0）

同步闭包跑在请求任务上且无法超时；数据库/HTTP/队列探测应实现 `IHealthCheck`：

```rust
use webx::health::{IHealthCheck, HealthStatus, DEFAULT_HEALTH_CHECK_TIMEOUT};

struct DatabaseProbe { pool: Arc<Pool> }

#[async_trait::async_trait]
impl IHealthCheck for DatabaseProbe {
    fn name(&self) -> &str { "database" }
    async fn check(&self) -> HealthStatus {
        match self.pool.ping().await {
            Ok(()) => HealthStatus::pass(),
            Err(e) => HealthStatus::fail(format!("db unreachable: {e}")),
        }
    }
}

Host::builder()
    .add_health_check_async(DatabaseProbe { pool })
    // 或显式超时：
    // .add_health_check_async_with_timeout(slow_probe, Duration::from_secs(1))
    .build();
```

- 每次 `/health`、`/health/ready` 请求**并发**执行所有 async 检查，保持注册顺序返回；每项独立超时（默认
  `DEFAULT_HEALTH_CHECK_TIMEOUT` = 3s）。超时 → `fail("timed out after ...")` → readiness 503；
  `warn` 仍为 200。
- 旧的同步 `snapshot()` / `overall_status()` 仍可用：async 检查在同步视图里显示为 `warn`（detail 说明
  需要异步求值），不会让旧调用点编译失败；权威结论用 `snapshot_async()` / `overall_status_async()`。
- 超时是协作式取消：检查内部若执行长时间阻塞同步代码，超时无法抢占——I/O 探测请用 async 实现。

## 小结

CORS、TLS、健康检查是生产部署的三大基础设施，框架均提供内置支持。

下一节：[缓存与速率限制](caching-rate-limit.md)
