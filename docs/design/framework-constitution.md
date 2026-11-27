# rust-webx 框架设计准则

> 面向维护者、核心贡献者与架构评审。本文回答**一个决策凭什么合格**，不记录版本沿革、
> 历史缺陷或阶段计划；当前 API 以开发者手册与 rustdoc 为准，发布内容以 `CHANGELOG.md` 为准。
> 基线：rust-webx 2.9.4。

---

## 1. 价值方程与不可约约束

```
框架价值 ≈ (被吸收的决策数 × 可逆性) / (概念数 × 耦合度) − 故障爆炸半径 × 不可观测系数
```

| 推论 | 判定方式 | 反例 |
|---|---|---|
| 一个概念服务 ≥3 类场景才配进核心 | 新增概念必须删旧或证明净收益 | 为单一场景新增 `XxxResult` |
| 可逆性即逃生舱 | 每个宏有手写等价物；每个端口 ≥2 实现 | 只有宏能表达的路由 |
| 概念税要收得明白 | 核心概念清单 ≤10 | 同时存在 3 种响应信封 |
| 爆炸半径可观测 | 每个故障有码、有指标、有 trace | 500 只有一句 `Internal error` |

五个不可约约束：

1. **无 GC** → 分配就是延迟预算；所有权就是 API 形状。
2. **编译期一切** → 没有反射/动态代理；宏是扩展语言；编译时间是 UX 指标。
3. **进程即部署单元** → 启动/停机/排空/取消/重载全部归框架管。
4. **故障必然发生** → 框架的产品是**可预测的降级**，不是 happy path。
5. **用户是维护多年的团队** → 语义稳定优先于功能速度。

---

## 2. 十二条原则（P1–P12）

| # | 原则 | 含义 | 检验方式 |
|---|---|---|---|
| P1 | 一个关注点一种写法 | 路由/绑定/结果/错误/鉴权/后台/实时各一条主路径 + 文档化逃生舱 | 同一件事示例 >1 即缺陷 |
| P2 | 无隐式状态 | 无 task-local、无全局可变注册表、无隐藏顺序 | 检索 `thread_local`/`static mut`；依赖出现在构造器或请求字段 |
| P3 | 默认安全、失败关闭 | 生产默认值安全；坏配置拒绝而非回退默认 | 每个默认值配威胁模型 + 负例测试 |
| P4 | 性能是预算 | 热路径每层声明分配/复制/系统调用上限 | 分配计数 + p99 门禁（见 `alloc-budget.md`、`stability-gates.md`） |
| P5 | 可诊断 | 每个失败有码、有因果链、有 request id | "任意 500 能否 5 分钟内解释清楚"演练 |
| P6 | 生命周期完备 | 启动序/停机序/排空/取消/释放都有定义 | 满载 SIGTERM：任务归零、连接排空、无泄漏 |
| P7 | 一切有界 | 队列有界、等待有超时、重试有预算 | 卡死依赖的混沌测试不得 OOM |
| P8 | 语义版本 | 行为也是 API；弃用 ≥2 minor；MSRV 显式 | N-2 升级测试 + `cargo-semver-checks` |
| P9 | 端口而非插件 | 扩展点 = 稳定 trait + ≥2 实现 + 契约测试 | 每个端口有 fake/real/契约测试 |
| P10 | 小核大生态 | core 只含传输/生命周期/DI/端口声明 | `scripts/check-crate-layout.py` |
| P11 | 错误路径也是热路径 | 过载时拒绝/日志/序列化比正常路径更贵 | 错误路径分配与耗时预算 |
| P12 | 控制面 fail-closed | 配置/热更新/发现数据非法状态拒绝，保留 last-known-good | 注入坏快照：旧值保留、告警、不降级 |

---

## 3. 结构裁决（当前）

| 决策 | 当前立场 | 验收 |
|---|---|---|
| 运行模型 | 保留 ASP.NET Core 式生命周期（DI/中间件/HostedService/Options/ProblemDetails），剥离运行期反射 | 显式依赖、无 ambient DI |
| 性能 | 预算化：地板 + 框架税，CI 只对税设限 | `alloc-probe compare 500 13` |
| 内置 vs 生态 | 框架只交付端口 + 默认实现；实体/表/数据适配归下游（参考实现 `examples/adapters/`，非发布）；`core` 不含 ORM/缓存/消息/AI | 依赖方向门禁 |
| 路由注册 | 编译期 inventory + 启动期确定性排序；重复/冲突启动失败 | 路由冲突测试、trybuild |
| 生命周期作用域 | 请求 scope + 连接 scope（实时）；root 不解析 Scoped | 严格作用域测试 |
| 执行引擎 | 每 worker current-thread runtime + 全局准入；阻塞代码必须出 runtime | 准入/排空/过载测试 |
| 错误模型 | 统一 `Error` + RFC 9457 `problem+json`（`code`/`retryable`/`traceId`）；`code` 必须登记进 `ERROR_CODES` | 错误映射/注册表测试 |
| 长连接 | 一个容器两种生命周期：HTTP 请求 + WebSocket 连接；SSE 仍是一等响应；关闭原因统一为 `CloseReason` | WS/SSE 端到端与关闭语义测试 |
| 传输后端 | engine 定义监听级 `IHttpTransport` 与每请求入口 `serve_transport_request`；非 hyper 后端（HTTP/3）是独立可选 crate，默认构建不链接 | [transport-port.md](transport-port.md)、发布顺序/布局门禁 |
| 观测 | 框架自带 RED 指标（按路由标签计数/直方图/inflight/连接 gauge，默认 noop）；`webx.http` DEBUG span 树与 `webx.connection` 事件是兼容契约，默认路径零分配 | `/metrics` 契约测试、`observability_test`、alloc 门禁 |
| 中间件 | 只有一种 `Next` 语义；旧式 `invoke/after` 由默认桥接保持兼容 | `middleware-next.md` |
| 参考实现 | docbit/dmbit/imbit 必须使用框架端口，禁止手写 JWT/口令哈希/错误映射 | CI 反模式检查 |
| 组合根 | 只有两个槽：`register`（DI 服务/端口，对象图唯一来源）与 `configure`（`AppOptions`/横切）；`use_*`/`add_*` 是写入同一 builder 状态的糖 | `composition_slots_test`；新增 builder 方法必须在 rustdoc 标注所属槽 |
| 发布 | 32 个 `rust-webx*` crate 精确钉版本、依赖序发布、可重入 | `scripts/check-publish-order.py` |

---

## 4. 明确不做

1. 不做 ORM、模板引擎、UI 框架、服务网格、集群管理。
2. 不做运行期反射/动态插件加载/ambient DI。
3. 不引入第二套路由/DI/错误/结果/后台/中间件模型。
4. 不让 `core` 出现领域词（IM、媒体、AI、支付）；领域能力进独立 crate。
5. 不为 benchmark 特化热路径。
6. 不做"万能配置"；承诺默认值正确 + 偏离可解释。
7. 不用 "last wins" 处理注册冲突；冲突必须启动失败。
8. 不留进程级开关控制单 Host 行为。
9. 不让参考实现示范反模式。
10. 不让依赖库隐式注入路由；扫描必须可白名单/可关闭。
11. 不用类型短名做分发键；只用 `TypeId` 或完全限定路径。
12. 不把版本沿革、阶段台账、失效路线图写进当前文档。

---

## 5. 门禁

| 门禁 | 位置 | 内容 |
|---|---|---|
| 格式 / Clippy | `ci.yml` lint | `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings` |
| 文档一致性 | `ci.yml` lint | `scripts/check-docs.sh`（链接、INDEX/SUMMARY、版本一致） |
| crate 归属 | `ci.yml` lint | `scripts/check-crate-layout.py`（固定分类法 + 依赖方向） |
| 发布顺序 | `ci.yml` lint | `scripts/check-publish-order.py`（完整性 + 依赖先行） |
| 测试 | `ci.yml` test | `cargo test --workspace --no-fail-fast`（三平台） |
| MSRV | `ci.yml` msrv | `cargo +1.95.0 check --workspace --locked` |
| 安全 | `ci.yml` audit | `cargo audit`（0 漏洞） |
| 兼容 | `ci.yml` semver/upgrade | 公开 API 对 `v2.9.2`；N-2（rust-webx 2.9.1）源码同编译 |
| 分配 | `ci.yml` alloc | `alloc-probe compare 500 13` |
| 稳定性 | `stability.yml` | soak / chaos / wire p99 / broker-integration / conformance（HTTP 互操作矩阵） |
| Fuzz | `fuzz.yml` | 独立 `fuzz/` workspace 的 nightly cargo-fuzz（6 个解析 target）+ 稳定工具链不变量测试 |

任何能力/API 变更必须同批更新：实现 + 测试 + 手册 + 能力矩阵 + 门禁；不允许只改文档或只改代码。

---

## 6. 版本治理

- **2.x 只做加性变更**；破坏性重构留到下一个 major。
- 内部 `rust-webx*` crate 精确钉 `=x.y.z`，禁止混版本解析。
- 弃用 ≥2 minor，登记在 `deprecations.md`；语义稳定面见 `compatibility.md`。
- 发布以 `CHANGELOG.md` 顶部条目为准；版本、文档 INDEX、包版本必须一致（`check-docs.sh`）。

---

## 7. 文档纪律

- 当前文档只写**当前基线**的正确做法；版本沿革、阶段编号、失效计划、评审快照一律不入库。
- 同一事实只有一个权威落点，其余位置用指针，禁止复制段落。
- 面向上手者的内容放 `docs/rust-webx/`；面向维护者的取舍放 `docs/design/`。
- 发布证据由 CI 与 `CHANGELOG.md` 承担，不另建"证据台账"。
