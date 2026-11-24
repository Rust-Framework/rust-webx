# 密钥存储（ISecretStore）

Provider key、上游 webhook token、数据库密码不应出现在 `appsettings.json`、命令行或
`Debug` 日志里。框架把"按名字取密钥"做成端口 `ISecretStore`，内置环境变量与文件两种实现；
生产可以用 KMS/Vault/信封加密实现同一 trait，调用方不变。

```rust
use webx::*;

let store = ChainSecretStore::new()
    .push(Arc::new(EnvSecretStore::new()))
    .push(Arc::new(FileSecretStore::new("/run/secrets")));

let secret = store.get("OPENAI_API_KEY").await?;
let provider = OpenAiProvider::from_secret_store("openai", client, &store, "OPENAI_API_KEY").await?;
```

## 契约

| 项 | 值 |
|----|----|
| 端口 | `ISecretStore::get(name) -> Result<SecretValue, SecretError>`（async，KMS 可用） |
| 值与调试 | `SecretValue` 的 `Debug` 只输出 `<redacted>`，`Drop` 时 zeroize |
| 引用 | `SecretRef { name, version }` 存配置，轮换只改版本 |
| 找不到 | `SecretError::NotFound`（链式 store 继续找下一个） |
| 不可用 | `SecretError::Unavailable`（链式 store 立即失败，不降级） |

`SecretValue` 不实现 `Clone`：复制密钥会制造额外副本；需要共享时用 `Arc<SecretValue>`。

## 内置实现

### EnvSecretStore

读进程环境变量；值必须非空。适合容器 secret、本地开发：

```rust
let store = EnvSecretStore::new();
let key = store.get("OPENAI_API_KEY").await?;
```

### FileSecretStore

在一个目录下"一密钥一文件"，适合 Docker/Kubernetes secret 挂载：

```rust
let store = FileSecretStore::new("/run/secrets");
let key = store.get("openai-api-key").await?;
```

- 名字必须是普通文件名（`[A-Za-z0-9._-]`）；`..`、路径分隔符、绝对路径一律拒绝。
- 末尾的 `\r`/`\n` 会被去掉（`echo` 与挂载文件常见），其余内容原样使用。
- 空文件视为"没有该密钥"（`NotFound`）。
- Unix 上文件必须是 `0600`；group/world 可读直接返回 `Unavailable`（与 `Jwt.SecretFile` 同一规则）。
- 符号链接被拒绝，避免挂载点被替换后读到意外文件。
- 错误详情只包含路径与原因，不包含密钥内容。

### ChainSecretStore

```rust
let store = ChainSecretStore::new()
    .push(Arc::new(EnvSecretStore::new()))
    .push(Arc::new(FileSecretStore::new("/run/secrets")));
```

按注册顺序尝试；`NotFound` 继续，`Unavailable` 立即返回（避免"文件坏了就悄悄用环境变量"的
隐蔽降级）。全部 `NotFound` 时返回第一个 `NotFound`。

## 静态保护（ISecretProtector）

`ISecretStore` 解决"密钥从哪来"；落库字段（TOTP seed、供应商 refresh token）还需要
"存进去的是密文"。端口 `ISecretProtector` 提供 `protect` / `unprotect`；host 的
`AesGcmSecretProtector` 是 AES-256-GCM + 可轮换 key ring 实现：

```rust
let store = Arc::new(EnvSecretStore::new()) as Arc<dyn ISecretStore>;
let protector = Arc::new(
    AesGcmSecretProtector::from_secret_store(
        store.as_ref(),
        &["SECRET_PROTECTION_K1", "SECRET_PROTECTION_K2"], // 32 字节密钥的 base64
        "SECRET_PROTECTION_K2",                            // 当前写密钥
    )
    .await?,
);
Host::builder().use_secret_protector(protector).build();
```

- 密文自描述：`webx1:<key-id>:<base64url(nonce||ciphertext)>`，读取按内嵌 key id 选密钥；
- 轮换：新 ring 把 active 换成新 key、保留旧 key；旧行可读，新写入自动用新 key；
- 失败关闭：密文被篡改 / 未知 key / 前缀不符 → `SecretError::Unavailable`，绝不返回明文；
- `from_keys(Vec<(id, [u8; 32])>, active)` 用于测试或自定义加载；
- **同步组合根**：`build_host(config)` 是同步函数（在 `#[tokio::main]` / `#[tokio::test]` 内不能
  `block_on`）时用 `AesGcmSecretProtector::from_env_names(&ids, active)`，它同步读 env 并复用同一套
  32B/base64 校验；async 或 KMS/挂载文件场景仍用 `from_secret_store`（首选）；
- **错误映射**：`impl From<SecretError> for webx::Error` 已提供——`NotFound` → 500
  `internal_error`（`public_detail() = None`，密钥名只进日志），`Unavailable` → 503
  `service_unavailable`、`retryable = true`（客户端可据 `retryable` 重试）。Handler 里直接 `?`，
  不再需要本地 `map_protector_error`；默认 problem 渲染器对所有 5xx 统一使用稳定文案，
  不把密钥名/存储原因写进响应；
- KMS/Vault 实现同一 trait 即可，调用方不变；不要把远调用放进长事务或锁内。

部署语义：多实例必须共享同一 key ring（同一 secret store / KMS），否则 A 节点写的行 B 节点读不出；
容器重启安全（密钥来自 env/挂载 secret，不落本地盘）；单字段一次 AEAD（微秒级），不要保护大 blob。

## 与 provider 集成

`OpenAiProvider::new` 接受 `impl Into<SecretValue>`，并把密钥保存在 `SecretValue` 中：

```rust
let provider = OpenAiProvider::new("openai", client, key); // key: SecretValue
```

推荐用 `from_secret_store` 一步完成解析 + 构造；解析失败是 typed `SecretError`，不会把密钥
打进入参数或错误消息。

## 边界

- 已实现：端口、`SecretValue`、`SecretRef`、env/file/chain、provider 集成、静态保护端口
  （AES-256-GCM + key ring 轮换）。
- 未实现：KMS/Vault **适配实现**、不重启的热轮换（通过重新注册 protector 实现）、
  把 `SecretRef` 直接放进目录配置（应用在构建 provider / protector 时解析）。
- 不要把 `SecretValue::expose()` 的返回值写日志、指标或错误详情；日志只记录名字。
- 环境变量在进程内可被 `std::env::var` 读到，容器env 仍建议用 secret 挂载而非镜像层。

## 测试

`crates/core/src/secrets.rs` 覆盖：env 读取/空值、`Debug` 脱敏、文件读取与换行裁剪、空文件、
路径穿越/分隔符拒绝、Unix `0600` 校验、链式 `NotFound`/`Unavailable` 语义；
`crates/host/src/secret_protector.rs` 覆盖：往返/随机 nonce、篡改与未知 key 失败关闭、
轮换后旧行可读、key ring 校验、从 `ISecretStore` 加载、`from_env_names` 缺失/空值/非法
base64/短 key 的脱敏错误；`crates/ai/core/src/openai.rs`
覆盖 `from_secret_store` 成功与 typed 失败。
