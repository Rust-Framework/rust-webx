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

## 与 provider 集成

`OpenAiProvider::new` 接受 `impl Into<SecretValue>`，并把密钥保存在 `SecretValue` 中：

```rust
let provider = OpenAiProvider::new("openai", client, key); // key: SecretValue
```

推荐用 `from_secret_store` 一步完成解析 + 构造；解析失败是 typed `SecretError`，不会把密钥
打进入参数或错误消息。

## 边界

- 已实现：端口、`SecretValue`、`SecretRef`、env/file/chain、provider 集成。
- 未实现：信封加密/KMS/Vault 实现、自动轮换与 key pool 健康、把 `SecretRef` 直接放进目录配置
  （应用在构建 provider 时解析）。
- 不要把 `SecretValue::expose()` 的返回值写日志、指标或错误详情；日志只记录名字。
- 环境变量在进程内可被 `std::env::var` 读到，容器env 仍建议用 secret 挂载而非镜像层。

## 测试

`crates/core/src/secrets.rs` 覆盖：env 读取/空值、`Debug` 脱敏、文件读取与换行裁剪、空文件、
路径穿越/分隔符拒绝、Unix `0600` 校验、链式 `NotFound`/`Unavailable` 语义；
`crates/ai/src/openai.rs` 覆盖 `from_secret_store` 成功与 typed 失败。
