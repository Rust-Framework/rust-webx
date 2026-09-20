# appsettings.json 配置体系

## 自动加载

框架启动时自动加载：

```
appsettings.json                    # 基础配置
appsettings.Development.json        # 开发环境覆盖（Development 模式）
```

## 内置配置节

```json
{
  "App": {
    "Name": "My API",
    "Urls": ["http://0.0.0.0:5000"],
    "MaxBodySize": 10485760,
    "MaxConnections": 10000
  },
  "Jwt": {
    "Secret": "change-me"
  },
  "Cors": {
    "Origins": ["*"],
    "Methods": ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
    "Headers": ["Content-Type", "Authorization"],
    "AllowCredentials": false,
    "MaxAge": 86400
  },
  "Tls": {
    "CertPath": "",
    "KeyPath": ""
  },
  "RateLimit": {
    "Enabled": false,
    "RequestsPerSecond": 100,
    "BurstSize": 200,
    "MaxTrackedIps": 10000,
    "TrustProxy": false
  },
  "Metrics": {
    "Enabled": false
  },
  "Form": {
    "MaxRequestSize": 268435456,
    "MaxFileSize": 134217728,
    "MaxFieldSize": 1048576,
    "MemoryThreshold": 1048576,
    "TempDir": "uploads-tmp"
  }
}
```

## 对应类型

| 配置节 | Rust 类型 |
|--------|----------|
| `App` | `AppSection` |
| `Jwt` | `JwtSection` |
| `Cors` | `CorsSection` |
| `Tls` | `TlsSection` |
| `RateLimit` | `RateLimitSection` |
| `Metrics` | `MetricsSection` |
| `Form` | `FormSection` |

合并后为 `AppOptions` 结构体。

> `App.MaxBodySize` 只约束非 multipart 请求；声明 `Content-Type: multipart/form-data`
> 的请求按 `Form` 节度量，因为上传端点天然需要比 JSON 端点大得多的上限。
> 详见[文件上传与下载](../05-request-pattern/file-upload-download.md)。

## 访问配置

```rust
Host::builder()
    .configure(|app| {
        app.useOptions(|opts| {
            println!("Listening on: {:?}", opts.app.urls);
        });
    })
```

## 小结

`appsettings.json` 与 ASP.NET Core 格式兼容，降低迁移成本。

下一节：[AppMode 与环境切换](app-modes.md)
