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
    "MaxBodySize": 10485760
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
    "CertificatePath": "",
    "KeyPath": ""
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

合并后为 `AppOptions` 结构体。

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
