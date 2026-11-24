# TOTP 与恢复码（`rust-webx-mfa`）

> 。独立 crate：只做 **验证/生成原语**，不包含用户表、二维码渲染或登录页面。

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
- 单次失败计数建议接 `ILoginThrottle`，防止在线爆破 6 位码。

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

数据库适配器不需要再算一遍 hash：`find` / `consume_matched` 直接返回**命中的记录**（含 hash），
用它做单条条件更新即可：

```rust
// 先读记录到 set（或直接按 hash 查询），再 CAS
if let Some(record) = set.find(&candidate) {
    let affected = sqlx::query(
        "UPDATE mfa_recovery_codes SET used_at = ? \
         WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
    )
    .bind(now)
    .bind(&user.id)
    .bind(&record.hash)
    .execute(&pool)
    .await?
    .rows_affected();
    // affected == 0：已被并发消费（重放）
}
```

- 10 个 10 字符码（去掉易混淆字符），展示形式 `XXXXX-XXXXX`；大小写/空格/连字符宽容。
- 存储只保存 `SHA-256(normalized)`，`hash_code` 会先归一化；**不存明文**。
- 内存集合：`consume_matched` 返回消费前的记录并标记 `used`；`verify_and_consume` 是其布尔薄包装。
- 数据库实现用 `used_at IS NULL` 条件更新取得同样的原子语义；0 行受影响 = 已消费。
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

## 5. 登录流配方（两步签发）

账号模型（用户表、`mfa_enabled`、`last_accepted_step`、恢复码记录）由应用持久化，框架原语按下面
顺序组合：

```rust
use webx::security::{ISessionStore, ITokenIssuer, TokenClaims};
use webx_mfa::{MfaAssurance, TotpConfig, AMR_PASSWORD};

// 1) 密码步：验证密码后看账号是否启用 MFA
if !hasher.verify(&password, &account.password_hash)? {
    return Err(Error::Unauthorized("invalid credentials".to_string()));
}
if account.mfa_enabled {
    // 受限 token：只有 amr=pwd、没有 acr、短 TTL；或返回 mfa_required + OneTimeSecret challenge
    let claims = TokenClaims::new(&account.id).with_claim("amr", AMR_PASSWORD);
    let limited = token_issuer.issue(claims).await?;
    return Ok(LoginStep::MfaRequired { limited_token: limited.token });
}
let full = token_issuer.issue(TokenClaims::new(&account.id)).await?; // 未启用 MFA 的账号

// 2) 第二因子步：用受限 token 定位用户，TOTP 与恢复码二选一
let verification = account.totp_secret.verify(
    &submitted_code, now_unix_secs, TotpConfig::default(), account.last_accepted_step,
)?;
account.last_accepted_step = Some(verification.matched_step); // 条件更新 last_step < matched_step

// 3) 升级签发：amr=pwd,totp、acr=urn:webx:acr:mfa
let claims = MfaAssurance::totp().apply_to_claims(TokenClaims::new(&account.id));
let full = token_issuer.issue(claims).await?;
session_store.revoke(&limited_jti).await?; // 撤销第 1 步的受限会话

// 4) step-up 端点：敏感操作要求第二次验证
if !MfaAssurance::satisfies_step_up(claims.extra.get("acr").map(String::as_str)) {
    return Err(Error::Forbidden("mfa step-up required".to_string()));
}
```

- 密码步的 token **不能**满足 step-up（`satisfies_step_up(None) == false`）：这是"限制 token"的
  关键性质，`crates/auth/mfa/tests/login_flow_test.rs` 覆盖了它。
- 第二因子失败统一 `401`，不要区分"码错/过期/未启用"，避免枚举；接 `ILoginThrottle` 防在线爆破。
- TOTP 与恢复码成功后都升级到同一 `ACR_MFA`，用 `amr` 区分方法便于审计；恢复码用尽提醒重新生成。
- 多实例部署时 `last_accepted_step` 与恢复码 `used` 必须走条件更新（`last_step < ?` / `used = 0`），
  由数据库唯一性/行锁保证跨节点单次使用。

## 6. 边界

- 不做短信/邮件 OTP、推送、WebAuthn（WebAuthn 由独立 crate `rust-webx-webauthn` 提供）；不做二维码渲染。
- 恢复码的持久化与并发原子更新由应用/存储层负责；框架给出 hash 契约与单次语义。
- TOTP 种子泄露等价于第二因子失效：存储加密、日志脱敏、禁止回显。

下一节：[JWT Bearer 认证](jwt-authentication.md)
