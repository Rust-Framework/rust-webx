# DPoP 发送方约束令牌（RFC 9449）

> 2.5 M3。DPoP 让 access token 与客户端私钥绑定：token 被窃后无法单独使用。

## 1. 协议形态

```
Authorization: DPoP <access_token>
DPoP: <proof JWT>
```

- access token 必须带 `cnf.jkt`（客户端公钥的 RFC 7638 thumbprint）。
- proof 是客户端用同一私钥签名的 JWT：header `typ=dpop+jwt`、非对称 `alg`、内嵌 `jwk`；
  claims 必须含 `htm`、`htu`、`iat`、`jti`、`ath`。
- 资源服务器校验 proof 签名、`cnf.jkt` 与内嵌 key 的 thumbprint 一致、请求方法/URI、时效、`ath`、`jti` 重放。

## 2. 服务端接入

```rust
use std::sync::Arc;
use webx::*;

let jwt_auth = Arc::new(JwtAuth::new(
    DecodingKey::from_secret(secret.as_bytes()),
    Validation::default(),               // 或 from_validation_with_resolver（JWKS）
));

let scheme = DpopScheme::new(
    jwt_auth,
    Arc::new(InMemoryDpopReplayStore::new()),
    DpopConfig::new("https://api.example.com")?, // htu 的 origin
)?;

Host::builder()
    .add_authentication_scheme(Arc::new(scheme))
    .build();
```

- `DpopConfig`：`expected_origin` 必须 HTTPS；`max_age` 1–600s；算法必须非对称（默认 ES256/RS256）。
- `DpopScheme` 只匹配 `Authorization: DPoP …` 且存在 `DPoP` 头；Bearer/API Key 不受影响。
- access token 仍由 `JwtAuth` 校验（iss/aud/exp/alg/JWKS）；DPoP 只增加发送方约束。

## 3. 校验细节

| 项 | 规则 |
|---|---|
| header `typ` | 必须 `dpop+jwt` |
| header `alg` | 在 `DpopConfig.algorithms` 内，且与 `jwk` 的 `kty` 兼容 |
| `jwk` | EC P-256 / RSA；thumbprint 必须等于 token `cnf.jkt`（常量时间比较） |
| `htm` | 与请求方法一致（大小写不敏感） |
| `htu` | `expected_origin + path`（不含 query/fragment） |
| `iat` | 不超前 60s，且不超过 `max_age` |
| `jti` | 非空；重放直接 401 |
| `ath` | `base64url(SHA256(access_token))`，必须匹配 |

- 重放存储：`IDpopReplayStore`；`InMemoryDpopReplayStore` 单进程，多实例用共享存储（Redis/DB）实现同一原子语义。
- proof 失败一律 401；JWKS/JWT 后端故障仍 503（fail-closed）。

## 4. 签发 DPoP-bound token

默认 `JwtTokenIssuer` 的 `TokenClaims.extra` 只支持字符串，无法直接写对象型 `cnf`。两种做法：

1. **自定义 `ITokenIssuer`**：在 payload 中写入 `"cnf": {"jkt": "<thumbprint>"}`（推荐，语义标准）。
2. 通过 `extra` 写入 `cnf` 的 JSON 字符串（本框架校验器兼容，但其它资源服务器可能要求对象）。

`jkt` 由客户端公钥计算：

```rust
let jwk: Jwk = serde_json::from_value(header_jwk)?;
let jkt = jwk.thumbprint().expect("supported key");
```

## 5. 边界

- DPoP 不替代 TLS；它防的是 token 被盗后的重放。
- 客户端 proof 的生成（刷新 token、密钥生命周期）属于客户端 SDK 职责。
- 与 OIDC `DPoP` token endpoint 的 `dpop_jkt` 参数/刷新绑定属于后续互操作收尾。

下一节：[密钥轮换与应急 Runbook](key-rotation-runbook.md)
