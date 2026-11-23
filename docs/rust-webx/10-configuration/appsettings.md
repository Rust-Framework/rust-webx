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
    "MaxTrackedIps": 10000
  },
  "Metrics": {
    "Enabled": false,
    "MaxSeries": 10000
  },
  "Idempotency": {
    "Enabled": false,
    "TtlSeconds": 86400,
    "MaxTrackedKeys": 10000,
    "MaxBodyBytes": 1048576
  },
  "ClientInfo": {
    "TrustForwardedHeaders": false,
    "TrustedProxies": []
  },
  "Turn": {
    "Secret": "",
    "Urls": [],
    "TtlSeconds": 3600,
    "Realm": null
  },
  "Form": {
    "MaxRequestSize": 268435456,
    "MaxFileSize": 134217728,
    "MaxFieldSize": 1048576,
    "MemoryThreshold": 1048576,
    "TempDir": "uploads-tmp"
  },
  "Culture": {
    "Default": "zh-CN"
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
| `Idempotency` | `IdempotencySection` |
| `ClientInfo` | `ClientInfoSection` |
| `Turn` | `TurnSection` |
| `Form` | `FormSection` |
| `Culture` | `CultureSection` |

合并后为 `AppOptions` 结构体。

### RateLimit 与 ClientInfo（转发头唯一入口）

`RateLimit` 节做鉴权前按客户端 IP 限流；`ClientInfo` 节决定“客户端 IP 是怎么解析出来的”。可信代理的
`X-Forwarded-For` / `X-Real-IP` 只由 `ClientInfo` 中间件解析一次（在限流、鉴权、绑定之前），限流桶、
审计与 Handler 的 `#[fromclient] ClientInfo` 读同一份结果：

```json
{
  "RateLimit": { "Enabled": true, "RequestsPerSecond": 20, "BurstSize": 40 },
  "ClientInfo": {
    "TrustForwardedHeaders": true,
    "TrustedProxies": ["10.0.0.0/8"]
  }
}
```

- **反向代理之后**：开启 `ClientInfo.TrustForwardedHeaders` 并填写 `ClientInfo.TrustedProxies`。只有直连
  对端落在网段内才采信转发头；否则按对端地址计。`TrustForwardedHeaders=true` 但 `TrustedProxies` 为空时
  框架忽略转发头并打 `warn`（否则客户端可以轮换 `X-Forwarded-For` 绕过限流）。
- **`RateLimit.TrustProxy` / `RateLimit.TrustedProxies` 成为 legacy**（2.8.0 起不再生效，计划 3.0 移除）：serde 仍
  接受旧配置（不会启动失败），但限流器忽略它们并在启动时打 `warn`。迁移方式与示例见
  [缓存与速率限制](../11-production/caching-rate-limit.md)。
- **环境变量**：`APP__ClientInfo__TrustForwardedHeaders=true`；数组元素按索引覆盖，例如
  `APP__ClientInfo__TrustedProxies__0=10.0.0.0/8`。
- 代码构造 `RateLimitSection` 时用 `..Default::default()` 即可跳过这两个 legacy 字段（字段仍保留以兼容
  `appsettings.json`）。

### Culture

`Culture.Default` 是**全局 UI culture**，决定端点名称/描述、OpenAPI 与端点目录的语言，
默认 `"en"`（也可用 `APP__Culture__Default` 覆盖）。它不跟随请求的 `Accept-Language`：权限资源、
菜单名称这类写入数据库的内容不应因访问者语言而漂移。多语言部署按 Host 配置切换，或由调用方
显式指定 culture。

资源默认从应用基准目录的 `locales/<culture>.json` 读取，只需：

```rust
Host::builder().add_localization().build();
```

单文件部署用 `add_localization_with(LocalizationOptions::none().resources(...))` 把资源编译进二进制。
资源键、`#[display]` / `#[description]` / `#[category]` 与回退链详见
[路由宏的端点名称章节](../05-request-pattern/route-macros.md#端点名称与描述资源键)。

> `Culture.Default` 应与资源文件名匹配（如 `"zh-CN"` → `locales/zh-CN.json`）；缺失时按
> `text` → key 回退。

> `App.MaxBodySize` 只约束非 multipart 请求；声明 `Content-Type: multipart/form-data`
> 的请求按 `Form` 节度量，因为上传端点天然需要比 JSON 端点大得多的上限。
> 详见[文件上传与下载](../05-request-pattern/file-upload-download.md)。

### Jwt 密钥来源（1.3.0 起）

`Jwt.Secret` 与 `Jwt.SecretFile` 二选一即可。启动时取**第一个非空**来源：

| 顺序 | 来源 | 说明 |
|------|------|------|
| 1 | `APP__Jwt__Secret` / `JWT_SECRET` | 环境变量在加载阶段覆盖 `Jwt.Secret`，`APP__` 优先 |
| 2 | `Jwt.Secret` | 配置里的明文密钥（生产守卫会拒绝弱值） |
| 3 | `Jwt.SecretFile` | 密钥文件路径；文件内容被读作密钥 |

```json
{
  "Jwt": {
    "Secret": "",
    "SecretFile": "data/jwt.key",
    "ExpirationMinutes": 60
  }
}
```

`Jwt.SecretFile` 的读取与校验发生在**启动期**（`HostBuilder` 装配认证之前），失败即终止启动，
不会带着坏密钥对外服务：

- 文件缺失/不可读 → 启动失败，错误信息包含路径和修复建议；
- 内容 trim 后为空或 **< 32 字节** → 启动失败（不足 256 bit 的 HS256 密钥可被暴力破解）；
- 文件内容首尾空白会被去掉（`echo secret > jwt.key` 写入的换行不属于密钥）；
- **Unix**：group/other 权限位必须为空（`chmod 600`；`0400`/`0700` 也可），否则启动失败；
- **Windows**：不存在 POSIX mode，框架**不校验 ACL**，请自行把文件限定为服务账号 +
  `Administrators` 可读；DPAPI 包装的密钥不在 1.3.0 支持范围（后续项）。
- 框架**不创建**密钥文件：首次部署由安装器/运维生成（足够随机的 32+ 字节写入 `0600` 文件）。

相对路径按**进程工作目录**解析（与 `Tls.CertPath`/`Tls.KeyPath` 一致）。优先级只影响密钥来源；
token 生命周期仍由 `Jwt.ExpirationMinutes` 决定。

### 自定义 issuer 与最终密钥

`Jwt.SecretFile` 在**应用 service 注册之前**解析为 `Jwt.Secret`，并作为 `JwtSecret` 单例注册进 DI。
自定义 `ITokenIssuer` 因此有两种拿最终密钥的方式：

- `HostBuilder::register_with_options(|svc, options| ...)`：闭包里的 `options.jwt.secret` 已是最终值；
- `HostBuilder::register(|svc| ...)` + DI 工厂内 `resolve_arc::<JwtSecret>()`。

不要自行读取 `SecretFile`：那会在 `APP__Jwt__Secret`/`JWT_SECRET` 或 `Jwt.Secret` 已设置时读到错误
（或空的）密钥。只配置 `SecretFile` 时上述顺序保证 issuer 与 `JwtAuth` 使用同一把密钥；未配置任何
密钥时 `JwtSecret` 不存在，解析即启动失败，不会空密钥签名。`register` 的签名与行为不变。

### Turn（TURN/STUN 凭据）

`Turn` 节是 TURN REST（coturn `use-auth-secret`）凭据的**唯一配置来源**：

```json
{
  "Turn": {
    "Secret": "与 coturn static-auth-secret 相同的密钥",
    "Urls": ["turn:turn.example.com:3478", "stun:turn.example.com:3478"],
    "TtlSeconds": 3600,
    "Realm": "example"
  }
}
```

- 环境覆盖：`APP__Turn__Secret`、`APP__Turn__Urls__0`、`APP__Turn__TtlSeconds` 等；
- `Secret`+`Urls` 均为空 = 未配置（Host 不注册 `TurnRestConfig`，TURN 端点报明确 DI 错误）；
- 只配置一部分（如只有 `Urls`）是**启动错误**，不会静默降级；
- Host 启动时校验并注册 `TurnRestConfig` 供 Handler 注入（`#[inject] config: Arc<TurnRestConfig>`）；
  也可用 `HostBuilder::use_turn_config(...)` 显式覆盖；
- TTL 允许 1..=86400；密钥只存在于服务端，不得下发静态密码。

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
