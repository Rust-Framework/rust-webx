# mTLS 客户端证书认证

> 2.5 M3。TLS 终止层验证客户端证书链，框架把叶证书指纹交给认证方案做身份映射。

## 1. 启用 mTLS

```jsonc
{
  "Tls": {
    "CertPath": "certs/server.pem",
    "KeyPath": "certs/server.key",
    "ClientCaPath": "certs/client-ca.pem",
    "ClientAuth": "Required"   // Off | Optional | Required
  },
  "App": { "Urls": ["https://0.0.0.0:8443"] }
}
```

- `Required`：握手必须提供由 `ClientCaPath` 链验证通过的证书；`Optional`：请求但允许无证书。
- `ClientAuth != Off` 但 CA 为空 / 没有 HTTPS listener → 启动失败（fail-closed）。
- `ClientCaPath` 设置但 `ClientAuth=Off` 也视为配置错误。

## 2. 验证与身份映射

1. rustls 验证客户端证书链和有效期。
2. engine 计算叶证书 DER 的 **SHA-256 指纹（小写 hex）**，写入请求头
   `x-webx-client-cert-sha256`。
3. engine 在同一条连接上**先删除客户端自带的同名头**，只注入 TLS 验证结果，
   因此普通 HTTP 或上游无法伪造。
4. `MtlsScheme` 读取该头，调用应用的 `IMtlsIdentityResolver` 映射为身份：

```rust
use async_trait::async_trait;
use std::sync::Arc;
use webx::*;

struct Certs;

#[async_trait]
impl IMtlsIdentityResolver for Certs {
    async fn resolve(&self, fingerprint: &str) -> Result<Option<Box<dyn IClaims>>> {
        // 查证书注册表；数据库/缓存均可
        match lookup(fingerprint).await? {
            Some(user) => Ok(Some(Box::new(JwtClaims::new(user.id)))),
            None => Ok(None), // → 401，绝不匿名放行
        }
    }
}

Host::builder()
    .add_authentication_scheme(Arc::new(MtlsScheme::new(Arc::new(Certs))))
    .build();
```

- 指纹未知 → 401（不是匿名）。
- 吊销证书 = 从注册表删除/标记，下一次请求即失效；证书链仍由 TLS 层校验。
- 多实例共享证书注册表；mTLS 不替代应用层授权。

## 3. 反向代理边界

如果 TLS 在网关终止，engine 看不到客户端证书。此时：

- 网关必须做 mTLS 并**覆盖** `x-webx-client-cert-sha256`；
- 应用只信任来自该网关的流量（网络策略/共享密钥），否则该头可被伪造；
- 更稳妥的是让 rust-webx 直接终止 TLS。

## 4. 测试与限制

- 引擎：CA 缺失/策略为 Off 但设置 CA/Required+Optional 构建成功（10 例 TLS 测试）。
- Host：已知指纹认证、未知指纹 401、缺头不匹配（3 例）。
- 只支持 rustls 的 WebPKI 客户端证书链（不支持自签 SPIFFE 以外的自定义 verifier）。
- 证书主体/SAN 解析不在本层：身份由指纹注册表决定，避免 DN 解析歧义。

下一节：[DPoP 发送方约束令牌](dpop.md)
