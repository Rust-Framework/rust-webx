# 兼容性政策（SemVer、N-2 升级、锁步版本）

> 基线：rust-webx 2.9.1。门禁在 `.github/workflows/ci.yml` 的 `semver`、`upgrade`、
> `lint`（内部版本钉）三个 job。

## 1. 三条承诺

1. **公开 API 只加性演进**：2.x minor 不删除/改名公开项，不收紧泛型 bound，不移除
   auto trait，不给既有函数新增 `#[deprecated]`（会让下游 `-D warnings` 构建失败）。
   **2.9.1 是破坏性补丁（semver 例外）**：维护者决定把原计划的 3.0 契约迁移整体并入这个
   补丁发布，此后不再有独立的 3.0，恢复加性演进。同一次补丁里的例外面：
   - `Error` 变为 `rust_core::AppError` 的别名（2.9 的 enum 删除，模式匹配改访问器）；
   - 分页 / keyset / operation 契约改由共享 `rust-core` 提供（`webx_core::pagination` /
     `::keyset` / `::operation` 的历史模块路径保留为 re-export）；
   - `rust-webx-ef`（`webx_ef`）兼容壳删除（`map_ef_error` / `EfResultExt` 早已移除；持久化幂等
     改由端口实现提供，参考适配器在 `examples/adapters/idempotency/ef`，不随框架发布）；
   - `HostAppBuilder::use_options` → `options`，驼峰别名 `useOptions` 删除；
   - 2.x 兼容层整体删除（无别名）：`DeclaredRules` / `IDynamicAuthorizer::authorize_with_rules`
     → `authorize_with_declared_rules`；`ResourceAuthorization` / `IAuthorizationPolicy` /
     `authz::resource_auth_middleware` → `#[authorize]`（静态/动态）+ `IDynamicAuthorizer` +
     Handler 内 `require_owner` / `require_same_tenant`；`init_jwt_secret` / `jwt_secret()` →
     `ITokenIssuer` + `Jwt.SecretFile` / `register_with_options`；`RateLimit.TrustProxy` /
     `RateLimit.TrustedProxies` 与 `RateLimiter::with_trust_proxy` → `ClientInfo` 转发头信任键；
   - `SelectQueryBuilder` 的字符串排序 `order_by(&str)` / `order_by_desc(&str)` → `order_by_raw` /
     `order_by_desc_raw`（强类型 `order_by_field*` 仍是主入口）；
   - 健康检查与中间件注册 spec 化（`HealthCheckSpec` / `MiddlewareSpec` 成为唯一入口）；
   - `HostBuilder::add_authentication` → `use_authentication`；
   - 通知端口 `use_email_sender` / `use_sms_sender` → `use_email` / `use_sms`。

   逐条源码改动见
   [`upgrade-to-2.9.1.md`](../rust-webx/16-migration/upgrade-to-2.9.1.md)；门禁例外登记见 §3。
2. **内部 crate 锁步发布**：`rust-webx*` 之间使用**精确版本** `=x.y.z` 依赖，
   消费者无法解析出 umbrella 与子 crate 混版本组合。与 ORM/数据库耦合的持久化适配器不在发布序列里
   （`examples/adapters/*` 全部 `publish = false`）：它们是参考实现，可复制不可依赖
   （见 [端口与适配器归属](integrations.md)）。
3. **N-2 源码兼容**：同一份只使用 N-2 稳定 API 的应用源码，必须能同时编译
   针对 N-2 发布 `rust-webx = "=2.9.0"`（2.9.2 的 N-2）与当前工作区路径。

## 2. 门禁

### semver（对上一发布）

```text
cargo semver-checks --package rust-webx-core   --baseline-rev v2.9.1
cargo semver-checks --package rust-webx-engine --baseline-rev v2.9.1
cargo semver-checks --package rust-webx-host   --baseline-rev v2.9.1
cargo semver-checks --package rust-webx        --baseline-rev v2.9.1
```

例外：`constructible_struct_adds_field = "allow"`（见 §3），以及 2.8.0 幂等值类型的
`struct_marked_non_exhaustive` / `enum_marked_non_exhaustive = "allow"`（同见 §3），
2.9.1 限流枚举/通知端口改名的两条 `allow` 已在 2.9.2 删除：semver baseline 前移到 v2.9.1，
这些 2.9.1 例外不再参与门禁，只在 §3 留档。

### upgrade（N-2 源码兼容）

`testing/upgrade-smoke/src/main.rs` 只用 N-2 稳定 API，分别在两个独立 workspace 编译。
N-2 基线随当前版本滚动：2.9.2 的 N-2 是 **rust-webx 2.9.0**（下一个 minor 发布时同步前移）。

```text
testing/upgrade-smoke/stable/Cargo.toml   → rust-webx = "=2.9.0"（固定 2.9.0 全套）
testing/upgrade-smoke/current/Cargo.toml  → rust-webx = { path = "../../../crates/webx" }
```

### 内部版本钉一致性

工作区所有 `path = "crates/..."` 内部依赖的 `version` 必须等于
`[workspace.package].version`；否则 CI 失败。

### 依赖重复（duplicate versions）

`cargo tree --workspace -d` 列出同一 crate 同时解析出多个版本的组。当前集合钉在
[`scripts/dependency-duplicates.baseline.txt`](../../scripts/dependency-duplicates.baseline.txt)，
每行 `crate@版本列表`，**只按 crate 名比较**：

- 出现基线外的新组 → `python3 scripts/check-dependency-dedup.py` 退出 1，`Dependencies
  (duplicate versions)` job 失败并打印组名；
- 某组收敛成单版本 → 通过并提示可收紧基线（`--update` 重写，禁止手工编辑）；
- 组内版本变化（例行升级）→ 通过，只打印 `~ crate: 旧版本 -> 新版本`。

脚本读 `cargo metadata --locked`：包集合与 `cargo tree -d` 相同，但包含所有目标平台的依赖，
不受被检查机器影响，所以同一份基线在 ubuntu runner 与开发者工作站上都成立（`cargo tree -d`
的行数更多，因为它把同一版本按依赖种类重复打印）。基线里保留一个分组等于承认这处分叉暂时
无法收敛：新增分组必须写清原因与收口计划，并在依赖收敛后同步收紧基线。

### 供应链与全量测试覆盖

| job | 覆盖 | 外部服务 |
|---|---|---|
| `Security Audit` | `cargo audit` 分别扫描根 workspace 与 `examples/adapters/Cargo.lock` | advisory-db（联网） |
| `Dependencies (duplicate versions)` | `cargo tree -d` + 基线门禁（只解析，不编译） | 无 |
| `Test (<os>)` | 根 `cargo test --workspace --locked`（默认特性）+ `examples/adapters`（`--locked`） | 无 |
| `Features (workspace --all-features)` | `cargo test --workspace --all-features --locked --no-fail-fast`（全特性整树测试） | 无 |
| `Real databases (idempotency store)` | adapters 的 `#[ignore]` 真库测试（PostgreSQL 16 / MySQL 8） | GHA service 容器 |
| `Broker adapters (redis/nats/rabbitmq/kafka)` | `scripts/test-brokers.sh`：四个适配器集成套件 | `docker-compose.brokers.yml` |
| `Examples (<app>)` | `examples/docbit` / `dmbit` / `imbit` 三个独立 workspace 的 `cargo test --workspace` | 无 |

适配器集成测试在 `WEBX_*_URL` / `RUST_EF_*_URL` 未设置时会静默跳过，默认特性下的
`cargo test --workspace` 因此无法证明它们：**"全量 test（含 broker）"的承载点是 `realdb` 与
`brokers` 两个 job**。nightly 的 `stability.yml` 保留同一套 broker 检查用于长跑回归；
打发布 tag 前这两个 job 必须为绿。特性策略：`Test (<os>)` 在三个 OS 上跑默认特性，
`Features (workspace --all-features)` 在 linux 上把整棵工作区的每个 target 用全特性跑一遍，
两处互补而不是互相替代。

## 3. 已知例外：幂等值类型的新增字段与 `#[non_exhaustive]`（配置 section / `IdempotentResponse`）、2.9.1 通知端口改名、`EmailMessage` 新字段、限流枚举新变体

`constructible_struct_adds_field`、`struct_marked_non_exhaustive` 与
`enum_marked_non_exhaustive` 设为 `allow`，覆盖以下已文档化的 2.8.0 变更：

1. **配置 section**（`ServerSection`、`JwtSection`、`IdempotencySection` …）是 serde DTO：
   文档化构造路径是 `Default` + `appsettings.json` / `APP__*`，不是结构体字面量。
   因此新增配置开关对文档化用法是加性的。下一破坏性版本一次性为配置 section 加
   `#[non_exhaustive]`（配迁移指南），随后删除该部分例外。
2. **2.8.0 的 `IdempotentResponse`** 新增 `headers`（缓存响应的可重放响应头），只破坏
   结构体字面量构造；同时与 `IdempotencyLease`、`IdempotencyBegin` 一起标记
   `#[non_exhaustive]`，以便 2.x 内继续加入状态/元数据而不再产生破坏性版本。
   CHANGELOG `[2.8.0]` 的 Breaking 段给出迁移写法：`IdempotentResponse` 用
   `::new(status, content_type, content_encoding, body)` + `.with_headers(..)` 构造，
   `IdempotencyLease` 用 `::new(token)`，`match IdempotencyBegin` 补 `_` 通配分支。
   `IIdempotencyStore` 仍保持对象安全，新增的租约方法都有默认实现。
   该部分与配置 section 同窗口在下一破坏性版本复核收口。
3. **2.9.1 通知端口改名**：`HostBuilder::use_email_sender` / `use_sms_sender` 删除，改为
   `use_email` / `use_sms`（参数类型本来就是 `Arc<dyn IEmailSender>` / `Arc<dyn ISmsSender>`，
   名字与 `use_clock` / `use_meter` / `use_session_store` 同形）。维护者决定**不给兼容别名、
   不走弃用窗口**，因此一个补丁版本里出现了公开方法移除——这与 §1 的加性承诺相悖，作为已登记
   例外处理：

   - 影响面：只有通知端口的两个 builder 方法；DI 注册方式、「默认不注册发送器」契约、
     `InMemoryEmailSender` / `InMemorySmsSender` 全部不变；
   - 迁移：调用点改一个标识符，见 CHANGELOG `[2.9.1]` → Breaking 与 `deprecations.md`；
   - 门禁：`inherent_method_missing` 的 `allow` **只写在 `crates/host/Cargo.toml` 的
     `[package.metadata.cargo-semver-checks.lints]`**，`rust-webx-core` / `rust-webx-engine` /
     `rust-webx` 仍保持完整的方法移除检测；
   - 锁定：`crates/webx/tests/notifications_test.rs` 覆盖 `use_email` / `use_sms` 的注册与捕获；
   - 收口：`semver` job 的 baseline 前移到 v2.9.1 之后，该条目与 crate 级 `allow` 一并删除。

4. **2.9.1 `EmailMessage` 新增 `from` / `reply_to` / `headers`**（上游反馈 §7）：邮件发送器需要
   按租户/品牌选发件人、设置 `Reply-To`、附加 `List-Unsubscribe` 等头。只破坏**结构体字面量**
   构造；文档化构造是 `EmailMessage::new(..)` + `with_from(..)` / `with_reply_to(..)` /
   `with_header(..)`。`validate()` 同时校验地址与头（拒绝 CRLF/控制字符注入）。
   CHANGELOG `[2.9.1]` → Breaking 给迁移写法，`constructible_struct_adds_field` 的全局允许在
   `Cargo.toml` 注释里登记为第三种用例。

5. **2.9.1 `RateLimitRoute` 新增 `Pattern`、`RateLimitSubject` 新增 `Composite`**（上游反馈 §5）：
   - 这两个仍是公开 enum，新增变体意味着下游**穷举 `match`** 需要补通配分支；这是 2.x 加性
     承诺的登记例外，`crates/core/Cargo.toml` 里以 `enum_variant_added = "allow"` 限定在
     `rust-webx-core`，baseline 前移后删除；
   - `Error` 已不是 enum：2.9.1 的 `Error::too_many_requests(secs, msg)` /
     `Error::conflict_retryable(msg)` 是 `rust_core::AppError` 的**构造器**（不是新变体），
     不产生本例外；`Error` 的稳定面是构造器 / 访问器 / code 注册表，见
     [`app-error-migration.md`](app-error-migration.md)；
   - 行为不变的部分：`RateLimitRoute::Exact/Prefix/Any` 与 `RateLimitSubject::{Ip,User,Tenant,ApiKey}`
     的匹配/键派生不变，新增分支只是让模板与组合主体可用；`Pattern` 只实现路由语义的文档化子集；
   - 迁移：限流规则按需改 `RateLimitRoute::pattern(..)` / `RateLimitSubject::Composite(..)`，旧的
     exact/prefix 规则不需要改；429 的等待值改用 `Error::too_many_requests(secs, msg)`，"等待
     冲突方完成可重试"的 409 改用 `Error::conflict_retryable(msg)`；
   - 锁定：`crates/core/src/rate_limit.rs` 单测 + `docs/design/error-codes.md`
     的码表一致性测试。

6. **2.9.2 加性补丁的字段（结构体字面量构造受影响）**：`WebSocketLimits` 新增 `session_revalidation`（用 `Default` + `.with_session_revalidation(d)` 构造）；`RedisEventsOptions` / `NatsOptions` / `RabbitMqOptions` 新增广播通道字段（serde 默认值，配置绑定属加性；字面量构造请用 `Default` + `with_*`）。本版不删除 API、不改默认行为，不需要迁移；`constructible_struct_adds_field` 的全局允许已在根 `Cargo.toml` 登记。

需要保持字面量构造的结构体（如 `ProblemDetails`）由
`crates/webx/tests/api_stability_test.rs` 锁定字段集合。

## 4. 边界约定：rust-ef 错误映射

`rust-ef` 是 Web 无关的 ORM：2.6 起它不再提供 `webx` feature，也不依赖 `rust-webx`。
2.9.1 起持久层错误到应用错误的翻译**归 rust-ef 自己的边界**：`rust-ef` 2.9.1 实现
`impl From<EFError> for rust_core::AppError`，handler 直接 `?`：

```rust
// Handler：不再需要适配层函数，`?` 直接产出 webx::Error（= rust_core::AppError）
let blogs = ctx.set::<Blog>().query().to_list().await?;
```

`rust-webx-ef`（lib 名 `webx_ef`）已在 2.9.1 **删除**（不再有兼容重导出），因此错误映射侧不需要
任何 crate：

- `map_ef_error`、`EfResultExt`（`.map_ef()`）与固定文案的 `DETAIL_*` 常量随该 crate 一起消失；
  历史 `webx_ef::*` 与 `webx_ef::idempotency::*` 路径**没有替代别名**；
- 新代码的依赖目标：

| 需求 | 依赖 |
|---|---|
| 持久层错误 → `AppError` | 不需要额外 crate；用 `rust-ef` 2.9.1 的 `From<EFError> for AppError` |
| 持久化 `Idempotency-Key` store | 框架端口 `IIdempotencyStore`；持久化实现由下游提供，参考适配器 `examples/adapters/idempotency/ef`（非发布，可复制） |

配对发布：先 rust-ef 2.9.1、后 rust-webx 2.9.1；workspace 依赖写 `rust-ef = "2.6"`
（semver 范围，已含 2.9.x，本版按 2.9.1 验证），不收紧为精确 pin——否则在 ORM 发布前
`Cargo.lock` 不可解析。

- 不在 `rust-webx-core` / umbrella 增加 `ef` feature 或可选 `rust-ef` 依赖。即使
  默认关闭，也会把 ORM 概念带进 core 的 feature 图与发布清单，破坏“小核心”边界。
- 不启用 `rust-ef` 的 `features = ["webx"]`，不恢复历史上的 `webx-core-shim` 兼容层或
  `[patch.crates-io]`。
- 应用侧不写 `impl From<EFError> for webx::Error`（孤儿规则），也不重复实现映射：边界转换
  由 rust-ef 提供，应用只写 `?`。

`From<EFError> for AppError` 的分类契约（推导权威见
[`app-error-migration.md`](app-error-migration.md) §5，唯一分类源是
`rust_ef::database_error_is_transient`；本版按 rust-ef 2.9.1 配对验证）：

| `EFError` | HTTP | `AppError`（构造器） | 客户端可见 detail |
|---|---|---|---|
| `NotFound` | 404 | `not_found` | 业务文案（原样） |
| `ModelValidation` | 400 | `validation` | 业务文案（原样） |
| `ConcurrencyConflict` | 409 | `conflict` | 业务文案（原样） |
| `Query`、`TypeConversion` | 400 | `validation` | 固定文案（不回显 SQL/列名） |
| `UniqueViolation` | 409 | `conflict` | 固定文案（不回显 constraint/table） |
| `Connection`、`Transaction` | 503 | `unavailable` | 固定文案、可重试 |
| `Database` 的瞬态条件（`SQLSTATE` 08/40/57P0*、SQLite `busy`/`locked`、MySQL 1205/1213） | 503 | `unavailable` | 固定文案、可重试 |
| `Database`、`Migration`、`Configuration`、`Provider`、`ChangeTracking`、`Other` | 500 | `internal(operator)` | 无（`public_detail() = None`，原文与 source 链只进日志） |

瞬态判定由 rust-ef 读 `DatabaseErrorInfo` 的 `sqlstate`/`code` 完成：provider 只把唯一约束提升为
`UniqueViolation`，其余驱动错误都以 `Database` 上报，应用侧不再需要 `code == 5/6` 或
`"database is locked"` 之类的前置判断。

运行期 DI 解析失败不属于持久层：`impl From<rust_dix::RdiError> for AppError` 把失败转成
`di(operator)`（500、`public_detail()` 为 `None`、容器消息只进运维日志），端口实现里
`scope.get_owned()?` 直接可用，应用不再需要本地 `map_di_error`。

5xx 的原文与 `source` 链进 `internal(operator)`，`public_detail()` 为 `None`，由
host 写入日志。新增 rust-ef 变体走同一隐藏 500 分支，升级依赖不会扩大客户端可见
信息。参考实现见 `examples/docbit` / `examples/dmbit` 的 handlers（持久层调用直接 `?`，
不再有 db 映射 helper 文件）。

若未来确需框架级的 ORM 桥接，也只能作为**下游数据适配**（参考实现放在 `examples/adapters/`，
`publish = false`），不能进入 core/umbrella——本仓始终零 ef 依赖，也不允许 ORM → Web 框架。

### 配对验证（pairing verified）的证据边界

发布顺序是 **rust-ef 2.9.1 → rust-webx 2.9.1**（见上）。发布前用本地 rust-ef
checkout 做编译级配对验证：

```bash
# 在 rust-webx 仓库根执行；RUST_EF_PATH 指向本地 rust-ef 仓库根。
RUST_EF_PATH=/path/to/rust-ef bash scripts/check-pairing-rust-ef.sh
```

脚本在临时目录生成 `[patch.crates-io]`（只把 `rust-ef` / `rust-ef-sqlite` 指向
本地源码树，其余依赖仍走 crates.io），先对齐根 workspace 的 `Cargo.lock` 再执行
`cargo check --workspace --locked`；EF 参考适配器自 2.9.1 起位于独立 workspace
`examples/adapters/`（`publish = false`，不是发布物），因此改用
`cargo test --manifest-path examples/adapters/Cargo.toml -p rust-webx-idempotency-ef` 验证；两个 lockfile
退出时都恢复原状，不污染工作树。CI 的可选 `pairing` job 只在配置了
`vars.RUST_EF_PATH`（自托管 runner 上已有 checkout）或
`vars.RUST_EF_REPO`（+ `RUST_EF_REF` / `rust_ef_ref` dispatch 输入，由 job 自行
checkout）时运行；未配置时打印 notice 并跳过，不阻塞普通 PR。

**"pairing verified" 只证明**：给定的 rust-ef 源码树能让 rust-webx 根工作区与 `examples/adapters`
工作区编译通过，并让参考适配器的测试通过。**不证明**：crates.io 上已发布
制品可下载/可解析、该 rust-ef 树自身的 semver 门禁、也不覆盖真 PostgreSQL/MySQL
（由 CI 真库 job 覆盖）。发布记录里写证据时应同时记录 rust-ef 的 commit/tag 与
rust-webx 的 commit，例如 "pairing verified: rust-ef@<sha> × rust-webx@<sha>"。

在 rust-ef 2.9.1 上线 crates.io 之前，`Cargo.lock` 允许继续解析到已发布的旧版本（保持可构建）；
rust-ef 发布后、给 rust-webx 打 tag 前，执行 `cargo update -p rust-ef --precise 2.9.1`
（`rust-ef-sqlite` / providers 同步），确认 `cargo tree -i rust-ef` 只有 2.9.1，再提交锁文件。

## 5. 弃用政策

- **2.x 不新增 `#[deprecated]`**：等价做法是"文档标注 + 迁移指南 + 兼容层"。2.x 里唯一用过这条
  路的是 2.5 的 `init_jwt_secret` / `jwt_secret()` shim，它已在 2.9.1 随其余兼容层一并删除
  （登记见 [`deprecations.md`](deprecations.md)），因此当前没有存活的文档标注兼容层。
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
- 补丁版本只修 bug/安全问题，不删 API、不改默认值、不改语义（**2.9.1 是已登记的
  破坏性补丁**，见 §1；此后恢复该规则）。
- 跨 minor 升级零代码改动（N-2 冒烟门禁）；跨 major 提供迁移指南与弃用窗口。

```text
□ CI 全绿（tests/clippy/fmt/MSRV/audit/dedup/features/docs/coverage/benches/alloc/semver/realdb/brokers/examples/upgrade）
□ CHANGELOG 含迁移说明（弃用/行为变更）
□ 配对 ORM 已先行发布（rust-ef x.y.z → rust-webx x.y.z；`rust-ef = "2.6"` 范围覆盖 2.9.x，不收紧为精确 pin）
□ 配对验证已跑：`RUST_EF_PATH=… bash scripts/check-pairing-rust-ef.sh`（或 CI `pairing` job）通过，证据边界见 §4
□ semver-checks 对上一发布通过（例外仅 §3）
□ N-2 冒烟通过（testing/upgrade-smoke；2.9.2 的 N-2 = rust-webx 2.9.0）
□ cargo audit 0 漏洞（根 + `examples/adapters/Cargo.lock` 两个 lockfile）+ 依赖重复基线通过 + 内部版本钉一致
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
