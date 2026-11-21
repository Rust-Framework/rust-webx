# WebAuthn/Passkey

> 。独立 crate：只做 **RP 侧验签原语**，不包含浏览器 JS、二维码/条件 UI 或用户表。

## 1. 范围

`rust-webx-webauthn` 校验：

- registration：`clientDataJSON` + `attestationObject`（`none` 格式）+ `rawId`；
- assertion：`clientDataJSON` + `authenticatorData` + `signature` + `rawId`；
- 凭据：EC2/P-256、ES256（passkey 默认算法）；
- 标志：UP 必须、UV 可选强制、AT/ED 边界、`rpIdHash`、origin、challenge（常量时间）；
- 签名计数器：单调递增（双方都为 0 的 authenticator 例外）。

不包含：浏览器 `navigator.credentials` 封装、RS256/EdDSA、`packed`/`tpm`/`apple` 等 attestation 格式、条件 UI/自动填充。

## 2. 配置与 challenge

```rust
use webx_webauthn::{WebAuthnChallenge, WebAuthnConfig};

let config = WebAuthnConfig::new("example.com", "https://app.example.com")?
    .require_user_verification(true);

let challenge = WebAuthnChallenge::generate();
// 存入服务端会话（绑定 user + 过期时间；一次使用后删除）
let options_challenge = challenge.to_base64url();
```

`rp_id` 必须是裸域（无 scheme/端口/路径）；origin 必须 HTTPS。生产环境 `require_user_verification(true)` 以获得真正的第二因子。

## 3. 注册

浏览器把 `PublicKeyCredential` 的 `response.clientDataJSON`、`response.attestationObject`、`rawId`（base64url 解码后）回传：

```rust
use webx_webauthn::verify::verify_registration;

let registered = verify_registration(
    &config,
    &challenge,
    &client_data_json,
    &attestation_object,
    &raw_id_bytes,
)?;
// 持久化 registered.credential（credential_id/public_key/sign_count）
// registered.aaguid / registered.fmt 供审计
```

失败语义：`Validation`（结构/格式/类型错误）、`Unauthorized`（challenge/origin/rpId/flag 不匹配）→ 分别映射 400/401。

## 4. 登录（assertion）

```rust
use webx_webauthn::verify::verify_assertion;

let result = verify_assertion(
    &config,
    &stored_credential,
    &challenge,
    &client_data_json,
    &authenticator_data,
    &raw_id_bytes,
    &signature,
)?;
// 原子更新 sign_count = result.sign_count（并发下用条件更新 sign_count < new）
```

- 验证 `clientData.type == "webauthn.get"`、challenge、origin、`crossOrigin != true`。
- 验证 `rpIdHash == SHA256(rp_id)`、UP/UV、签名 `authData || SHA256(clientDataJSON)`。
- 计数器：`new > stored`；双方均为 0 的多设备 passkey 允许。
- 凭据已删除/计数器回退/签名失败一律 401。

## 5. Step-up 与恢复

- 成功后用 `MfaAssurance::new(["pwd","webauthn"], ACR_MFA)` 写 `amr`/`acr`（见 [TOTP 与恢复码](mfa.md)）。
- 每个用户注册多把 passkey；丢失全部凭据时走恢复码/人工审核。
- challenge 必须一次性、短 TTL（建议 ≤5 分钟）并绑定用户/会话；不要复用登录 CSRF token。

## 6. 限制与后续

- 仅 `none` attestation：需要设备证明时再启用 `packed`/`tpm` 等 verifier。
- 仅 ES256：RS256/EdDSA 依赖后续扩展（接口已按 COSE 解析设计）。
- 浏览器端 options/JSON 编解码、条件 UI、跨设备认证属于应用/前端职责。

下一节：[DPoP 发送方约束令牌](dpop.md)
