# 兼容性政策（SemVer、N-2 升级、锁步版本）

> 基线：rust-webx 2.7.0。门禁在 `.github/workflows/ci.yml` 的 `semver`、`upgrade`、
> `lint`（内部版本钉）三个 job。

## 1. 三条承诺

1. **公开 API 只加性演进**：2.x minor 不删除/改名公开项，不收紧泛型 bound，不移除
   auto trait，不给既有函数新增 `#[deprecated]`（会让下游 `-D warnings` 构建失败）。
2. **内部 crate 锁步发布**：`rust-webx*` 之间使用**精确版本** `=x.y.z` 依赖，
   消费者无法解析出 umbrella 与子 crate 混版本组合。
3. **N-2 源码兼容**：同一份只使用 2.0 API 的应用源码，必须能同时编译
   针对 `rust-webx = "=2.0.0"` 与当前工作区路径。

## 2. 门禁

### semver（对上一发布）

```text
cargo semver-checks --package rust-webx-core   --baseline-rev v2.1.0
cargo semver-checks --package rust-webx-engine --baseline-rev v2.1.0
cargo semver-checks --package rust-webx-host   --baseline-rev v2.1.0
cargo semver-checks --package rust-webx        --baseline-rev v2.1.0
```

唯一例外：`constructible_struct_adds_field = "allow"`（见 §3）。

### upgrade（N-2 源码兼容）

`compat/upgrade-smoke/src/main.rs` 只用 2.0 时代 API，分别在两个独立 workspace 编译：

```text
compat/upgrade-smoke/stable/Cargo.toml   → rust-webx = "=2.0.0"（固定 2.0 全套）
compat/upgrade-smoke/current/Cargo.toml  → rust-webx = { path = "../../../crates/webx" }
```

### 内部版本钉一致性

工作区所有 `path = "crates/..."` 内部依赖的 `version` 必须等于
`[workspace.package].version`；否则 CI 失败。

## 3. 已知例外：配置 section 的新增字段

配置 section（`ServerSection`、`JwtSection`、`TlsSection` …）是 serde DTO：
文档化构造路径是 `Default` + `appsettings.json` / `APP__*`，不是结构体字面量。
因此新增配置开关对文档化用法是加性的，`constructible_struct_adds_field` 设为 `allow`。

3.0 的第一个 minor 一次性为配置 section 加 `#[non_exhaustive]`（配迁移指南），随后删除
该例外。需要保持字面量构造的结构体（如 `ProblemDetails`）由
`crates/webx/tests/api_stability_test.rs` 锁定字段集合。

## 4. 边界约定：rust-ef 错误映射

`rust-ef` 是 Web 无关的 ORM：2.6 起它不再提供 `webx` feature，也不依赖
`rust-webx`。持久层错误到 HTTP 的翻译属于**应用边界**，由单独的适配 crate
`rust-webx-ef` 承担（应用按需依赖；core 与 umbrella 不链接它）：

- 不在 `rust-webx-core` / umbrella 增加 `ef` feature 或可选 `rust-ef` 依赖。即使
  默认关闭，也会把 ORM 概念带进 core 的 feature 图与发布清单，破坏“小核心”边界。
- 不启用 `rust-ef` 的 `features = ["webx"]`，不恢复 `compat/webx-core-shim` 或
  `[patch.crates-io]`。
- 应用侧不写 `impl From<EFError> for webx::Error`（孤儿规则），也不重复实现映射：
  依赖 `rust-webx-ef`，调用点写 `.map_ef()?`。

```rust
// Cargo.toml
rust-webx-ef = "2.7"

// handlers/src/db.rs
pub use webx_ef::{map_ef_error, EfResultExt};

// Handler
let blogs = ctx.set::<Blog>().query().to_list().await.map_ef()?;
```

映射契约（与 rust-ef 2.6 迁移指南一致）：

| `EFError` | HTTP | `webx::Error` | 客户端可见 detail |
|---|---|---|---|
| `NotFound` | 404 | `NotFound` | 业务文案（原样） |
| `ModelValidation` | 400 | `Validation` | 业务文案（原样） |
| `ConcurrencyConflict` | 409 | `Conflict` | 业务文案（原样） |
| `Query`、`TypeConversion` | 400 | `Validation` | 固定文案（不回显 SQL/列名） |
| `UniqueViolation` | 409 | `Conflict` | 固定文案（不回显 constraint/table） |
| `Connection`、`Transaction` | 503 | `Status(503, _)` | 固定文案、可重试 |
| `Database`、`Migration`、`Configuration`、`Provider`、`ChangeTracking`、`Other` | 500 | `Internal` | 无（`public_detail() = None`，原文与 source 链只进日志） |

5xx 的原文与 `source` 链拼进 `Error::Internal`，`public_detail()` 为 `None`，由
host 写入日志。新增 rust-ef 变体走同一隐藏 500 分支，升级依赖不会扩大客户端可见
信息。参考实现见 docbit/dmbit 的 `handlers/src/db.rs`（已改为 re-export 适配 crate）。

若未来确需框架级桥接，只能另立**独立集成 crate**（依赖方向
rust-webx → rust-ef），单独评审；本仓 core/umbrella 始终零 ef 依赖，也不允许
ORM → Web 框架。

## 5. 弃用政策

- **2.x 不新增 `#[deprecated]`**：等价做法是"文档标注 + 迁移指南 + 兼容层"。
  `init_jwt_secret` / `jwt_secret()` 是带文档说明的兼容 shim，不加属性。
- **3.x 起**：窗口 ≥2 个 minor（N 标记 → 最早 N+2 移除），每项必须在 CHANGELOG、
  迁移指南和 rustdoc 三处给出替代 API；运行时行为不变；锁步 crate 一起弃用。
- 登记表见 [`deprecations.md`](deprecations.md)。

## 6. 语义稳定面

以下内容在 2.x 内**只加不改**；任何行为变更必须在 CHANGELOG 标注 `behavior change`
并提供 opt-in 开关：

| 面 | 具体内容 |
|---|---|
| HTTP 错误 | `ProblemDetails` 字段与语义、`code`/`status` 映射、`WWW-Authenticate` 行为 |
| 绑定/路由 | 绑定失败 400/415 语义、`#[frompath]`/`#[fromquery]`/`#[fromclaim]` 规则、路由优先级 |
| 中间件 | `IMiddleware` 的 `invoke`/`after`/`ControlFlow` 语义与 `Next` continuation |
| 实时 | WS 帧协议、关闭码 1000/1001/1008、`WebSocketLimits` 默认值 |
| 观测 | 指标名与标签（`http_requests_total`、`realtime_*`、`media_*`）、`/metrics` 文本格式 |
| 配置 | `AppOptions` 键名、`APP__Section__Key` 覆盖规则、`Jwt.SecretFile` 优先级 |
| 端口 | `IObjectStore`/`IMediaPlaneAdapter` 方法语义、`RealtimeFanoutEvent` 事件名 |

## 7. LTS 与发布检查单

- 支持窗口：当前 minor 全程支持；N-1、N-2 只接收安全与关键缺陷修复。
- LTS 分支：每个 stable major 的最后一个 minor 建 `lts/<major>.<minor>`，维护 12 个月，
  只接受 backport 的安全/文档/兼容修复。
- 补丁版本只修 bug/安全问题，不删 API、不改默认值、不改语义。
- 跨 minor 升级零代码改动（N-2 冒烟门禁）；跨 major 提供迁移指南与弃用窗口。

```text
□ CI 全绿（tests/clippy/fmt/MSRV/audit/docs/coverage/benches/alloc/semver/upgrade）
□ CHANGELOG 含迁移说明（弃用/行为变更）
□ semver-checks 对上一发布通过（例外仅 §3）
□ N-2 冒烟通过（compat/upgrade-smoke）
□ cargo audit 0 漏洞 + 内部版本钉一致
□ LTS backport 只含安全/关键缺陷修复
```

## 8. 例外维护与本地运行

新增 `cargo-semver-checks` 例外必须：在本文档记录原因/影响面/收口版本；用
`api_stability_test` 锁定文档化构造路径；指定负责人与移除检查项。

```text
cargo semver-checks --package rust-webx-host --baseline-rev v2.1.0
cargo build --manifest-path compat/upgrade-smoke/stable/Cargo.toml
cargo build --manifest-path compat/upgrade-smoke/current/Cargo.toml
```
