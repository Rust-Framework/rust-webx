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
    "LeaseSeconds": 60,
    "MaxTrackedKeys": 10000,
    "MaxBodyBytes": 1048576,
    "AllowAnonymous": true,
    "Paths": [],
    "SkipPaths": []
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

### 启动期聚合校验

`HostBuilder::try_build` 先收集**全部**启动期配置违规，再一次性以
`webx::Error::di(..)` 返回；`HostBuilder::build` panic 同一条消息。列表按
`section + key` 稳定排序，运维不再需要“改一个键、重启、再看下一个”。

被聚合的既有规则（规则与默认值均未改变，只改报错方式）：

| Key | 何时失败 |
|-----|----------|
| `Tls.ClientAuth` / `Tls.ClientCaPath` / `App.Urls` | mTLS 半配置：`ClientAuth` 开启但缺 CA 路径、CA 文件不存在、未配置 `https://` listener；或 `ClientAuth=Off` 却设置了 `ClientCaPath` |
| `Server.RequestTimeoutSeconds` | `Production` 下为 `0`（关闭入站 deadline） |
| `Server.AdmissionTimeoutMs` | `Production` 下为 `0`（无限等待准入） |
| `App.MaxConnections` | 超出引擎信号量范围 `1..=MAX_SEMAPHORE_PERMITS`（`0` 仍按既有语义夹到 `1`，不是违规） |
| `Cors.Origins` | `Production` 下为 `*`（校验中间件实际使用的列表，含 `use_cors` 显式覆盖） |
| `Jwt.Secret` | `Production` + 启用认证 + HS256 + 空/弱密钥（不足 32 字节或占位符） |
| `Jwt.*` | 启用认证时 `Jwt` 节本身非法（空算法列表、空 issuer/audience、JWKS 约束、超界的 ClockSkew/MaxTokenBytes 等） |
| `Turn.*` | `Turn` 节只配置了一部分，或 TTL 超界 |

以下情况保持原行为，**不会**变成启动错误：

- `Development`/`Test` 允许 `Server.RequestTimeoutSeconds=0`、`Server.AdmissionTimeoutMs=0`；
- `Production` 下 `Idempotency.Enabled=true` + `AllowAnonymous=true` + 空 `Paths` 只打
  `warn`（见[请求级幂等](../11-production/idempotency.md)）；
- `ClientInfo.TrustedProxies` 中的非法 CIDR 只打 `warn` 并忽略该条。

需要在启动前自行检查一份合并后的配置时（例如部署流水线），可复用同一套规则：

```rust
use webx_core::mode::AppMode;
use webx_host::config_validation::{format_violations, validate_options};

let violations = validate_options(&options, AppMode::Production);
if !violations.is_empty() {
    eprintln!("{}", format_violations(&violations)); // 一条消息列出全部违规
}
```

`validate_options` 覆盖只依赖 `AppOptions` 与 `AppMode` 的规则；依赖
`HostBuilder` 状态的检查（`use_cors` / `use_turn_config` / `JwtValidation`
覆盖、是否启用认证）由 Host 在 `try_build` 内一并收集，`format_violations`
负责稳定排序与汇总格式。

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
- **转发头信任只有一个入口**：`ClientInfo.TrustForwardedHeaders` / `ClientInfo.TrustedProxies`。
  旧的 `RateLimit.TrustProxy` / `RateLimit.TrustedProxies`（与 `RateLimiter::with_trust_proxy`）
  已在 2.9.1 删除：`RateLimitSection` 不再有这两个字段，写了也不生效（serde 忽略未知键）。
  把它们搬到 `ClientInfo` 即可；`RateLimit` 节只留 `Enabled` / `RequestsPerSecond` /
  `BurstSize` / `MaxTrackedIps`。迁移细节见
  [upgrade-to-2.9.1](../16-migration/upgrade-to-2.9.1.md)。
- **环境变量**：`APP__ClientInfo__TrustForwardedHeaders=true` 这类标量可覆盖；**数组不能**用
  `APP__ClientInfo__TrustedProxies__0=...` 按索引覆盖（当前实现只递归 JSON 对象，索引段会静默
  不生效或让严格绑定启动失败）。数组请在 `appsettings*.json` 或代码里配置。
- 代码构造 `RateLimitSection` 时同样只设置上面四个字段。
  `Idempotency.AllowAnonymous`（默认 `true`）控制匿名请求是否启用幂等；
  对外服务建议设为 `false`，见 [请求级幂等](../11-production/idempotency.md)。

### Idempotency 的路径过滤与自动装配

`Idempotency.Paths` / `SkipPaths` 把幂等收敛到需要保护的写接口（空 `Paths` = 全部路径，保持兼容；
`SkipPaths` 优先；按路径分段边界匹配，末尾 `/` 忽略）：

```json
{
  "Idempotency": {
    "Enabled": true,
    "Paths": ["/api/orders", "/api/payments"],
    "SkipPaths": ["/api/payments/webhook"]
  }
}
```

- `/api/orders` 命中 `/api/orders` 与 `/api/orders/123`，**不**命中 `/api/orders-archive`。
- 不在范围内的请求不读 `Idempotency-Key`、不访问存储（无存储往返）。
- `Enabled=true` 且未显式 `use_idempotency` 时，Host 自动使用 DI 中注册的
  `dyn IIdempotencyStore`（如参考适配器 `examples/adapters/idempotency/ef` 的 `EfIdempotencyStore`），
  没有才退回有界内存存储；显式 `use_idempotency(store, ttl)` 优先。
- `Production` 下 `Enabled=true` + `AllowAnonymous=true` + 空 `Paths` 会在启动时打 `warn`
  （所有路径的匿名写请求按 IP 分键，NAT/反代后可能互相重放），不改变行为。
- `LeaseSeconds`（默认 60）是进行中预约的租约，下传时取 `min(LeaseSeconds, TtlSeconds)`；
  `TtlSeconds` 是已完成响应的重放保留期。
- 其余键（`TtlSeconds` / `MaxTrackedKeys` / `MaxBodyBytes` / `AllowAnonymous`）语义见
  [请求级幂等](../11-production/idempotency.md)。

### Culture

`Culture.Default` 是**全局 UI culture**，决定端点名称/描述、OpenAPI 与端点目录的语言，
默认 `"en"`（也可用 `APP__Culture__Default` 覆盖）。它不跟随请求的 `Accept-Language`：权限资源、
菜单名称这类写入数据库的内容不应因访问者语言而漂移。多语言部署按 Host 配置切换，或由调用方
显式指定 culture。

资源默认从应用基准目录的 `locales/<culture>.json` 读取，只需：

```rust
Host::builder().use_localization(LocalizationOptions::new()).build();
```

单文件部署用 `use_localization(LocalizationOptions::none().resources(...))` 把资源编译进二进制。
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
        app.options(|opts| {
            println!("Listening on: {:?}", opts.app.urls);
        });
    })
```

`options` 是 `AppOptions` 的**唯一**入口（2.9.1 由 `HostAppBuilder::use_options` 改名，驼峰别名
`useOptions` 删除），只能通过 `configure` 触达。

自定义配置节（`add_options::<T>("Section")`）需要**绑定后再覆盖**时，用一行
`add_options_with`——覆盖闭包在绑定之后、创建三种注入实例之前运行：

```rust
Host::builder()
    .add_options_with::<SiteConfig>("Site", |s| s.public_url = std::env::var("SITE_URL").unwrap_or_default())
```

它只用于环境差异（CI 地址、本地开关）；生产弱密钥 / 通配 CORS 仍 fail-closed。
详见 [配置热更新与 Options 注册](../11-production/live-options.md)。

## 小结

`appsettings.json` 与 ASP.NET Core 格式兼容，降低迁移成本。

下一节：[AppMode 与环境切换](app-modes.md)
