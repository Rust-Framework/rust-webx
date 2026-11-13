# 安全最佳实践

## 密钥管理

| 实践 | 说明 |
|------|------|
| 生产环境使用强密钥 | JWT Secret ≥ 256 bit 随机值 |
| 密钥不入代码库 | 通过环境变量或密钥管理服务注入 |
| 开发/生产分离 | `appsettings.Development.json` vs 生产配置 |
| 密钥可放文件 | `Jwt.SecretFile` 指向 `0600` 文件，启动期校验长度 ≥32 字节与权限（1.3.0 起） |

### `jwt_secret()` 进程级 shim

`HostBuilder::add_authentication()` 在 build 时调用 `init_jwt_secret()`，登录 Handler 通过 `jwt_secret()` 读取签名密钥。这是**独立于 DI `ServiceProvider` 的配置单例**（`OnceLock`）：JWT 密钥不是 per-request 依赖，且需在 middleware 与 Handler 间共享。

- **推荐**：保持 `add_authentication()` + `Jwt.Secret` / `JWT_SECRET` 环境变量；Handler 内继续用 `jwt_secret()` 签发 token。
- **不推荐**：在 Handler 中硬编码密钥或绕过 `init_jwt_secret`。

### 从文件加载 JWT 密钥（1.3.0 起）

密钥入 Git / 入镜像都不安全，环境变量又可能出现在进程环境或容器编排文件里；把密钥放进
一个只有服务账号可读的文件是常见折中：

```json
{ "Jwt": { "SecretFile": "data/jwt.key" } }
```

```text
# Unix：文件内容 trim 后 >= 32 字节，权限必须是 0600（或更严格）
$ head -c 48 /dev/urandom | base64 > data/jwt.key && chmod 600 data/jwt.key
```

- 解析顺序：`APP__Jwt__Secret` / `JWT_SECRET` > `Jwt.Secret` > `Jwt.SecretFile`；
  `Jwt.Secret` 非空时文件**不会被读取**。
- 启动期 fail-fast：文件缺失/不可读/空/<32 字节/（Unix）group-or-other 权限 → **启动失败**，
  错误信息包含路径与修复建议，不会用坏密钥继续签发 token。
- `Jwt.SecretFile` 由 host 在装配认证前解析（`apply_jwt_secret_file`），因此 `JwtTokenIssuer`、
  `JwtAuth`、`jwt_secret()` 与 Production 弱密钥守卫看到的都是解析后的值。
- **Windows**：不校验 ACL（无 POSIX mode），需要部署脚本把文件 ACL 限定为服务账号 +
  `Administrators`；DPAPI 包装属于后续能力，1.3.0 未提供。
- 框架**不生成**密钥文件（避免悄悄写入低熵密钥）：首次部署由安装器/运维生成。
- 相对路径按进程工作目录解析（与 `Tls.CertPath` 一致）。详见
  [appsettings 配置体系](../10-configuration/appsettings.md#jwt-密钥来源130-起)。

## 安全原语：常量时间比较与一次性密钥（1.3.0 起）

两个常见需求过去只能由业务自己实现（一次性安装 token、邀请链接、密码重置、webhook 签名），
1.3.0 起由 `webx::security` 提供，避免各处手写比较与「先检查后消费」的竞态：

### `constant_time_eq`

```rust
use webx::security::constant_time_eq;

assert!(constant_time_eq(b"token", b"token"));
assert!(!constant_time_eq(b"token", b"tokeX"));   // 不会在首个不同字节处提前返回
```

- 循环跑满 `max(len_a, len_b)` 次，差异累积到一个字节后再判断，耗时与「从哪里开始不同」无关；
  普通 `==` 在首个不同字节处短路，会泄露共享前缀长度。
- **长度不隐藏**（耗时随较长输入增长，返回值也区分长度）：密钥长度本来就是公开常量，
  需要连长度都保密时请先自行填充到定长。
- 典型用途：比较 header/query 里带来的 token、校验 webhook 签名、比较文件里的密钥。

### `OneTimeSecret`

TTL + 单次消费（CAS）的一次性密钥槽位：

```rust
use std::time::Duration;
use webx::security::{OneTimeSecret, OneTimeSecretState};

let setup = OneTimeSecret::new(Duration::from_secs(30 * 60));
setup.issue("token-from-a-csprng")?;                 // 值由调用方生成（框架不提供 RNG）
assert_eq!(setup.state(), OneTimeSecretState::Pending);

// 请求里拿到的候选值：检查并消费是同一个原子步骤
if !setup.consume(candidate) {
    // 分类失败原因：已用/过期 => 410，其它 => 401
}
```

| 行为 | 说明 |
|------|------|
| 单次消费 | N 个并发请求带同一值，**恰好 1 个** `consume` 返回 `true`（`Mutex` 内完成 CAS） |
| 错误值不消耗 | 猜错不烧掉密钥，合法持有者仍可使用（避免 spraying 把用户锁死） |
| TTL | 用 `Instant` 计时；`elapsed >= ttl` 后 `consume`/`matches` 都失败 |
| 重新签发 | 再次 `issue` 会替换并重置状态（旧值立即失效） |
| `matches` | **不消费**的常量时间比较，仅用于诊断（如已消费 token 重放 → 410 而非 401），不能当作鉴权闸门 |
| `state()` | `Empty` / `Pending` / `Expired` / `Consumed`，用于区分 401 与 410 |
| `revoke()` | 主动作废当前未消费密钥 |

- 值由调用方生成：框架不引入 RNG 依赖，请用应用已有的 `uuid::Uuid::new_v4()`、`getrandom`
  等生成高熵值后 `issue`；空值会被拒绝（`Error::Validation`）。
- 只存哈希：`issue` 存「已哈希值」、`consume` 传「候选值的哈希」即可，比较仍是常量时间，
  明文不进入内存存储（框架不带哈希实现，用应用已有的 SHA-256）。
- 进程内存储：进程重启后未消费密钥即失效；多副本部署要用共享存储（数据库/缓存）自行实现
  同样的「一次性」语义。

## HTTPS

```json
{
  "Tls": {
    "CertPath": "/path/to/cert.pem",
    "KeyPath": "/path/to/key.pem"
  }
}
```

生产环境**必须**启用 TLS。

## 认证安全

- Token 过期时间合理（Docbit 默认 24 小时）
- 密码使用 bcrypt 哈希存储
- 登录失败不泄露用户是否存在

## 授权安全

- 最小权限原则：受保护端点显式 `#[authorize]`，公开端点不标注
- 敏感操作要求特定 role/permission；需要任一角色时用 `roles = ["a", "b"]`（多个 `#[authorize]` 之间是 AND）
- 动态授权用 `IDynamicAuthorizer`，按 `(method, route_pattern)` 缓存授权结果；不要每次请求查库
- `IDynamicAuthorizer` 只做路由级判断；资源所有权/租户隔离在 Handler 内用 `req.user.tenant_id` /
  `req.user.id` 完成，不要从请求参数取租户（见[用户身份与多租户](../05-request-pattern/user-identity.md)）
- 不在客户端存储 Secret

## 响应头

`SecurityHeadersMiddleware` 自动添加：
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY`
- `Referrer-Policy`

## 速率限制

对登录、注册等端点启用 `RateLimitMiddleware` 防止暴力破解。

## 小结

安全是配置 + 代码的双重保障：框架提供机制，开发者遵循最佳实践。

下一章：[配置与环境](../10-configuration/INDEX.md)
