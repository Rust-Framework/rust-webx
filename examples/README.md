# 参考应用与参考适配器（examples/）

三个可运行的参考应用，用来验收框架能力，同时充当服务端规范的可执行标杆；另有一个**非发布**的
参考适配器 workspace，展示怎么把框架端口落到具体 ORM/数据库。
**每个案例与 `adapters/` 都是独立 cargo workspace**（各自的 `Cargo.toml` + `Cargo.lock`）：
根 workspace 不包含它们，因此构建/测试要在案例目录内执行，或从仓库根带 `--manifest-path`。

## 三个案例

| 案例 | 定位 | 分层 | 开发地址 |
|------|------|------|----------|
| [`docbit`](docbit/) | 作品集全栈站：SPA + 博客 + RBAC + 六项目文档浏览（rust-ef / SQLite） | 四层 | <http://localhost:5000> |
| [`dmbit`](dmbit/) | 数据中心机房的设备与库存管理：设备 / 产品 / 规格 / 库存（rust-ef / SQLite） | 四层 | <http://localhost:5100> |
| [`imbit`](imbit/) | 实时 IM：HTTP + WebSocket 同宿主，房间消息与断线补拉、信令、TURN、录制 | 四层 | <http://localhost:8090> |

## 四层目录树

三个案例是**同一套四层结构**：`contracts` / `domain` / `handlers` / `host`，且每个操作
落到 `<context>/<operation>.rs` 一个文件。

```
examples/
├── docbit/                     # 独立 workspace（四层）
│   ├── Cargo.toml              #   [workspace] members = contracts|domain|handlers|host
│   ├── Cargo.lock              #   独立 lockfile，与根 workspace 无关
│   ├── contracts/src/          #   DTO / 路由宏 / 对外 port，按 <context>/ 分目录
│   │   ├── docs/               #     models.rs + service.rs + <operation>.rs
│   │   └── auth|blog|category|comment|exhibition|media|rbac|site|tracking|user/
│   ├── domain/src/             #   entities/（实体 + XxxConfiguration）、mappers/、seed
│   ├── handlers/src/           #   每个操作一个 Handler；mappers 在 domain 的同名分片
│   ├── host/                   #   组合根：startup/{services,hosted,seed}、迁移、探活
│   ├── wwwroot/ + appsettings*.json
│   └── publish.sh / publish.ps1 / Dockerfile / docker-compose.yml
├── dmbit/                      # 同上四层（contracts/domain/handlers/host + wwwroot）
├── imbit/                      # 同上四层；host 包名 imbit-host（bin 名仍是 imbit）
│   └── host/appsettings.json   #   配置锚点：避免向上命中兄弟示例的 appsettings
└── adapters/                   # 参考适配器（独立 workspace，全部 publish = false）
    └── ai|auth|idempotency|outbox|payment/ef/
                                  #   把框架端口落到 rust-ef 的参考实现，复制使用
```

依赖方向：`contracts` → 无（只依赖框架）、`domain` → 可依赖 `contracts` 的枚举、
`handlers` → `contracts` + `domain`、`host` → 全部（唯一的可执行入口在 `host`）。

## 构建与测试

每个案例独立 workspace。在案例目录内执行：

```bash
# docbit
cd examples/docbit
cargo build                                  # 构建全部四层
cargo test --workspace --no-fail-fast
cargo run -p docbit-host                     # → http://localhost:5000（-- --doctor 打印路由诊断）

# dmbit
cd examples/dmbit
cargo test --workspace --no-fail-fast
cargo run -p dmbit-host                      # → http://localhost:5100

# imbit（bin 名 imbit，包名 imbit-host）
cd examples/imbit
cargo test --workspace --no-fail-fast
cargo test -p imbit-host --test e2e_im_test  # 真实 HTTP + WS 端到端
cargo run -p imbit-host                      # → http://localhost:8090
```

等价的根目录写法（CI 用的就是这一条）：

```bash
cargo test --workspace --no-fail-fast --manifest-path examples/docbit/Cargo.toml
cargo test --workspace --no-fail-fast --manifest-path examples/dmbit/Cargo.toml
cargo test --workspace --no-fail-fast --manifest-path examples/imbit/Cargo.toml
cargo test --workspace --no-fail-fast --manifest-path examples/adapters/Cargo.toml  # 参考适配器
cargo build --release --manifest-path examples/docbit/Cargo.toml -p docbit-host
```

CI 的 `Examples` job 用 `examples/${{ matrix.app }}/Cargo.toml` 逐个跑
`cargo test --workspace --no-fail-fast`；矩阵当前是 `[docbit, dmbit]`，`imbit` 四层已就位、
但还没加进 `.github/workflows/ci.yml` 的 `matrix.app`，本地请按上面的命令自行验证。

## 参考适配器（`adapters/`，非发布）

`adapters/` 是独立 workspace（`examples/adapters/Cargo.toml`，自带 lockfile），
五个与 ORM/数据库耦合的持久化适配器都在这里，全部 `publish = false`：

| 适配器 | 实现的端口 | 说明 |
|---|---|---|
| `ai/ef` | `IUsageSink` | 用量账本（`ai_usage_events`）+ outbox/rollup workers |
| `auth/ef` | `ISessionRepository` / `IRefreshTokenStore` | 会话表与 refresh token 表 |
| `idempotency/ef` | `IIdempotencyStore` | `webx_idempotency` 表、租约围栏、清理服务 |
| `outbox/ef` | `IOutbox`（EF 存储） | 同事务 `enqueue_outbox` + `add_outbox_ef` |
| `payment/ef` | 支付回调 inbox | 唯一去重、重试、死信 |

**它们是参考实现，不是依赖项**：

- crates.io 上没有制品（`publish = false`），业务 `Cargo.toml` 里写这些包名会解析失败；
- 需要持久化时**复制**到自己的仓库并改名：
  `cp -r examples/adapters/outbox/ef crates/my-service-outbox`，随后包名、实体、表、迁移都归你；
- 也可以只实现端口、不复制任何代码，但必须跑 `testing/contract-tests` 的端口契约
  （对象存储 / 媒体面 / 事件传输），参考适配器自带测试可作模板；
- 参考实现的真库测试是 env-gated（`RUST_EF_PG_URL` / `RUST_EF_MYSQL_URL`），默认 SQLite 即可跑通。

```bash
cargo test --workspace --manifest-path examples/adapters/Cargo.toml
```

接入方式（用默认实现 / 复制参考适配器 / 自己实现 + 契约测试）与端口清单见
[端口与适配器（下游接入）](../docs/rust-webx/13-extensibility/integrations.md)。
案例本体仍然只需要 **`rust-webx` + `rust-ef`**：持久层错误由 rust-ef 的 `From<EFError>` 交给 handler
的 `?`，不需要任何映射层；`rust-webx-ef`（`webx_ef`）兼容壳已在 2.9.1 删除。

## 规范标杆

这三个案例是 rust-xazgo 服务端规范的可执行标杆：

- **一操作一文件**：一个操作 = 一个 Request（`contracts/<context>/<operation>.rs`）+ 一个
  Handler（`handlers/<context>/<operation>.rs`）+ 一个文件；
- **映射目录化**：DTO ↔ 实体映射在该 crate 的 `src/mappers/<context>_mapper.rs`，不堆单文件；
- **映射三件套**：创建 `TryFrom<CreateXRequest> for Entity`（校验必填/不变量，`id`、时间戳与审计字段由服务端赋值）、
  读模型 `From<&Entity> for XModel`、部分更新 `IPatchable<UpdateXRequest> for Entity`（只覆盖 `Some` 字段）；
  更新写路径统一 `ctx.edit(row, |r| r.patch(req))` + `detect_changes` + `save_changes`，不再有
  `stage_update` / `track_row` 脚手架。**`std::convert` 是指导，不是框架契约；框架只拥有 `IPatchable`**（`webx::IPatchable`）；
- **身份只来自身份字段**：`#[fromclaim] user: IdentityUser`（+ 自动 `RequestUser`），租户/主体绝不取 body 或 query；
- **框架原生**：分页 `PagedRequest` / `PagedResult`、错误 `webx::Error`（持久层直接 `?`）、
  注册 `#[handler]` 编译期收集、DDL 与初始化走官方迁移、组合根 `configure(|app| app.options(..))`。

新增或修改案例时不要引入框架之外的第二套实现。

## 延伸阅读

- docbit：[README](docbit/README.md) 与 [生产部署](docbit/PRODUCTION.md)
- 案例研究：[第十五章 案例研究：Docbit](../docs/rust-webx/15-case-study/INDEX.md)
- IM 与媒体控制面：[im-reference](../docs/design/im-reference.md)、[media-contract](../docs/design/media-contract.md)
