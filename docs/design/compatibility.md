# 兼容性政策（SemVer、N-2 升级、锁步版本）

> 基线：v2.2 开发期。门禁实现在 `.github/workflows/ci.yml` 的 `semver`、
> `upgrade`、`lint`（内部版本钉检查）三个 job。

## 1. 三条承诺

1. **公开 API 只能加性演进**：2.x minor 不删除/改名公开项，不收紧泛型 bound，
   不移除 auto trait，不给既有函数加 `#[deprecated]`（这会让下游 `-D warnings` 构建失败）。
2. **内部 crate 锁步发布**：`rust-webx*` 之间使用**精确版本** `=x.y.z` 依赖，
   防止消费者解析出"umbrella 2.0 + host 2.1"这类混版本组合（2.0.0 的 umbrella
   仍引用 `webx_host::context`，而 2.1 移除了该模块——这正是本政策要避免的事故）。
3. **N-2 源码兼容**：同一份只使用 2.0 API 的应用源码，必须能同时编译
   针对 `rust-webx = "=2.0.0"` 与当前工作区路径。

## 2. 门禁

### `semver`（对上一发布）

```text
cargo semver-checks --package rust-webx-core   --baseline-rev v2.1.0
cargo semver-checks --package rust-webx-engine --baseline-rev v2.1.0
cargo semver-checks --package rust-webx-host   --baseline-rev v2.1.0
cargo semver-checks --package rust-webx        --baseline-rev v2.1.0
```

- CI 用 `taiki-e/install-action` 安装工具；本地可用 GitHub Release 的预编译包。
- 当前 4 个包全部通过（core 228 checks / engine / host / webx）。
- 唯一配置的例外：`constructible_struct_adds_field = "allow"`（见第 3 节）。

### `upgrade`（N-2 源码兼容）

`compat/upgrade-smoke/src/main.rs` 只用 2.0 时代的 API（`#[get]`/`#[handler]`、
`Host::builder`、`ProblemDetails::new`、`PagedRequest::from_parts`、`RateLimiter`），
分别在两个独立 workspace 中编译：

```text
compat/upgrade-smoke/stable/Cargo.toml   → rust-webx = "=2.0.0"（并固定 2.0 全套）
compat/upgrade-smoke/current/Cargo.toml  → rust-webx = { path = "../../../crates/webx" }
```

### 内部版本钉一致性（`lint` job）

工作区所有 `path = "crates/..."` 的内部依赖，其 `version` 必须等于
`[workspace.package].version`；否则 CI 失败。

## 3. 已知例外：配置 section 的新增字段

配置 section（`ServerSection`、`JwtSection`、`TlsSection` …）是 **serde DTO**：
文档化构造路径是 `Default` + `appsettings.json` / `APP__*`，不是结构体字面量。
因此新增配置开关对文档化用法是加性的，`constructible_struct_adds_field` 设为
`allow`。

**收口计划**：v2.4 语义稳定窗口的决定是**推迟到 3.0**——2.x 仍是加性演进线，给
配置 section 加 `#[non_exhaustive]` 属于破坏性变更，与"2.x minor 不破坏"承诺冲突。
3.0 的首个 minor 将一次性为所有配置 section 加 `#[non_exhaustive]`（配迁移指南），
随后删除该例外。需要保持字面量构造的结构体（如 `ProblemDetails`）由
`crates/webx/tests/api_stability_test.rs` 锁定字段集合。

## 4. 本轮修复的兼容性问题（2.2）

| 问题 | 修复 |
|---|---|
| `IEventRequest` 新增 `Serialize + DeserializeOwned` supertrait（迫使实现者实现 serde） | supertrait 移除；serde bound 下放到 `EventMessage::from_event`/`decode` 与 `IMediator::publish` 方法 |
| `JwtAuth` 因持有 `Arc<dyn IJwtKeyResolver>` 失去 `UnwindSafe`/`RefUnwindSafe` | 显式 impl（resolver 自带同步；2.1 auto-trait 表面保持不变） |
| `init_jwt_secret`/`jwt_secret` 被标 `#[deprecated(since = "2.5.0")]` | 移除属性（迁移说明保留在文档注释）；2.4 再随大版本引入 |
| 内部 crate caret 依赖导致混版本解析 | 2.2 起内部依赖精确钉版本 + CI 一致性检查 |

## 5. 上游缺口：rust-ef 2.5 的 `webx` feature 仅支持 rust-webx 1.x

`rust-ef 2.5.1` 的 `Cargo.toml`：

```toml
webx = ["dep:rust-webx-core"]
[dependencies.rust-webx-core]
version = "1.2"
```

在本 2.x 工作区启用该 feature 会解析出**第二份** `rust-webx-core`（1.x），
其 `impl From<EFError> for Error` 作用于 1.x 的 `Error` 类型，对 2.x 的
`webx::Error` 不生效（类型不匹配）——因此参考应用无法用裸 `?` 传播 `EFError`。

现状：

- 参考应用保留 `db.rs` 中的 `EfResultExt::map_ef()` 适配器，并在文件头注明原因；
- CI 守卫只禁止手写 JWT/bcrypt，**不**禁止该适配器；
- 待 rust-ef 发布依赖 `rust-webx-core 2.x` 的版本后，删除适配器并把 `.map_ef()` 改为 `?`
  （`2.x` 工作区的 `Cargo.toml` 注释已标注）。

## 6. 2.x 的弃用决策（不发弃用）

2.x 线不新增 `#[deprecated]`：下游常用 `-D warnings`，minor 中新增弃用会让其构建
失败。2.x 的做法是**文档标注 + 迁移指南 + 兼容层**；2.2 开发期误标的
`init_jwt_secret`/`jwt_secret` 已按此原则撤回（§4）。3.0 起按 §7 的窗口政策执行，
登记表见 [`deprecations.md`](deprecations.md)。

## 7. 弃用政策（deprecation，3.x 起生效）

2.x 线**不引入 `#[deprecated]`**：下游常用 `-D warnings` 构建，框架在 minor 中新增
弃用会让他们的构建失败；2.x 的等价做法是"文档标注 + 迁移指南 + 兼容层"。

3.x 起的规则：

1. **窗口 ≥ 2 个 minor**：在 minor `N` 标记弃用 → 最早在 `N+2` 移除；同一项不得
   在窗口内反复标记/撤销。
2. **可编译的迁移路径**：每个弃用必须在 `CHANGELOG`、对应迁移指南、rustdoc note
   三处同时给出替代 API 与示例；弃用说明只指向**已存在**的替代项。
3. **运行时行为不变**：弃用只影响编译期告警，不改变语义；行为变更必须走新的
   显式配置开关（默认旧行为），并在下一 major 改默认值。
4. **弃用清单集中维护**：`docs/design/deprecations.md` 记录项、引入版本、计划移除
   版本、替代项与验收测试；CI 检查每个 `#[deprecated]` 都有对应清单条目（3.x 起）。
5. **锁步 crate**：`rust-webx*` 同版本一起弃用、一起移除，不允许 umbrella 先移除。

## 8. 语义稳定面与 LTS

### 8.1 稳定面（Stable surfaces）

以下内容在 2.x 内**只加不改**；任何行为变更必须在 `CHANGELOG` 标注 `behavior change`
并提供 opt-in 开关：

| 面 | 具体内容 |
|---|---|
| HTTP 错误 | `ProblemDetails` 字段与语义、Error 2.0 的 `code`/`status` 映射、`WWW-Authenticate` 行为 |
| 绑定/路由 | 请求绑定失败 400/415 语义、`#[frompath]`/`#[fromquery]`/`#[fromclaim]` 规则、路由优先级 |
| 中间件 | `IMiddleware` 语义（`invoke`/`after`/`ControlFlow`；2.4 新增 continuation 为可选路径） |
| 实时 | WS 帧协议（消息即请求的错误帧 `{error:{code,message,retryable}}`）、关闭码 1000/1001/1008、`WebSocketLimits` 默认值 |
| 观测 | 指标名与标签（`http_requests_total`、`realtime_*`、`media_*`）、`/metrics` 文本格式 |
| 配置 | `AppOptions` 键名、`APP__Section__Key` 覆盖规则、`Jwt.SecretFile` 优先级 |
| 存储/媒体端口 | `IObjectStore`/`IMediaPlaneAdapter` 方法语义（新增方法必须带默认实现）；`RealtimeFanoutEvent` 的事件名（跨节点实时扇出的 wire 契约） |

### 8.2 LTS（长期支持）

- **支持窗口**：当前 minor 全程支持；N-1、N-2 只接收**安全与关键缺陷**修复（不新增
  功能）；更早版本按需提供补丁，不承诺。
- **LTS 分支**：每个 stable major 的最后一个 minor 建立 `lts/<major>.<minor>` 分支
  （如 `lts/2.4`），维护 12 个月；分支只接受 backport 的安全修复、文档修正和
  同分支内的兼容性修复，随 `x.y.Z` 补丁发布。
- **补丁版本承诺**：`x.y.Z` 只修 bug/安全问题，不删 API、不改默认值、不改语义；
  若修复必须改变行为，先加配置开关（默认旧行为）并在补丁说明中公告。
- **升级支持**：跨 minor 升级零代码改动（N-2 冒烟门禁）；跨 major 提供迁移指南与
  弃用窗口（≥2 minor）。
- **发布节奏**：minor 按功能线发布；安全修复不等待 minor，直接从 LTS 分支出补丁。

### 8.3 LTS 分支与发布检查单

```text
□ CI 全绿（tests/clippy/fmt/MSRV/audit/docs/coverage/benches/alloc/semver/upgrade）
□ CHANGELOG 迁移说明（含弃用/行为变更）
□ semver-checks 对上一发布通过（例外仅 compatibility.md 第 3 节）
□ N-2 冒烟通过（compat/upgrade-smoke）
□ 安全审计：cargo audit + 依赖版本钉一致性
□ LTS 分支 backport 只含安全/关键缺陷修复
```

## 9. 例外维护流程

新增 `cargo-semver-checks` 例外（`workspace.metadata.cargo-semver-checks.lints`）必须：

1. 在本文档记录：例外名、原因、影响的包/类型、收口版本；
2. 用 `api_stability_test` 或专项测试锁定"文档化构造路径"；
3. 指定负责人与移除检查项（发布检查单里引用）。

当前唯一例外：`constructible_struct_adds_field = "allow"`（配置 section，收口 3.0）。

## 10. 局部运行

```text
# 安装预编译工具（Windows 示例；Linux/macOS 取对应 asset）
# https://github.com/obi1kenobi/cargo-semver-checks/releases
cargo semver-checks --package rust-webx-host --baseline-rev v2.1.0

# N-2 冒烟
cargo build --manifest-path compat/upgrade-smoke/stable/Cargo.toml
cargo build --manifest-path compat/upgrade-smoke/current/Cargo.toml
```
