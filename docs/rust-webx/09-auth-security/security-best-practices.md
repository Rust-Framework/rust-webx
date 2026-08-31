# 安全最佳实践

## 密钥管理

| 实践 | 说明 |
|------|------|
| 生产环境使用强密钥 | JWT Secret ≥ 256 bit 随机值 |
| 密钥不入代码库 | 通过环境变量或密钥管理服务注入 |
| 开发/生产分离 | `appsettings.Development.json` vs 生产配置 |

### `jwt_secret()` 进程级 shim

`HostBuilder::add_authentication()` 在 build 时调用 `init_jwt_secret()`，登录 Handler 通过 `jwt_secret()` 读取签名密钥。这是**独立于 DI `ServiceProvider` 的配置单例**（`OnceLock`），与 Phase 4 废弃的 `global_provider()` 不同：JWT 密钥不是 per-request 依赖，且需在 middleware 与 Handler 间共享。

- **推荐**：保持 `add_authentication()` + `Jwt.Secret` / `JWT_SECRET` 环境变量；Handler 内继续用 `jwt_secret()` 签发 token。
- **不推荐**：在 Handler 中硬编码密钥或绕过 `init_jwt_secret`。
- **未来**：若需多 Host 实例各用不同密钥，需显式重构（例如经 `IHttpContext` 或 DI 注入 `JwtOptions`）；当前单 Host 进程模型下 shim 可接受。

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

- 最小权限原则：默认 `#[authorize]`，公开端点显式标记
- 敏感操作要求特定 role/permission
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
