# TOTP 与恢复码（`rust-webx-mfa`）

> 2.5 M2。独立 crate：只做 **验证/生成原语**，不包含用户表、二维码渲染或登录页面。

## 1. 注册（enrollment）

```rust
use webx_mfa::{TotpConfig, TotpSecret};

let secret = TotpSecret::generate();                 // 160-bit
let uri = secret.enrollment_uri("Travel", "alice@example.com", TotpConfig::default());
// 把 uri 渲染成二维码；把 secret.to_base32() 加密存储（不要日志/明文返回）
```

- 默认 `SHA1 / 6 位 / 30 秒`；支持 `SHA256`、`SHA512`，6–8 位，skew 0–2 步。
- `TotpSecret` 的 `Debug` 脱敏；base32 解析大小写/空格/连字符宽容。

## 2. 登录校验与重放防护

```rust
use webx_mfa::{TotpConfig, TotpSecret};

let verification = secret.verify(
    &submitted_code,
    now_unix_secs,
    TotpConfig::default(),
    last_accepted_step,          // 上次成功验证的 step（持久化列）
)?;
// 原子更新：last_accepted_step = verification.matched_step
```

- 验证按 ±`skew_steps` 计算，常量时间比较。
- 同一 step 或更早的 code 再次出现 → **401 重放**；`matched_step` 由调用方原子持久化（条件更新 `last_step < matched_step`）。
- 单次失败计数建议接 `ILoginThrottle`（M1-03），防止在线爆破 6 位码。

## 3. 恢复码

```rust
use webx_mfa::{RecoveryCodeSet, DEFAULT_RECOVERY_CODE_COUNT};

let (plaintext, set) = RecoveryCodeSet::generate(DEFAULT_RECOVERY_CODE_COUNT);
// 只把 plaintext 展示一次；持久化 set.records()（hash + used）

let mut set = RecoveryCodeSet::from_records(records);
if set.verify_and_consume(&candidate) {
    // 更新该条 used=true
}
```

- 10 个 10 字符码（去掉易混淆字符），展示形式 `XXXXX-XXXXX`；大小写/空格/连字符宽容。
- 存储只保存 `SHA-256(normalized)`，`hash_code` 会先归一化；**不存明文**。
- `verify_and_consume` 在内存集合内保证单次使用；数据库实现用 `used = 0` 条件更新取得同样的原子语义。
- 剩余数量用 `remaining()` 做 UI 提醒；用尽后要求重新生成。

## 4. `amr`/`acr` step-up

```rust
use webx_mfa::MfaAssurance;

let claims = MfaAssurance::totp().apply_to_claims(TokenClaims::new(&user.id));
// claims.extra: amr="pwd,totp", acr="urn:webx:acr:mfa"
let issued = token_issuer.issue(claims).await?;
```

- 需要二次验证的端点用 `#[authorize(dynamic)]` 或 Handler 检查 `acr == ACR_MFA`；`MfaAssurance::satisfies_step_up(acr)` 提供同一判定。
- 恢复码登录使用 `MfaAssurance::recovery_code()`，`amr` 标记 `recovery_code` 便于审计。
- 密码 + TOTP 的 `amr` 顺序固定为 `pwd,totp`；自定义流程用 `MfaAssurance::new`。

## 5. 边界

- 不做短信/邮件 OTP、推送、WebAuthn（WebAuthn 属 M3）；不做二维码渲染。
- 恢复码的持久化与并发原子更新由应用/存储层负责；框架给出 hash 契约与单次语义。
- TOTP 种子泄露等价于第二因子失效：存储加密、日志脱敏、禁止回显。

下一节：[JWT Bearer 认证](jwt-authentication.md)
