# 兼容性政策（SemVer、N-2 升级、锁步版本）

> 基线：rust-webx 2.9.0。门禁在 `.github/workflows/ci.yml` 的 `semver`、`upgrade`、
> `lint`（内部版本钉）三个 job。

## 1. 三条承诺

1. **公开 API 只加性演进**：2.x minor 不删除/改名公开项，不收紧泛型 bound，不移除
   auto trait，不给既有函数新增 `#[deprecated]`（会让下游 `-D warnings` 构建失败）。
2. **内部 crate 锁步发布**：`rust-webx*` 之间使用**精确版本** `=x.y.z` 依赖，
   消费者无法解析出 umbrella 与子 crate 混版本组合。
3. **N-2 源码兼容**：同一份只使用 2.7 API 的应用源码，必须能同时编译
   针对 N-2 发布 `rust-webx = "=2.7.0"`（2.9 的 N-2）与当前工作区路径。

## 2. 门禁

### semver（对上一发布）

```text
cargo semver-checks --package rust-webx-core   --baseline-rev v2.7.0
cargo semver-checks --package rust-webx-engine --baseline-rev v2.7.0
cargo semver-checks --package rust-webx-host   --baseline-rev v2.7.0
cargo semver-checks --package rust-webx        --baseline-rev v2.7.0
```

例外：`constructible_struct_adds_field = "allow"`（见 §3），以及 2.8.0 幂等值类型的
`struct_marked_non_exhaustive` / `enum_marked_non_exhaustive = "allow"`（同见 §3）。

### upgrade（N-2 源码兼容）

`testing/upgrade-smoke/src/main.rs` 只用 2.7 时代 API，分别在两个独立 workspace 编译。
N-2 基线随当前版本滚动：2.9.0 的 N-2 是 **rust-webx 2.7.0**（下一个版本发布时同步前移）。

```text
testing/upgrade-smoke/stable/Cargo.toml   → rust-webx = "=2.7.0"（固定 2.7 全套）
testing/upgrade-smoke/current/Cargo.toml  → rust-webx = { path = "../../../crates/webx" }
```

### 内部版本钉一致性

工作区所有 `path = "crates/..."` 内部依赖的 `version` 必须等于
`[workspace.package].version`；否则 CI 失败。

## 3. 已知例外：幂等值类型的新增字段与 `#[non_exhaustive]`（配置 section / `IdempotentResponse`）

`constructible_struct_adds_field`、`struct_marked_non_exhaustive` 与
`enum_marked_non_exhaustive` 设为 `allow`，覆盖以下已文档化的 2.8.0 变更：

1. **配置 section**（`ServerSection`、`JwtSection`、`IdempotencySection` …）是 serde DTO：
   文档化构造路径是 `Default` + `appsettings.json` / `APP__*`，不是结构体字面量。
   因此新增配置开关对文档化用法是加性的。3.0 的第一个 minor 一次性为配置 section 加
   `#[non_exhaustive]`（配迁移指南），随后删除该部分例外。
2. **2.8.0 的 `IdempotentResponse`** 新增 `headers`（缓存响应的可重放响应头），只破坏
   结构体字面量构造；同时与 `IdempotencyLease`、`IdempotencyBegin` 一起标记
   `#[non_exhaustive]`，以便 2.x 内继续加入状态/元数据而不再产生破坏性版本。
   CHANGELOG `[2.8.0]` 的 Breaking 段给出迁移写法：`IdempotentResponse` 用
   `::new(status, content_type, content_encoding, body)` + `.with_headers(..)` 构造，
   `IdempotencyLease` 用 `::new(token)`，`match IdempotencyBegin` 补 `_` 通配分支。
   `IIdempotencyStore` 仍保持对象安全，新增的租约方法都有默认实现。
   该部分与配置 section 同窗口在 3.0 复核收口。

需要保持字面量构造的结构体（如 `ProblemDetails`）由
`crates/webx/tests/api_stability_test.rs` 锁定字段集合。

## 4. 边界约定：rust-ef 错误映射

`rust-ef` 是 Web 无关的 ORM：2.6 起它不再提供 `webx` feature，也不依赖
`rust-webx`。持久层错误到 HTTP 的翻译属于**应用边界**，由单独的适配 crate
`rust-webx-ef` 承担（应用按需依赖；core 与 umbrella 不链接它）：

配对发布：先 rust-ef 2.8.0、后 rust-webx 2.9.0；workspace 依赖写 `rust-ef = "2.6"`
（semver 范围，已含 2.8.x，本版按 2.8.0 验证），不收紧为精确 pin——否则在 ORM 发布前
`Cargo.lock` 不可解析。

- 不在 `rust-webx-core` / umbrella 增加 `ef` feature 或可选 `rust-ef` 依赖。即使
  默认关闭，也会把 ORM 概念带进 core 的 feature 图与发布清单，破坏“小核心”边界。
- 不启用 `rust-ef` 的 `features = ["webx"]`，不恢复历史上的 `webx-core-shim` 兼容层或
  `[patch.crates-io]`。
- 应用侧不写 `impl From<EFError> for webx::Error`（孤儿规则），也不重复实现映射：
  依赖 `rust-webx-ef`，调用点写 `.map_ef()?`。

```rust
// Cargo.toml
rust-webx-ef = "2.9"

// handlers/src/db.rs
pub use webx_ef::{map_ef_error, EfResultExt};

// Handler
let blogs = ctx.set::<Blog>().query().to_list().await.map_ef()?;
```

映射契约（与 rust-ef 2.6+ 迁移指南一致；本版按 2.8.0 配对验证）：

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

### 配对验证（pairing verified）的证据边界

发布顺序是 **rust-ef 2.8.0 → rust-webx 2.9.0**（见上）。发布前用本地 rust-ef
checkout 做编译级配对验证：

```bash
# 在 rust-webx 仓库根执行；RUST_EF_PATH 指向本地 rust-ef 仓库根。
RUST_EF_PATH=/path/to/rust-ef bash scripts/check-pairing-rust-ef.sh
```

脚本在临时目录生成 `[patch.crates-io]`（只把 `rust-ef` / `rust-ef-sqlite` 指向
本地源码树，其余依赖仍走 crates.io），先对齐 `Cargo.lock` 再执行
`cargo check --workspace --locked` 与 `cargo test -p rust-webx-ef --locked`；
退出时恢复原 `Cargo.lock`，不污染工作树。CI 的可选 `pairing` job 只在配置了
`vars.RUST_EF_PATH`（自托管 runner 上已有 checkout）或
`vars.RUST_EF_REPO`（+ `RUST_EF_REF` / `rust_ef_ref` dispatch 输入，由 job 自行
checkout）时运行；未配置时打印 notice 并跳过，不阻塞普通 PR。

**"pairing verified" 只证明**：给定的 rust-ef 源码树能让 rust-webx 工作区与其
EF 适配层编译通过，并让 `rust-webx-ef` 的测试通过。**不证明**：crates.io 上已发布
制品可下载/可解析、该 rust-ef 树自身的 semver 门禁、也不覆盖真 PostgreSQL/MySQL
（由 CI 真库 job 覆盖）。发布记录里写证据时应同时记录 rust-ef 的 commit/tag 与
rust-webx 的 commit，例如 "pairing verified: rust-ef@<sha> × rust-webx@<sha>"。

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
□ 配对 ORM 已先行发布（rust-ef x.y.z → rust-webx x.y.z；`rust-ef = "2.6"` 范围覆盖 2.8.x，不收紧为精确 pin）
□ 配对验证已跑：`RUST_EF_PATH=… bash scripts/check-pairing-rust-ef.sh`（或 CI `pairing` job）通过，证据边界见 §4
□ semver-checks 对上一发布通过（例外仅 §3）
□ N-2 冒烟通过（testing/upgrade-smoke；2.9.0 的 N-2 = rust-webx 2.7.0）
□ cargo audit 0 漏洞 + 内部版本钉一致
□ LTS backport 只含安全/关键缺陷修复
```

## 8. 例外维护与本地运行

新增 `cargo-semver-checks` 例外必须：在本文档记录原因/影响面/收口版本；用
`api_stability_test` 锁定文档化构造路径；指定负责人与移除检查项。

```text
cargo semver-checks --package rust-webx-host --baseline-rev v2.7.0
cargo build --manifest-path testing/upgrade-smoke/stable/Cargo.toml
cargo build --manifest-path testing/upgrade-smoke/current/Cargo.toml
```
