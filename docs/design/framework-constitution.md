# rust-webx 框架宪法：第一性原理、行业证据与对抗审查

> 基线：rust-webx 2.1.0（`dd23b92`）。本文不是功能清单，而是**判定框架决策是否合格**的准则、证据与验收门槛。
> 面向：维护者、核心贡献者、架构评审。v2.2 / v2.3 / v2.4 的取舍以此为准；认证域整体在 v2.5（见 `docs/design/auth-system.md`，2.2-6 已移交）。
>
> 证据标注：**实测**（本仓库代码/CI/基准）＞**复盘**（公开事故报告/官方文档，附 URL）＞**推断**（机制推理）。
> 本文所有 `文件:行` 引用均已对照 `dd23b92` 源码核实。

---

## 0. 执行摘要

**判断：rust-webx 赢在"架构骨架"（显式 DI、请求即端点、生命周期完备、全仓 `unsafe` 禁止），尚未赢在"运行信任"——可观测性、过载保护、分配预算、诊断质量、实时执行模型，这五项决定它能否被称为顶级框架。**

五条结论：

1. **框架的价值 = 被固化的决策 × 可逆性 ÷（概念数 × 耦合度）。** 每新增一个概念都在向用户收注意力税；护城河是"概念少、覆盖广、能退出"。
2. **性能必须是预算，不是口号。** 2.1 的 22.06 allocs/request 只在"进程内无 typed behavior"时成立（见 §4.2 F1）；实测 hyper 地板为 **6.00**，框架税才是可优化的部分（见 [`alloc-budget.md`](alloc-budget.md)）；没有分配门禁，性能三年内必腐烂。
3. **稳定性不是"不出错"，而是"出错时可预测"。** v2.1 曾有 `RequestTimeoutSeconds` 默认 0=关闭、配置绑定失败静默回退默认值、无过载卸载——前两项已在 v2.2-W0 修复，过载卸载在 2.2-3。
4. **实时不是加端点，是加执行模型。** 正确做法是"一个容器、两种生命周期（请求/连接）"，而不是把 WS 塞进 request/response。
5. **音视频不进 core。** 物理约束（UDP/RTP/抖动/拥塞）与 HTTP 正交；框架管控制面，媒体面是独立 crate 家族。

**路线：v2.2 运行信任 → v2.3 实时执行模型 → v2.4 媒体控制面与稳定收官**（工作包、写入范围与验收见 §5，滚动台账见 §10；v2.4 吸收"两大执行模型统一"、TestHost、LTS 与 starter 内容）。

**首批三项 P0"真发现"**（F1/F2/F3 均已修复；原始证据保留于 §4.2 基线快照）：

| # | 问题 | 证据 | 影响 | 成本 |
|---|---|---|---|---|
| F1 | 快路径开关是**进程级 static**：任一 Host 注册 typed pipeline，全进程所有路由退化为 `Box<dyn Any>` 调解路径 | `host/src/server.rs:1372`、`core/src/route/ext.rs:346-359` | 性能与行为随无关注册漂移，测试顺序决定走哪条路径 | 1–2 天 |
| F2 | 配置绑定失败**静默回退默认值** | `core/src/config.rs:531-561` | 一个字段拼错 → 限流/CORS 静默失效（违反 fail-closed） | 1–2 周（含测试） |
| F3 | 入站请求**无默认 deadline** | `core/src/config.rs:389,407`（默认 0=disabled） | 慢依赖可永久占住连接与许可 | 1–2 周 |

三条明确不做：不做 ORM/模板/UI/服务网格/集群管理；不引入运行期反射、动态加载、ambient DI、全局可变注册表；不为 benchmark 数字牺牲语义。

---

## 1. 第一性原理

### 1.1 框架是什么（物理定义）

框架是**一组被提前固化的决策**（路由模型、DI 生命周期、错误映射、停机顺序、默认安全值）加上**一组允许用户推翻这些决策的合同**（trait/端口/宏展开），以编译期元数据 + 运行期状态的形式进入用户二进制。

三条硬结论：

- 框架的产物是**行为**，不是 API。用户押注的是错误码映射、启动顺序、停机语义、并发模型。
- 框架的成本是**认知负荷 × 团队规模 × 维护年限**，不是代码行数。
- 框架的失败几乎不来自"功能不够"，而来自**行为不可预测 + 无法退出**。

### 1.2 价值方程（启发式）

```
框架价值 ≈ (被吸收的决策数 × 可逆性) / (概念数 × 耦合度) − 故障爆炸半径 × 不可观测系数
```

| 推论 | 判定方式 | 反例 |
|---|---|---|
| 决策覆盖要"一鱼多吃" | 一个概念服务 ≥3 类场景才配进核心 | 为单一场景加 `XxxResult` |
| 可逆性即逃生舱 | 每个宏有手写等价物；每个端口 ≥2 实现 | 只有宏能表达的路由 |
| 概念税要收得明白 | 核心概念清单 ≤10，新增必须删旧或证明净收益 | 同时存在 3 种响应信封、2 种中间件模型 |
| 爆炸半径要可观测 | 每个故障有码、有指标、有 trace | 500 只有一句 `Internal error` |

### 1.3 五个不可约约束

1. **无 GC** → 分配就是延迟预算；所有权就是 API 形状。
2. **编译期一切** → 没有反射/动态代理；宏是扩展语言；编译时间是 UX 指标。
3. **进程即部署单元** → 启动/停机/排空/取消/重载全部归框架管；框架之外的长生命周期组件必然变成事故。
4. **故障必然发生** → 框架真正的产品是**可预测的降级**，不是 happy path。
5. **用户是维护多年的团队** → **语义稳定**优先于功能速度；破坏一次信任要用三年补。

### 1.4 十二条宪法原则（每条都有检验）

| # | 原则 | 含义 | 检验方式 |
|---|---|---|---|
| P1 | 一个关注点一种写法 | 路由/绑定/结果/错误/鉴权/后台/实时各一条主路径 + 文档化逃生舱 | 文档中同一件事示例 >1 即缺陷 |
| P2 | 无隐式状态 | 无 task-local、无全局可变注册表、无隐藏顺序 | 检索 `thread_local`/`static mut`；依赖必须出现在构造器或请求字段 |
| P3 | 默认安全、失败关闭 | 生产默认值安全；坏配置拒绝而非回退默认 | 每个默认值配威胁模型 + 负例测试（当前 F2 违反） |
| P4 | 性能是预算 | 热路径每层声明分配/复制/系统调用上限 | 分配计数 + p99 门禁（§6.1） |
| P5 | 可诊断 | 每个失败有码、有因果链、有 request id；能启动校验的全部前置 | "任意 500 能否 5 分钟内解释清楚"演练 |
| P6 | 生命周期完备 | 启动序/停机序/排空/取消/释放都有定义 | 满载 SIGTERM：任务归零、连接排空、无泄漏 |
| P7 | 一切有界 | 队列有界、等待有超时、重试有预算 | 卡死依赖的混沌测试不得 OOM |
| P8 | 语义版本 | 行为也是 API；弃用 ≥2 minor；MSRV 策略显式 | N-2 用户代码升级测试 + semver 检查 |
| P9 | 端口而非插件 | 扩展点 = 稳定 trait + ≥2 实现 + 契约测试 | 每个端口有 fake/real/契约测试 |
| P10 | 小核大生态 | core 只含传输/生命周期/DI/端口声明 | 依赖方向审计 + core 依赖预算 |
| P11 | 错误路径也是热路径 | 过载时拒绝/日志/序列化比正常路径更贵 | 错误路径分配与耗时预算（单独测） |
| P12 | 控制面 fail-closed | 配置/热更新/发现数据的非法状态拒绝，保留 last-known-good | 注入坏快照：旧值保留、告警、不降级 |

> 已有文档的 SOLID 原则回答"为什么这样分层"；本文回答"一个决策凭什么算合格"。SOLID 是结构，P1–P12 是验收。

### 1.5 顶级框架的"人本定义"

- 新工程师 **10 分钟**内交付一个带鉴权、可观测、安全的端点；
- on-call **5 分钟**内能解释任意一个 500 的成因；
- 维护者跨 **两个 minor** 升级无需通读迁移指南；
- 压测时**性能可预测**（p99 不塌、内存不涨、队列不爆）。

这四条比任何 benchmark 数字更能定义"顶级"。

---

## 2. 行业证据：几十年的成功与失败

> 只保留**机制**，不保留功能清单。每条给"当时约束 / 今天是否仍成立 / 对 rust-webx 的裁决"。

### 2.1 成功机制（可迁移）

| 案例 | 赢的机制（不是功能） | 关键约束 | 今天仍成立？ | 对 rust-webx 的裁决 |
|---|---|---|---|---|
| Spring Boot | 约定默认 + starter 收敛依赖；`--debug` 打印"哪些自动配置生效、为什么"，`exclude` 可退出 | J2EE 配置地狱 | 成立 | **采纳**：starter crate 承载适配器；**采纳**"配置生效报告" |
| ASP.NET Core | 一套心智模型贯通 DI/中间件/Options/HostedService/ProblemDetails，启动期 fail-fast | 企业级一致性 | 成立 | **已采纳，继续守**：别让 Mediator 变成第二套 |
| Django | 电池 + 稳定 API + 官方弃用时间表（发布日历即契约） | 快速交付 | 成立 | **采纳**：弃用预算制度化 |
| Phoenix (Elixir) | 借 BEAM 而非自造并发；Channel/Presence/PubSub 是一等公民 | 实时是产品核心 | 成立 | **采纳**：实时是第二执行模型；押注 tokio 而非自造运行时 |
| Go net/http | 极小稳定核心 + Go 1 兼容承诺 | 生态补位 | 成立 | **采纳**：小核；但结构（DI/生命周期）仍需框架给 |
| Netty / Vert.x | 只做传输层/事件循环门面，被 gRPC 等复用 | JVM 阻塞 IO | 部分（tokio 已解决） | **部分采纳**：显式 handler chain 的可诊断性 |
| Rails | omakase 单一菜单 + CoC + `app:update` 升级工具 | 手工装配时代 | 部分 | **采纳**：单一推荐栈 + 升级工具；**不采纳**隐式加载 |
| NestJS | 模块/DI/装饰器给大团队结构 | JS 缺类型 | 部分 | **采纳**：结构即产品；**规避**装饰器膨胀 |

### 2.2 失败机制（"死亡模式"）

| 死亡模式 | 代表 | 机制 | rust-webx 风险点 | 防线 |
|---|---|---|---|---|
| 复杂度税 | Micronaut/Quarkus 的编译期 AOP、Dropwizard 配置爆炸 | 为性能/灵活付的概念成本超过收益 | 宏 + DI + 端口数量膨胀 | 概念预算 ≤10；新增必须删旧 |
| 不可解释魔法 | Rails 隐式加载、Spring 组件扫描 | 行为不可预测，排障靠猜 | 编译期 inventory 的隐式注册 | 每个宏有手写等价物 + `cargo expand` 文档 |
| 破坏性升级 | Play 1→2、Angular 1→2、Python 2→3 | 升级=重写，生态断裂 | 为 2.x 引入破坏性重构的冲动 | 只做加性演进；等价重构放到下一个 major |
| 许可/治理事故 | Akka 改 BSL | 信任崩塌，一夜流失 | 无（MIT） | 许可证与治理写进宪法 |
| 核心膨胀 | 框架自带 ORM/模板/UI | 锁死、迭代慢 | EF 集成、AI gateway 已外置（正确） | 领域能力只进独立 crate |
| 性能神话 | Node 单线程神话、benchmark 驱动特化 | 真实负载下崩塌 | 22.06 allocs 被当卖点 | 数字只作回归信号 + 真实负载 p99 |
| 生态饥饿 | 单维护者框架 | 无人补位，版本停更 | 贡献者集中 | 端口 + 契约测试降低贡献门槛 |
| 文档债 | 大量框架 | 示例不能编译 | 参考实现 docbit 违反自家纪律（§4.2） | 文档即测试：doctest/example 进 CI |
| 观测缺失 | 多数自研框架 | 上线即盲飞 | 仅 4 个状态计数器 | 观测即合同 |

### 2.3 不可迁移的"经验"

- **运行期反射/注解扫描**：Rust 无反射，编译期/宏是唯一正解；把 Spring 的"扫描"照搬成运行期注册是倒退。
- **GC 调优经验**：无 GC 下"减少分配"比"调堆"重要一个数量级。
- **动态语言魔法**：Rails autoload 无等价物；显式性要当特性而不是缺陷。
- **JVM 启动优化**（GraalVM/Quarkus）：Rust 启动已毫秒级，不要为此引入复杂度。
- **微服务治理平台**：服务网格/发现/配置中心属运维层（K8s/Istio），框架不该重复实现。
- **微框架自由组合**（Express/Koa 中间件海洋）：Rust 跨 crate 类型不兼容、组合与编译成本高，应给**已验证的黄金栈**而非无限组合。

### 2.4 两条被低估的"元教训"

1. **默认必须可解释，也必须可退出。** 魔法不是原罪；"不可解释、不可退出"才是。Spring Boot 打印自动配置报告，Dropwizard 把 Jetty/Jersey/Jackson 锁死成唯一组合而输给 Spring Boot——同样的"opinionated"，命运相反。
2. **版本节奏本身就是承诺。** 外部采用者按 semver 定价：本仓库 `CHANGELOG.md` 显示 **1.0.0（2026-11-11）→ 2.0.0（2026-11-20）仅 9 天**，`git shortlog` 仅 3 个提交身份。这不会影响技术判断，但会直接影响外部团队"敢不敢押注"——建议 2.x 内只做加法、公开 RFC、增加第二维护者、提供 LTS。

---

## 3. 对抗审查（红队）

> 方法：每场辩论给出"最强正方 → 最强反方 → 证据 → 裁决 → 若反方赢的代价 → 翻案条件"。
> 裁决结果（对应红队 J1–J12）：维持 4、修改 8、删除 0——没有需要推翻重做的架构，问题集中在**守卫、边界与默认值**。

### 3.1 三场战略辩论

#### D1：做"ASP.NET Core 的 Rust 版"还是"Rust 原生最佳实践"？

- **正方（身份派）**：企业心智模型已被 ASP.NET Core 验证；DI + 中间件 + Options + HostedService + ProblemDetails 是低学习成本组合，差异化明确。
- **反方（原生派）**：Rust 生态奖励显式、组合、零成本抽象；把 .NET 的运行期容器/Mediator 搬来，会"两头不靠"——没有反射的便利，却背上容器的复杂度。
- **证据**：`core/src/mediator` 的 boxed dispatch 与 `route/scan.rs:210-223` 的 `Box<dyn Any>` 是运行期容器的直接成本；Rust 主流（Axum/tower）走泛型 + 显式 state。
- **裁决**：**保留运行模型（生命周期/DI/中间件/Options），剥离魔法（运行期反射式注册、Mediator 作为核心概念）。** 对外话术："ASP.NET Core 的运维模型 + Rust 的编译期保证。"
- **代价**：文档层面"去 Mediator 中心化"，不动代码。
- **翻案条件**：Mediator 真实使用率（issue/示例/调研）>30% 时重新提升。

#### D2：性能优先还是易用性优先？

- **正方**：性能是硬指标，Rust 用户为此而来；benchmark 是获客成本最低的营销。
- **反方**：性能是约束，易用性是目标；为 5% 吞吐牺牲一个概念，三年后的流失远大于赢得的排名。
- **证据**：22.06 allocs/request 与 152k RPS 中位（`performance-2.1.0.md`）；真正决定生产体验的是 p99 与内存。
- **裁决**：**性能预算化**：热路径设硬上限，其余以正确性/可诊断性优先；benchmark 对"预算"负责，不对"排名"负责。
- **翻案条件**：目标用户转为极致低延迟场景时，收紧阈值而非改架构。

#### D3：继续加内置能力还是守住小核？

- **正方**：内置越多"开箱即用"越强（Spring Boot 路径）。
- **反方**：核心每加一个能力，就多一份语义承诺、一条升级路径、一个安全面；Rails/Django 的"电池"最终都成迁移负担。
- **裁决**：**端口 + starter crate**。core 只保留传输/生命周期/DI/端口；数据库、缓存、消息、对象存储、OTel 全部独立 crate。
- **翻案条件**：某能力被 ≥80% 用户使用且无法用端口表达（需框架内部状态），才考虑进 core。

### 3.2 九场技术辩论

#### D4：编译期 inventory 注册 vs 显式运行期注册

- **正方**：零配置、请求即端点、无路由表；跨 crate 自动发现；与 ASP.NET Core 体验一致。
- **反方**：链接顺序不确定、重复注册运行期才发现、增量编译与 dead-code stripping 有平台差异、测试隔离困难、错误在启动期才暴露。
- **证据**：`route/scan.rs` 的 `inventory::collect!`；`host/src/server.rs:1386` 已有重复直接注册的启动期报错（说明重复是真实风险）；`core/src/route/diagnostics.rs:85` 仍写着"last wins at runtime"。更关键的三点（实测）：① `IRequest<T>` 是空 marker（`mediator.rs:29-33`），路由元数据由宏另交 5 个 inventory 项，"类型与路由"是弱关联；② `#[get]` 宏不校验 impl 的 trait（解析失败静默回退 `()`），把 `#[get]` 写在错误 impl 上**不会编译失败**，只在启动期以 orphan/501 暴露——"编译期安全"实际降级为"启动期检查"；③ `inventory` 对整条链接生效，任何依赖库的 `#[get]` 都会进入宿主路由表，而 `add_request_endpoints()` 只是置位一个**从未被读取**的静态量（`core/src/route/ext.rs:203-208,361-365`），Host 无条件扫描。
- **裁决**：**保留编译期注册**，补齐：① 启动期确定性排序（`(method, path, req_type_id)`）并删除"last wins"语义；② 重复/冲突报错列出双方类型与位置；③ 宏展开期校验 impl 为 `IRequest<..>`，否则 `compile_error!`；④ 扫描白名单/依赖隔离（`routes_from(..)` 或显式开关）；⑤ 删除短名回退（`server.rs:1399-1402` 的同名类型跨模块覆盖风险）；⑥ 提供 `direct-only` 裁剪 feature（当前每端点 8 个 inventory 项 + 2 个 async dispatch 函数，500 端点约 4000 个静态，链接与编译线性膨胀）。
- **代价**：排序与诊断 ~3 天。
- **翻案条件**：出现跨 dylib/插件需求时，改为"显式注册为主 + inventory 为辅"。

#### D5：每请求 DI scope + owned DbContext

- **正方**：请求即工作单元，`DbContext` 天然隔离，Dispose 确定；`handle(&mut self)` 无需内部可变性。
- **反方**：每请求构造 handler + Box + scope 解析是固定成本；严格作用域让"从 root 取 Scoped"报错，第一小时体验差；连接级共享状态无处安放。
- **证据**：`scan.rs:199-223` 每请求工厂 + `Box<dyn Any>`；`dispatch_runtime.rs` 明确无 ambient fallback。
- **裁决**：**保留请求 scope**，增加**连接 scope**作为第二种一等生命周期（实时），并让作用域错误信息给出"用 `scope()`/`owned`"的修复建议。
- **代价**：新作用域语义 + 文档 + 测试，~2 周。
- **翻案条件**：分配预算不达标时评估 handler 池化（牺牲 `&mut self` 语义，慎用）。

#### D6：per-worker current-thread runtime

- **正方**：无跨核迁移 → 缓存局部性、可预测 p99；停机按 worker 独立排空。
- **反方**：单请求阻塞卡住整条 worker；负载不均无法 work-stealing；每 worker 一套 runtime/blocking pool。
- **证据**：`engine/src/worker.rs:204-214`；`config.rs` `MAX_WORKERS=64`。业界判据（复盘+推断）：CPU 方差 p99/p50<2、零阻塞、每核分片状态、p99 目标 <~0.5ms 时 per-core 才净胜；否则 multi-thread 容错更好。**两者都救不了**：handler 内同步阻塞、无 deadline、无准入。
- **裁决**：**保留默认**；补"阻塞检测"（debug 下 poll 超时告警 + `spawn_blocking` 审计）与 per-worker 队列深度指标；文档明确"阻塞/CPU 密集必须出 runtime"。
- **代价**：指标 + lint，~1 周。
- **翻案条件**：混合负载压测中 p99 比 multi-thread 差 >20%。

#### D7：全局连接信号量 + 无排队超时

- **正方**：硬上限保护 FD/内存；所有 listener 共享；实现简单。
- **反方**：饱和后新连接在 backlog 排队且无超时 → 客户端挂起、重试放大；没有 503/负载卸载；per-IP 限流在 NAT 下误伤。
- **证据**：`engine/src/server.rs` 全局 `Semaphore`；`host/src/rate_limit.rs:93-120` 是**单全局 `tokio::Mutex` + 仅 per-IP**（开启即串行，多实例限额翻倍）。
- **裁决**：**修改**：准入加 deadline（超时立即 503 + `Retry-After`），暴露队列深度 gauge，per-tenant/per-user 预算挂钩；限流改分片计数 + 端口化（支持分布式实现）。
- **代价**：~2 周。
- **翻案条件**：接入层已做全局卸载时可降级为可选。

#### D8：统一 Error + problem+json

- **正方**：一处映射、状态码一致、metrics 能看到真实状态、前端统一处理。
- **反方**：`Error` 变体大量只装 `String`，没有机器可读码/retryability/source 链；EF 结构化错误在转换时丢失；错误路径也分配；且 `Error` 未 `#[non_exhaustive]`，新增变体即破坏 semver。
- **证据**：`core/src/error.rs`（17 个变体，0 个 `non_exhaustive`）。
- **裁决**：**加性演进**：增加 `code: Cow<'static, str>`、`retryable: bool`、`public_detail`、`source`；problem+json 按 **RFC 9457**（已取代 RFC 7807）输出 `type/title/status/detail/instance` + `code`/`retryable`/`traceId`；旧变体保留并加 `#[non_exhaustive]`。
- **代价**：~2 周；应用错误通过 trait 映射，不把领域枚举塞进框架 semver。
- **翻案条件**：无。

#### D9：SSE 是唯一长连接形态

- **正方**：穿透代理/CDN、天然背压、HTTP/2 多路复用、实现质量高（keep-alive、断连钩子、停机事件）。
- **反方**：单向、纯文本、浏览器 `EventSource` 不能带头、无消息语义；IM/协作/信令需要双向与二进制。
- **证据**：`core/src/stream.rs` 完整；`engine` 无 `with_upgrades`（`worker.rs:204-214`）；`host/src/auth_jwt.rs:199-212` 只读 `Authorization`，浏览器 `EventSource` 无法带头。
- **裁决**：**维持 SSE，推翻"唯一"**：引入 WebSocket + 连接作用域；SSE 保留为单向推送/兼容路径。补齐 Cookie/BFF scheme + 一次性连接票据。
- **代价**：引擎 + core + host + 测试，~2–3 个月。
- **翻案条件**：产品形态证明只需服务端推送（但连接注册表仍必须补）。

#### D10：观测最小化

- **正方**：零依赖、零开销、应用自选导出器（`IMeter` 端口）。
- **反方**：4 个状态计数器无法回答"慢在哪、饱和没有、谁在拖尾"；无直方图/span/连接 gauge；端口无实现等于没有。
- **证据**：`host/src/metrics.rs:11-17`；`core/src/metrics.rs` 注明"不提供 exporter"。
- **裁决**：**修改**：框架自带 RED 指标（按路由标签的计数 + 延迟直方图 + inflight + 连接/队列 gauge），OTel 适配器作为 feature；默认 noop，开销预算 <1%；**被取消的请求也必须计入延迟直方图**（当前是 SLO 盲区）。
- **代价**：~2 周。
- **翻案条件**：无。

#### D11：中间件是"洋葱"还是"前向 + 反向 after"？

- **正方（现状）**：`invoke` 前向 + 逆序 `after`，错误统一在 `after` 前写出，让 metrics/压缩看到真实状态；实现简单、无 `Box::pin` 链。
- **反方**：它不是洋葱：中间件无法包住下游（不能按路由做超时/重试/熔断/事务），"短路"与"异常"语义要靠 `after` 特判；而框架同时另有 `IPipelineBehavior<Req,Rsp>` 的 `Next` 模型——**两套中间件概念**违反 P1。
- **证据**：`engine/src/pipeline.rs:104-149`（前向循环 + `executed` 计数 + 逆序 after）；`core/src/pipeline.rs` 的 typed `Next`。
- **裁决**：**修改（长线）**：统一为一种 `Next` 语义（HTTP 中间件与 typed pipeline 共享心智模型），保留"错误先写、after 后跑"的可观测性收益作为实现细节，而非用户模型。
- **代价**：大（触及全部中间件）；先在 v2.4 设计，2.x 只补文档说明差异。
- **翻案条件**：若"按路由包下游"的需求（超时/重试/熔断）持续增长，则必须统一。

#### D12：参考实现是否必须合规？

- **正方**：docbit/dmbit 只是示例，能跑就行。
- **反方**：参考实现是用户复制粘贴的第一来源，也是框架承诺的"证据"；它不合规，等于框架没兑现承诺。
- **证据（实测）**：docbit 手写 JWT（`jsonwebtoken`）与口令哈希（`bcrypt`），并手写 `map_ef_error`（`docbit/handlers/src/auth.rs:10-11`、`db.rs:8`）；全仓**没有任何 crate 启用 `rust-ef` 的 `webx` feature**（`Cargo.toml:71` 仅 `rust-ef = "2.5"`）——即"EFError 裸 `?` 传播"这条卖点在仓库内未被证明。
- **裁决**：**修改**：参考实现 = 合规证明。docbit 改为 `ITokenIssuer`/`IPasswordHasher` + 启用 `webx` feature；CI 增加"参考实现禁止出现 `jsonwebtoken`/`bcrypt`/`map_ef_error`"的检查。
- **代价**：~1 周。
- **翻案条件**：无。

### 3.3 裁决汇总

| ID | 决策 | 裁决 | 变更类型 | 优先级 |
|---|---|---|---|---|
| D1 | ASP.NET 身份 vs Rust 原生 | 保留运行模型，剥离魔法 | 文档/定位 | P1 |
| D2 | 性能 vs 易用 | 性能预算化 | 工程 | P0 |
| D3 | 内置 vs 小核 | 端口 + starter | 结构 | P1 |
| D4 | inventory 注册 | 保留 + 确定性/诊断/逃生舱 | 加性 | P0 |
| D5 | 请求 scope | 保留 + 连接 scope | 加性 | P0 |
| D6 | per-worker runtime | 保留 + 阻塞检测 | 加性 | P1 |
| D7 | 全局准入 | 修改（deadline/卸载/分片限流） | 加性 | P0 |
| D8 | Error 模型 | 加性演进（code/retryable/source） | 加性 | P0 |
| D9 | SSE 唯一长连接 | 维持 SSE，加 WS + 连接模型 | 新增执行模型 | P0 |
| D10 | 观测最小化 | 修改（框架自带 RED） | 加性 | P0 |
| D11 | 中间件模型 | 2.x 文档，v2.4 统一 `Next` | 结构性（长线） | P1（文档缓解）/ v2.4 结构修复 |
| D12 | 参考实现合规 | 修改（docbit 用原生端口） | 工程 | P1 |

---

## 4. 现状体检（代码证据）

### 4.1 已被验证的强项

| 强项 | 证据 | 为什么稀缺 |
|---|---|---|
| 全 workspace `unsafe` 禁止 | `Cargo.toml` `unsafe_code = "forbid"` | 多数高性能框架都有 unsafe 热点 |
| 三平台测试 + clippy `-D warnings` + MSRV + audit + 覆盖率 + docs `-D warnings` | `.github/workflows/ci.yml` | 完整度高于多数同类项目 |
| 启动期 fail-fast | `server.rs:1386` 重复处理器、`router.rs:177-186` 路由冲突、JWT 密钥强度 | 多数框架把冲突留到运行期 |
| SSE 工程质量 | `core/src/stream.rs`（背压/心跳/寿命/断连钩子/停机事件） | 少见地把长连接纳入停机预算 |
| 引擎准入与排空 | `engine/src/config.rs`（全局上限、首请求超时、header 超时、worker 预算） | 多数框架不控制连接层 |
| 无 ambient DI | `dispatch_runtime.rs` | 显式作用域是长期可维护性的关键 |
| 出站 SSRF 默认开启 | `client.rs:305` | 安全默认值优于多数 HTTP client |
| EF 解耦 | `webx` feature 存在、core 不依赖 ORM（但参考实现未启用，见 §4.2） | 避免"框架自带 ORM"的锁死 |

### 4.2 已核实的缺陷（按严重度）

> 修复状态见 §10 台账；下表保留 v2.1.0 基线快照，便于回归评审。
> 已修复：F1（快路径 Host 化）、F2（配置严格绑定 + 未知键检测 + 热更新拒绝）、F3（默认 30s 入站 deadline + Production 拒绝关闭），均为 v2.2-W0。

| 级别 | 缺陷 | 证据 | 后果 |
|---|---|---|---|
| **P0** | 快路径开关是进程级 static | `host/src/server.rs:1372`；`core/src/route/ext.rs:346-359` | 任一 Host 注册 typed pipeline → 全进程退化到 `Box<dyn Any>` 路径；测试顺序决定性能与行为；2.1 的 22.06 allocs 只对"零 behavior"成立 |
| **P0** | 配置绑定失败静默回退默认 | `core/src/config.rs:531-561`（`unwrap_or_default` + `warn`） | 字段拼错 → 限流/CORS 静默失效（违反 P3 fail-closed） |
| **P0** | 入站请求无默认 deadline | `core/src/config.rs:389,407`（`request_timeout_seconds: 0`） | 慢依赖/挂起 handler 永久占用连接与许可 |
| **P0** | 无过载卸载 | 全局 Semaphore 无排队超时；`rate_limit.rs:93-120` 单全局 Mutex + 仅 per-IP | 饱和时挂起 + 重试风暴；NAT 误伤；多实例限额翻倍 |
| **P1** | 观测仅 4 个计数器 | `host/src/metrics.rs:11-17` | 无法定位 p99/饱和/慢依赖；取消请求不计入 |
| **P1** | Error 无码、无 `non_exhaustive` | `core/src/error.rs`（17 变体） | 客户端无法按码分支；新增变体即破坏 semver |
| **P1** | 大量公共 API 无行为 | `should_scan_endpoints`/`is_mediator_active` 仅被 re-export（`core/src/route/ext.rs:361-365`） | 用户以为能控制扫描，实际是 no-op |
| **P1** | 宏不校验 impl trait | `macros/src/endpoint.rs:90-91,374-387`（解析失败回退 `()`） | `#[get]` 写在错误 impl 上不报错，"编译期安全"降级为启动期检查 |
| **P1** | 依赖库可隐式注入路由 | `inventory` 对整条链接生效；扫描开关无消费者（`core/src/route/ext.rs:203-208`） | 加一个依赖即改变公开面/授权面，供应链风险 |
| **P1** | 两套中间件模型 | `engine/src/pipeline.rs:104-149`（前向+after）vs typed `Next` | 概念税；无法按路由包下游（超时/重试/熔断） |
| **P1** | 浏览器长连接不可鉴权 | `auth_jwt.rs:199-212` 仅 Authorization；无 Cookie scheme/CSRF | `EventSource` 无法使用；IM 浏览器推送被阻断 |
| **P1** | 参考实现不合规 | docbit 手写 JWT/bcrypt/`map_ef_error`；`Cargo.toml:71` 未启用 `webx` feature | 用户复制到反模式；EF 原生错误映射未被证明 |
| **P2** | 无 WebSocket/连接模型 | `engine/src/worker.rs:204-214`；全仓无 `with_upgrades` | IM/协作/信令需自建网关，丢失准入/停机/指标 |
| **P2** | 无分布式原语 | `IDistributedCache` 仅 KV + 内存实现；`ISessionStore` 仅吊销 | 多实例下限流/会话/在线状态失真 |
| **P2** | 测试工具薄 | 仅 `host/src/testing.rs`（spawn + 健康轮询）；0 个 trybuild、无 proptest/loom/fuzz | 宏诊断、绑定解析、并发契约无回归保护 |
| **P2** | 门禁偏"绿"不偏"稳" | CI 有 bench smoke（`ci.yml:141-150`）但无阈值；无分配/浸泡/混沌门禁；无 `cargo-semver-checks` | 性能/稳定性/semver 回归无法在合并前发现 |
| **P2** | 版本节奏与治理 | `CHANGELOG.md`：1.0.0（11-11）→2.0.0（11-20）；`git shortlog` 3 个身份 | 外部采用者按 semver 定价，信任成本高 |
| **P2** | 分发短名回退 | `host/src/server.rs:1399-1402` | 同名类型跨模块可能静默串线 |
| **P2** | 每端点代码膨胀 | 8 个 inventory 项 + 2 个 async dispatch（direct/mediated）+ binder + encoder | 500 端点约 4000 静态、1000 async fn，链接与编译线性增长 |
| **P2** | JWT 无 Issuer/Audience 配置 | `core/src/config.rs` `JwtSection` 仅 secret/secret_file/expiration | 多租户/多环境无法校验 `iss`/`aud`（可自定义 `Validation`，但默认不校验） |

### 4.3 分配实测与预算（v2.2 更新）

> 实测方法、原始数据与预算推导见 [`alloc-budget.md`](alloc-budget.md)。结论：
> **hyper 地板 6.00，框架 24.42→23.42（v2.2 优化后），框架税 17.42。**
> 原始"总分配 ≤12"的假设（地板为 0）不成立；预算改为按"地板 + 框架税"定义。

此前按代码结构推断的来源拆解保留如下（待 2.2-4b profiling 证实）：

| 来源 | 代码位置 | 可否消除 |
|---|---|---|
| 请求体整块 `Vec<u8>` | `engine/src/pipeline.rs` `body_bytes` | 可：流式/借用解析，或 `Bytes` 复用 |
| serde 反序列化缓冲 | `route/binding.rs` | 部分可 |
| 路由参数 `Vec<(name, String)>` → `HashMap<String,String>` | `engine/src/context.rs:280-289` | 可：无参数路径零分配；参数用切片视图 |
| handler `Box<dyn Any + Send>` | `route/scan.rs:210-223` | 可：scope 内构造 + 类型化槽位 |
| 每请求 future `Box::pin` | `RouteDispatchFn` / `bind` | 部分可 |
| 响应 `Box<dyn IActionResult>`（部分路径） | `action.rs` | 可：`enum` 小对象优化 |
| 限流全局 `Mutex` | `host/src/rate_limit.rs:93-120` | 可：分片/原子计数 |

> **22 不是 Rust 的极限，是当前架构的税。** 目标：v2.2 ≤12，v2.4 ≤8（同负载、同 DTO）。

### 4.4 能力缺口（相对 IM/实时产品）

| 缺口 | 影响 | 归属 |
|---|---|---|
| WebSocket + 连接作用域 | IM/协作/信令必须自建 | 框架（2.3） |
| 连接注册表/Hub/房间广播 | 每个业务重写一套 | 框架端口 + 适配器 |
| pub/sub + presence + 分布式会话/限流 | 多实例不可用 | 端口 + Redis/NATS |
| 二进制帧/消息协议 | SSE 只能 base64 | 随 WS |
| 音视频媒体面（RTP/SFU/ICE/TURN/编解码/QoS） | 完全缺失 | **独立 crate 家族，不进 core** |
| 对象存储/分片上传 | 媒体消息与录制受限 | 独立 crate |
| 端侧 SDK | 产品成败一半在端上 | 生态/独立项目 |

---

## 5. 落地方案

### 5.1 版本总览（v2.2 → v2.3 → v2.4）

| 版本 | 主题 | 解决的问题 | 出口条件（全部满足才发版） |
|---|---|---|---|
| **v2.2** | 运行信任 | 观测/错误/过载/分配/诊断/安全/参考实现 | 3 个 P0 修复有回归测试；RED 指标可用；默认 deadline；`/hello` ≤12 allocs；宏诊断 trybuild；`cargo-semver-checks`；docbit 合规 |
| **v2.3** | 实时执行模型 | WS + 连接作用域 + 消息即请求 + registry + 跨节点扇出（事件总线）+ 背压限流 | 10k 连接压测通过；跨节点扇出 p99 达标；慢消费者隔离；IM 参考应用端到端（登录/建群/发消息/在线/重连） |
| **v2.4** | 媒体控制面与稳定收官 | 信令/房间/TURN/录制 + 媒体适配契约 + 中间件统一 + TestHost + 语义稳定 | A/V 参考集成跑通；24h soak；chaos 5 场景；fuzz 零 panic；LTS 文档；`/hello` ≤8 allocs |

依赖关系：`v2.2 → v2.3 → v2.4` 严格串行；每个版本只做加性变更，破坏性重构留到下一个 major。

### 5.2 v2.2「运行信任」（8–10 周）

#### 5.2.1 起手三件套（第 0 周，全部为 P0）

| 项 | 动作 | 验收 |
|---|---|---|
| F1 | 快路径开关改 Host 级（`HostBuilder`/`DispatchRuntime` 字段），删除进程 static | 两个 Host 同进程：一个注册 behavior，另一个仍走快路径；分配数可断言 |
| F2 | `bind_config` 失败 → 启动 typed error（或严格模式拒绝），并打印"生效来源"（Spring `--debug` 式报告） | 拼错字段 → 启动失败；`APP__` 覆盖链可追溯 |
| F3 | `RequestTimeoutSeconds` 默认值改为非零（如 30s），关闭需显式 opt-in + 生产模式拒绝 | 挂起 handler 在预算内被取消并计入指标 |

#### 5.2.2 v2.2 工作包

| WP | 交付 | 主要文件（写入范围） | 验收 |
|---|---|---|---|
| 2.2-1 | 观测 2.0：按路由标签的计数器 + 延迟直方图（µs 桶）+ inflight + 连接/队列 gauge + 取消计数；OTel feature | `crates/core/src/metrics.rs`、`crates/host/src/metrics.rs`、`crates/engine/src/pipeline.rs`、新增 `crates/host/src/otel.rs` | 默认 noop 开销 <1%；任意 5xx 可按 `code` 聚合；取消计入 |
| 2.2-2 | Error 2.0：`code/retryable/public_detail/source` + `#[non_exhaustive]`；problem+json 按 RFC 9457 | `crates/core/src/error.rs`、`crates/core/src/problem.rs`、`crates/host/src/problem_response.rs` | 旧代码零改动；客户端可按码分支；EF 错误保留结构 |
| 2.2-3 | 过载保护：准入 deadline → 503 + `Retry-After`；per-tenant 预算；限流分片 + 分布式端口 | `crates/engine/src/{config,server,worker}.rs`、`crates/host/src/rate_limit.rs` | 饱和压测：无挂起、无重试放大、RSS 封顶 |
| 2.2-4 | 分配预算：`alloc-probe` 探针（框架 / 裸 hyper 双模式）+ 基线 + request-id/JSON 优化 | `crates/alloc-probe/`、`crates/core/src/json.rs`、`crates/engine/src/response.rs` | 框架税门禁：2.2 ≤18，2.4 ≤8（总分配 = 地板 6.0 + 税） |
| 2.2-5 | 诊断与注册守卫：宏校验 impl trait、扫描白名单、删短名回退、`direct-only` feature、trybuild | `crates/macros/src/{endpoint,handler}.rs`、`crates/core/src/route/{ext,scan,diagnostics}.rs`、`crates/host/src/server.rs`、新增 `crates/macros/tests/` | 错误 impl 直接编译失败；依赖不能隐式注入路由 |
| 2.2-6 | **认证域整体移交 v2.5**：JWT `Issuer`/`Audience`、Cookie/BFF + CSRF、连接票据等全部认证工作项移至 `2.5`（M1–M4，见 `docs/design/auth-system.md`） | 2.2 不写认证实现；只在共享底层（Error codes/ProblemDetails/IMeter）做 additive 对齐 | 2.2 出口不含认证；认证验收以 auth-system.md 为准 |
| 2.2-7 | 参考实现合规：docbit/dmbit 认证改用 `ITokenIssuer`/`IPasswordHasher`；EF 错误改用官方约定路径 | `docbit/handlers/src/auth.rs`、`compat`、CI 检查步骤 | CI 禁止参考实现手写 `jsonwebtoken`/`bcrypt`；EF `?` 迁移标注上游阻塞（rust-ef 2.5 `webx` feature 仅支持 core 1.x） |
| 2.2-8 | CI 门禁与发布：分配计数、wire 不变量、`cargo-semver-checks`、N-2 升级、soak | `.github/workflows/ci.yml`、`scripts/`、`docs/rust-webx/16-migration/` | 门禁全绿；语义稳定承诺可执行 |

### 5.3 v2.3「实时执行模型」（10–12 周）

**设计原则：一个容器，两种生命周期。** 不为实时另造编程模型，而是让"消息 = 请求"复用既有语义。

| WP | 交付 | 主要文件（写入范围） | 验收 |
|---|---|---|---|
| 2.3-1 | 引擎 WS 升级：`serve_connection_with_upgrades`；`IConnection` 抽象（WS/SSE/HTTP-2 stream）；host 会话层用 `tokio-tungstenite` 做帧处理并提供 `use_websocket` | `crates/engine/src/{server,worker,context}.rs`、`crates/core/src/{http,realtime}.rs`、`crates/host/src/*` | 升级后仍受准入/停机/指标管辖；端到端回显测试 |
| 2.3-2 | 连接作用域：`connection` DI scope（长生命周期，随会话释放）+ `ConnectionContext`（identity/trace/resolve）；每消息子 scope 与 `#[inject(connection)]` 随 2.3-3 | `crates/core/src/realtime.rs`、`crates/host/src/websocket.rs` | 连接内共享状态有明确归属；连接间隔离；会话结束确定性释放 |
| 2.3-3 | 消息即请求：帧 → `IRequest<T>` → Mediator 复用同一 Handler/结果/错误管线（每消息子作用域）；帧绑定/升级鉴权在 2.3-3b | `crates/host/src/websocket.rs`、`crates/core/src/request_context.rs` | HTTP 与 WS 共用 Handler，业务零分叉；错误帧与 401 语义一致 |
| 2.3-4 | 扇出：`IConnectionRegistry`（进程内，连接/用户/房间 + presence TTL）+ 跨节点扇出复用事件传输（`RealtimeFanoutEvent`，不新增 pub/sub 端口；见 ADR） | `crates/core/src/realtime/{registry,fanout}.rs`、`crates/host/src/realtime_fanout.rs`、`crates/host/src/websocket.rs` | 房间广播 p99 可测；presence TTL 正确；跨节点扇出语义（at-most-once/顺序/失败隔离）有测试 |
| 2.3-5 | 背压与连接级限流：每连接有界出站队列（writer task，Drop/Disconnect）、入站 1s 滑窗 msg/s+bytes/s、帧上限、空闲超时、`ConnectionStats` | `crates/core/src/realtime.rs`、`crates/host/src/websocket.rs` | 慢客户端不阻塞发送方；单连接滥用自动关闭 |
| 2.3-6 | 实时观测：`realtime_*` 指标（连接 gauge/开闭、消息 in/out/字节、丢帧、限流、关闭码、分发延迟直方图） | `crates/core/src/realtime/metrics.rs`、`crates/host/src/{metrics,websocket}.rs` | 任意连接问题可在 `/metrics` 定位 |
| 2.3-7 | IM 参考应用：登录/建群/发消息/在线状态/断线重连/离线补拉 | `dmbit/*`（或新增 `imbit/*`） | 端到端脚本跑通；重连零消息丢失（至少一次 + 幂等） |

**v2.3 出口门禁**：10k 连接压测（p99 与内存达标）；跨节点扇出 p99 ≤ 目标；慢消费者隔离测试；滚动发布零丢失；IM 参考应用端到端脚本进 CI。

### 5.4 v2.4「媒体控制面与稳定收官」（12–16 周）

| WP | 交付 | 主要文件（写入范围） | 验收 |
|---|---|---|---|
| 2.4-1 | 信令与房间控制面：`SignalingMessage`/`SignalingKind`（服务端打标、成员/目标校验）+ TURN REST 凭据（HMAC-SHA1、TTL 限制）+ IM 参考应用 1:1/群组信令 | `crates/core/src/realtime/signaling.rs`、`imbit/*`、`docs/design/media-contract.md` | 1v1 与群组通话建立流程可跑通（端到端测试） |
| 2.4-2 | 对象存储端口 + 录制元数据：`IObjectStore`（put/get/head/delete + 分片续传 + presign 可选）与内存/本地文件两实现；`RecordingMeta` 播放索引 | `crates/core/src/storage.rs`、`imbit/src/recording.rs` | 录制文件可上传/断点续传/回放；端口有 fake/real 两实现 |
| 2.4-3 | 媒体面适配契约：`IMediaPlaneAdapter`（加入/离开/发布/订阅/统计/关房）+ `MediaStats` QoS 摄入 `IMeter`（`media_*`）；内存参考适配器通过契约测试 | `crates/core/src/media.rs` | 至少 1 个参考适配器通过契约测试 |
| 2.4-4 | 中间件统一 `Next` 语义（D11）：`NextStage` continuation + 旧式默认桥接 + 构建期执行器选择（默认零分配）+ `TimeoutMiddleware` | `crates/core/src/middleware.rs`、`crates/engine/src/pipeline.rs` | 旧中间件零迁移；超时/重试/熔断可按管道组合；错误先转响应再执行 `after` |
| 2.4-5 | 测试基座：`TestHost`（进程内分发、零端口）+ `IClock`/`TestClock` 可控时钟 + `ChaosControl`（失败/延迟注入）+ 确定性 runtime 指引 | `crates/host/src/testhost.rs`、`crates/core/src/time.rs`、`crates/webx/tests/test_host_test.rs` | 框架核心能力有不依赖端口的测试形态（HTTP 字节级仍用临时端口 TestServer） |
| 2.4-6 | 语义稳定与 LTS：稳定面清单 + 2.x 不弃用/3.x 弃用窗口 ≥2 minor（登记表）+ LTS 分支与补丁承诺 + 升级指南 + semver/N-2 门禁；Mediator 可选 crate / core 拆分推迟（预算驱动） | `docs/design/{compatibility,deprecations}.md`、`docs/rust-webx/16-migration/upgrade-to-2.4.md`、`compat/upgrade-smoke` | 跨两个 minor 升级零代码改动（N-2 冒烟通过）；semver-checks 4 包通过 |
| 2.4-7 | starter 家族：可复用端口契约测试 crate（object-store/media-plane）+ 事件传输适配器契约（Redis/NATS 复用 `IEventPublisher`/`EventDispatcher`）+ starter 配置/装配/集成计划文档 | `crates/contract-tests/`、`docs/design/starters.md`、`docs/design/mediator-handler-architecture.md` | 内存参考实现通过契约（PR CI）；真实 SDK 适配器排入 2.5（需外部服务） |
| 2.4-8 | 收官门禁：chaos 5 场景 + 线路 fuzz-lite + wire p99 + 短时 soak + 安全审计（PR audit）+ 分配复测 | `.github/workflows/{ci,stability}.yml`、`crates/webx/tests/{stability_chaos_test,fuzz_lite_test,wire_p99_test}.rs`、`docs/design/stability-gates.md` | 门禁绿；p99 165µs；tax 10.42；≤8 拉伸项记入 2.5/3.0 |
| 2.4-9 | 框架税收敛（原 2.2-4b）：内置中间件同步 `fast_invoke` + 跳过空 `after` + CORS 无 Origin 快路径 + `RouteTarget::direct` 直接分发 + `RequestModel::bind_inline` 无装箱绑定（17.42 → 10.42；CI 门禁 18 → 12） | `crates/core/src/{middleware,route/binding}.rs`、`crates/engine/src/{pipeline,server}.rs`、`crates/host/src/{cors,request_id,endpoint}.rs`、`crates/macros/src/*` | 分配预算显著下降并通过 CI（≤12）；≤8 拉伸目标（RespBody 枚举化 / poll ABI / 按值交接）记入 2.5/3.0 |

**v2.4 出口门禁**：A/V 参考集成（信令 + 房间 + ICE 配置 + 录制元数据）端到端；媒体面适配契约有参考实现；24h soak 无泄漏；chaos 5 场景全过；fuzz 零 panic；LTS 与弃用政策发布。

### 5.5 媒体面边界（明确不进 core）

| 层 | 归属 | 理由 |
|---|---|---|
| 信令、房间编排、TURN 凭据、录制元数据 | 框架 + 业务（HTTP/WS） | 复用鉴权、事务、观测 |
| SFU/MCU、RTP/RTCP、ICE/STUN/TURN 服务、抖动缓冲、NACK/FEC、拥塞控制 | 独立 crate 家族/独立服务 | 物理约束正交；Rust 生态已有 `str0m`/`webrtc` 等底座 |
| 编解码/转码/混流 | 外部（FFmpeg/硬件） | 许可与硬件差异大 |

### 5.6 版本级 Definition of Done

| 维度 | 每个版本必须满足 |
|---|---|
| 功能 | 工作包全部合入；参考实现/示例可运行 |
| 性能 | §6.1 预算不回退；本版目标阈值达成 |
| 稳定 | soak（v2.3 起）、chaos（v2.4 全量）、零 panic |
| 安全 | `cargo audit` 0 漏洞；默认 fail-closed；参考实现无反模式 |
| 兼容 | 加性变更；N-2 升级测试；`cargo-semver-checks` |
| 文档 | 单一心智模型；示例可编译；迁移说明 |
| 观测 | 新增能力自带指标/trace；`/metrics` 可定位 |

---

## 6. 度量与门禁

### 6.1 性能预算（CI 可执行，阈值按"只对低变异信号设硬门禁"原则）

| 指标 | 形式 | 阈值 | 不 flaky 的理由 |
|---|---|---|---|
| 框架税（总分配 − hyper 地板） | `alloc-probe` 双模式同窗口测量 | 2.2 ≤18；2.4 ≤8；地板同步记录 | 同机同窗口，抵消版本漂移 |
| 指令数/L1 miss | `iai-callgrind` 替代墙钟 | +2% 失败 | 计数无噪声 |
| p99 | 专用 runner，基线与候选各 5 轮取中位 | <1.25× 基线 | 同机同轮消漂移 |
| wire 不变量 | 首请求超时释放 permit、满载 SIGTERM、413/431、SSE drain | 断言语义 | 语义确定 |
| soak 24–72h | RSS 斜率、fd 数、稳态 5xx、p99 漂移 | <1%/h、恒定、0、<10% | 趋势指标 |
| 过载 | 开环 2× 容量 | 准入 p99<SLO、拒绝 <5ms、RSS 封顶 | 断言语义 |
| 启动 | 冷启动到 ready | <100ms（v2.4 <50ms） | 同机 |
| 内存 | 空闲连接 RSS | ≤32KiB/连接（v2.4 ≤16KiB） | 同机 |

> 共享 CI 不设绝对 RPS（与 `performance-2.1.0.md` 的结论一致）；火焰图只做评审差分，不自动 block。

### 6.2 易用性指标

| 指标 | 门槛 |
|---|---|
| 第一个受保护端点（TTFPE） | ≤10 分钟、≤12 步、≤10 概念、≤1 个编译错误 |
| 核心概念数 | ≤10（超出必须删旧） |
| 宏诊断 | 100% 覆盖 trybuild 快照；每条错误含 span + 修复建议 |
| 升级成本 | N-2 代码编译零错误（仅弃用警告） |

> 参考基线（Rust 生态对照，外部分析）：受保护端点约需 12 个概念，Axum 约 7–8 个；这是可以在不牺牲 DI 正确性的前提下压缩的。

### 6.3 稳定性门禁

- 浸泡：夜间 `stability_soak_test`（默认 300s，手动 `SOAK_SECONDS=86400` 做 24h；Linux RSS 增长 ≤ `SOAK_MAX_RSS_GROWTH_MB`，请求零失败，停机后端口关闭）。
- 混沌：连接风暴（503 卸载 + 恢复）、带载 `shutdown()`（零 5xx）、静默上游（typed timeout）；依赖变慢/磁盘满/时钟偏移仍待补。
- fuzz：绑定/路由/SSE 帧解析语料零 panic（2.2-8d）。
- `cargo-semver-checks` + N-2 升级测试。
- 全 workspace `unsafe` 禁止（已有）+ `cargo-deny`。
- 满载 SIGTERM 与取消计费纳入 wire 级测试（当前是 SLO 盲区）。

### 6.4 治理与版本

| 项 | 现状 | 目标 |
|---|---|---|
| 许可 | MIT | 写进宪法，永不改 |
| 维护者 | 3 个提交身份（疑似同一人） | ≥2 个组织/维护者；公开 RFC |
| 发布节奏 | 1.0→2.0 仅 9 天 | 2.x 只做加法；major ≥18 个月 |
| 弃用政策 | 迁移文档存在，无自动检查 | 弃用 ≥2 minor + `cargo-semver-checks` |
| 支持窗口 | 未声明 | 当前 + 前两个 minor；LTS 18 个月 |

---

## 7. 反面清单（明确不做）

1. 不做 ORM、模板引擎、UI 框架、服务网格、集群管理。
2. 不做运行期反射/动态插件加载/ambient DI。
3. 不引入第二套路由/DI/错误/结果/后台/中间件模型。
4. 不让 core 出现领域词（IM、媒体、AI、支付）。
5. 不为 benchmark 特化热路径（会侵蚀语义）。
6. 不做"万能配置"（配置项爆炸是 Dropwizard 的死因之一）。
7. 不承诺"零配置"；承诺"默认值正确 + 偏离可解释"。
8. 不用"last wins"语义处理注册冲突；冲突必须启动失败。
9. 不留"进程级开关"控制单 Host 行为。
10. 不让参考实现示范反模式。
11. 不让依赖库隐式注入路由；扫描必须可白名单/可关闭。
12. 不用类型短名做分发键；只用 `TypeId`（或完全限定路径）。

---

## 8. 执行节奏与里程碑（滚动推进）

### 8.1 里程碑

| 里程碑 | 目标窗口 | 关键交付 | 发布门禁 |
|---|---|---|---|
| **M2.2** | 第 8–10 周 | 起手三件套 + 8 个工作包 + 参考实现合规 | §5.6 全维度 + 分配 ≤12 + 默认 deadline |
| **M2.3** | 第 18–22 周 | WS/连接作用域/消息即请求/扇出/背压 + IM 参考应用 | 10k 连接压测 + 跨节点扇出 + 重连零丢失 |
| **M2.4** | 第 30–38 周 | 信令/房间/TURN/录制 + 媒体适配契约 + 中间件统一 + LTS | 24h soak + chaos 5 + fuzz + `/hello` ≤8 |

### 8.2 每轮推进纪律（持续目标）

1. 每轮只推进**一个工作包的最小可合并切片**，不做"顺手重构"。
2. 测试与实现同批提交：新增行为必须有负例/边界测试。
3. 每轮结束跑 `cargo test -p <crate>` + `cargo clippy -p <crate> --all-targets -- -D warnings` + 相关门禁。
4. 更新本文 §10 台账与 `CHANGELOG.md`；发现与宪法冲突时**先改宪法再改代码**。
5. 任何版本只做加性变更；需要破坏性重构的，记入下一个 major 的待办，不在 2.x 内做。

### 8.3 风险即触发信号

见 §9；每轮开始先扫一遍"触发信号"，命中则先处理再继续推进。

---

## 9. 风险与翻案条件

| 风险 | 触发信号 | 应对 |
|---|---|---|
| WS 让 core 膨胀 | 核心概念 >10 或编译时间 +30% | 连接层独立 crate，ports 进 core |
| 分配优化伤害可读性 | review 成本/复杂度上升 | 只做高杠杆项，保留可读实现 |
| 观测开销超标 | 开关对比 >1% | 采样 + noop 默认 |
| 语义稳定导致演进停滞 | 关键需求被旧行为阻塞 | 新 API 并行，而非改旧行为 |
| 采用率不足 | TTFPE/首周留存不达标 | 优先 DX 而非新能力 |
| 治理单点 | 维护者 <2 组织 | 公开 RFC + 联合维护 |

**翻案条件总则**：本文每条裁决都必须在"触发信号"出现时被重新评审；宪法本身也受版本控制。

---

## 10. 进展台账（持续目标）

> 状态：`[ ]` 未开始 / `[~]` 进行中 / `[x]` 完成（附证据链接）。每轮推进后更新本表与 `CHANGELOG.md`。
> 版本严格串行：v2.2 全部完成后才开 v2.3。
>
> **架构复核（2026-11-21，事件体系以本地最新迭代为准）**：中介者 Handler 体系与事件分发已形成
> 单一执行模型：跨进程只有 `IEventPublisher` 一条传输通道；实时跨节点扇出改为
> `RealtimeFanoutEvent`（移除重复的 `IPubSub` 端口）；realtime 指标通过 `IMeter` 桥接导出；
> `TestHost` 与端口测试助手合并到单一 `testing` 模块。裁决与复用矩阵见
> [`mediator-handler-architecture.md`](mediator-handler-architecture.md)。

> **交付状态（2026-11-21）**：2.2「运行信任」、2.3「实时执行模型」、2.4「媒体控制面与
> 稳定收官」三条功能线均已按本台账交付并通过 CI 等价门禁复核（测试/Clippy/格式/
> Rustdoc/MSRV/SemVer/N-2/分配/安全/稳定性，见
> [`release-evidence.md`](release-evidence.md)）。已知偏差与 2.5 计划：starter 真实 SDK
> 适配器、≤8 allocs 拉伸目标、rust-ef `webx` feature 上游缺口。

> **v2.2 出口（2026-11-21）**：2.2 特性清单全部完成（2.2-4b 框架税收敛是 2.4 的性能目标，
> 移入 v2.4 工作包；2.2-6 认证域按设计移交 2.5 线，不计入出口）。版本号由并行发布线
> 统一为 `2.5.0`（2.2 的运行信任能力随该版本发布）。

### v2.2 —— 运行信任

> **W0 已完成**（F1 快路径 Host 化、F2 配置 fail-closed、F3 默认 30s deadline）。
> **并行会话**：2.2-6 安全（Cookie/CSRF/会话仓库，`auth_cookie.rs`/`csrf.rs`/`session_repo.rs`）正在进行；本会话不重复实现，避免冲突。

- [x] 2.2-W0 F1 快路径 Host 化（`server.rs` + `ext.rs` + 双 Host 回归测试）— 证据：`crates/host/tests/direct_dispatch_per_host_test.rs`；`cargo test -p rust-webx-host` / `-p rust-webx-core` + clippy 全绿
- [x] 2.2-W0 F2 配置 fail-closed + 生效来源报告 — 证据：`core::config::{try_load_appsettings, try_bind_root, try_bind_config, ConfigError, ConfigSources}`；`crates/host/tests/config_fail_closed_test.rs`；`OptionsFileWatcher` 拒绝坏快照；Production 零来源告警。注：`add_options*` 的用户 section 启动绑定仍为兼容 lossy，严格化并入 2.2-5 一并处理
- [x] 2.2-W0 F3 默认入站 deadline — 证据：`Server.RequestTimeoutSeconds` 默认 30s；`validate_request_timeout`（Production 拒绝 0）；`crates/host/tests/request_timeout_defaults_test.rs`
- [x] 2.2-1a 观测核心：按路由 RED + 延迟直方图 + inflight + 504 超时计数 + 基数保护 — 证据：`crates/host/src/metrics.rs`、`crates/webx/tests/metrics_status_test.rs`、`IHttpContext::elapsed`
- [x] 2.2-1b 连接/队列 gauge + OTel 桥接：`engine_connections_active`（`ConnectionPermit` RAII 计数，permit/握手/任务同生命周期，不泄漏）+ `engine_connections_capacity`；OTel 走 Collector 抓 `/metrics`（零依赖），进程内 exporter 留 2.4 starter — 证据：`crates/engine/src/shared.rs`、`crates/host/src/metrics.rs`、`crates/webx/tests/connection_gauges_test.rs`、`docs/rust-webx/11-production/metrics.md`
- [x] 2.2-2 Error 2.0：`Error::code()/retryable()` + `ProblemDetails` 的 `code/retryable/instance/traceId`（RFC 9457）— 证据：`crates/core/src/error.rs`、`crates/core/src/problem.rs`、`crates/webx/tests/problem_details_test.rs`。注：`#[non_exhaustive]` 与结构化 `source` 链留到 v2.4 语义稳定窗口（2.x 只做加性变更，避免 minor 破坏既有穷尽匹配）
- [x] 2.2-3a 过载保护：准入 deadline（`Server.AdmissionTimeoutMs` 默认 1000，Production 拒绝 0）→ 立即 503 + `Retry-After`；`engine_*` 准入队列指标 — 证据：`crates/engine/src/server.rs`、`crates/host/tests/admission_overload_test.rs`
- [x] 2.2-3b 限流改造：`IRateLimiter` 端口 + `InMemoryRateLimiter` 分片（1–32 shard，全局 `MaxTrackedIps` 上限；分片内淘汰为近似）+ 兼容包装 `RateLimiter` — 证据：`crates/core/src/rate_limit.rs`、`crates/host/src/rate_limit.rs`（8 个单测 + 既有 429 集成测试）
- [x] 2.2-5b 依赖库路由白名单（`routes_from`）+ 扫描开关（`use_inventory_routes`）：宏并行提交 `RouteSource`（`module_path!()`），host 按模块前缀过滤（白名单激活时 fail-closed），默认全量扫描不变 — 证据：`crates/core/src/route/scan.rs`、`crates/macros/src/endpoint.rs`、`crates/host/src/server.rs`、`crates/webx/tests/routes_from_test.rs`
- [x] 2.2-4a 分配预算基座：`alloc-probe`（框架 / 裸 hyper 双模式）+ 实测量化 + request-id 零拷贝 + JSON 预分配 — 证据：[`alloc-budget.md`](alloc-budget.md)、`crates/alloc-probe/`
- [x] 2.2-4b 框架税收敛：内置中间件去 box、直接分发去 `Box<dyn Any>`、CORS 无 Origin 快速路径——**并入 v2.4-9 完成**（tax 17.42 → 10.42，CI 门禁 ≤12）；2.2 出口以分配门禁 ≤18 为准 — 证据：[`alloc-budget.md`](alloc-budget.md) §6
- [x] 2.2-5 诊断与注册守卫：路由宏编译期校验 `impl IRequest<Response>`、`#[handler]` 拒绝未知 trait、路由确定性排序、宏路由禁止短名回退、trybuild 快照 — 证据：`crates/macros/src/{endpoint,handler}.rs`、`crates/host/src/server.rs`、`crates/webx/tests/ui/`
- [~] 2.2-6 认证域按设计移交 v2.5（不计入 2.2 出口；由 2.5 发布线交付，见 `docs/design/auth-system.md`）
- [x] 2.2-7 参考实现合规：docbit/dmbit 认证走 `ITokenIssuer`/`IPasswordHasher`（CI 禁止手写 JWT/bcrypt）；EF `?` 迁移被上游阻塞并记录（rust-ef 2.5.1 的 `webx` feature 依赖 `rust-webx-core ^1.2`，见 [`compatibility.md`](compatibility.md) §6）
- [x] 2.2-8a CI 门禁：分配门禁（`alloc-probe compare`，框架税 ≤18）+ API 稳定性冒烟（`api_stability_test`）— 证据：`.github/workflows/ci.yml`、`crates/webx/tests/api_stability_test.rs`
- [x] 2.2-8b semver 与升级门禁：`cargo-semver-checks`（基线 v2.1.0，四包全过）+ N-2 源码冒烟（同一份 2.0 代码对 `=2.0.0` 与当前路径）+ 内部 crate 精确版本钉及 CI 检查 — 证据：`.github/workflows/ci.yml`、`compat/upgrade-smoke/`、[`compatibility.md`](compatibility.md)
- [x] 2.2-8c soak/chaos：`stability_soak_test`（夜间 300s / 手动 86400s、RSS 上限、停机停听）+ `stability_chaos_test`（连接风暴 503 卸载并恢复、带载停机零 5xx、静默上游超时）— 证据：`.github/workflows/stability.yml`、[`stability-gates.md`](stability-gates.md)
- [x] 2.2-8d fuzz 语料：proptest 属性测试（Router 任意输入不 panic、静态/动态路由、分页 clamp、ProblemDetails 往返）— 证据：`crates/webx/tests/property_test.rs`、[`stability-gates.md`](stability-gates.md)

### v2.3 —— 实时执行模型

> **v2.3 出口（2026-11-21）**：WS 执行模型、连接作用域、消息即请求、注册表/扇出/presence、
> 背压与连接级限流、实时观测、IM 参考应用全部完成；跨节点 broker 适配器（2.3-4b）
> 的跨节点扇出已收敛到事件传输（`RealtimeFanoutEvent`；`IPubSub` 端口在架构复核中移除），
> Redis/NATS 适配复用 `IEventPublisher`（需外部服务，
> 不在仓库 CI 覆盖范围内）。

- [x] 2.3-1a 引擎 WS 升级 + `IConnection` 抽象：`serve_connection_with_upgrades`；`IHttpContext::set_upgrade_session` + `IUpgradeSession`；升级后连接仍受准入/指标/停机排空管辖（hyper 连接 future 在升级 IO 关闭前保持 pending）— 证据：`crates/core/src/{realtime,http}.rs`、`crates/engine/src/{context,worker,server}.rs`、`crates/engine/tests/websocket_upgrade.rs`
- [x] 2.3-1b host WebSocket 会话：`tokio-tungstenite` 帧处理 + `HostBuilder::use_websocket(path, handler)` + `IConnection` 实现 + 端到端回显（文本/二进制）、WS 路径普通请求 426、未注册路径升级失败 — 证据：`crates/host/src/websocket.rs`、`crates/webx/tests/websocket_session_test.rs`、`docs/rust-webx/05-request-pattern/websocket.md`
- [x] 2.3-2a 连接作用域：每连接一个 DI scope（`ConnectionScope`，会话结束 dispose）+ `ConnectionContext`（`meta`/`identity`/`trace_id`/`resolve`）+ `IConnection::context()`；连接级服务跨消息共享、连接间隔离、随会话释放（含测试）— 证据：`crates/core/src/realtime.rs`、`crates/host/src/websocket.rs`、`crates/webx/tests/websocket_session_test.rs`
- [x] 2.3-2b 连接上下文桥：`RequestContext::connection()`（消息 Handler 内可取连接元数据/trace/身份/连接 scope 服务）+ `RequestContext::run_in_connection`；`#[inject(connection)]` 宏注入延后（当前用 `RequestContext::connection()`，语义等价）
- [x] 2.3-3a 消息即请求：`use_websocket_request::<Req, Rsp>`（JSON 帧 → Mediator 同一 `#[handler]` 管线 → JSON 回复；每消息子作用域；结构化错误帧 `{error:{code,message,retryable}}`；坏 JSON/业务错误不杀连接）— 证据：`crates/host/src/websocket.rs`、`crates/webx/tests/websocket_message_request_test.rs`
- [x] 2.3-3b 升级鉴权与帧绑定：WebSocket 端点移到认证/用户中间件之后（Cookie/Bearer/API Key 同一管线，身份固定到连接与消息上下文）；帧解码用 `serde_path_to_error` 区分 `invalid_message`（语法）与 `validation_failed`（字段路径）— 证据：`crates/host/src/server.rs`（管道顺序）、`crates/host/src/websocket.rs`、`crates/webx/tests/websocket_message_request_test.rs`（含自定义 scheme 鉴权测试）
- [x] 2.3-4a 扇出基座：`IConnectionRegistry`（进程内，连接/用户/房间索引 + presence TTL）+ host 默认注册 + 会话自动 register/unregister；房间广播与 presence 端到端测试 — 证据：`crates/core/src/realtime/registry.rs`、`crates/host/src/websocket.rs`、`crates/webx/tests/realtime_registry_test.rs`（2.4 架构复核后原 `IPubSub` 端口移除，见 ADR）
- [x] 2.3-4b 跨节点扇出：**收敛到事件传输**——`RealtimeFanoutEvent`（wire 名 `webx.realtime.fanout`）+ 框架 `IEventHandler` 把事件写到本节点房间连接；单进程仍走 registry 直连。`IPubSub` 端口因与 `IEventPublisher` 重复而移除（架构复核，见 `mediator-handler-architecture.md`）；Redis/NATS 适配器复用 `IEventPublisher`（需外部 broker，nightly 集成）
- [x] 2.3-5 背压与连接级限流：每连接有界出站队列（独立 writer task，`try_send`，`Drop`/`Disconnect` 策略）+ 入站 1s 滑窗 msg/s、bytes/s + 帧大小上限 + 空闲超时 + `IConnection::stats()`（含 dropped/rate_limited）；端到端测试（超大帧保留连接、限速关 1008、空闲关 1001）+ host 单测（队列策略/滑窗）— 证据：`crates/core/src/realtime.rs`、`crates/host/src/websocket.rs`、`crates/webx/tests/websocket_limits_test.rs`
- [x] 2.3-6 实时观测：`RealtimeMetrics` 进程级聚合（连接开/关、消息 in/out、字节、丢帧、限流、关闭码、消息分发延迟直方图）+ `/metrics` 渲染 `realtime_*` 指标（registry 活跃连接 gauge 与注册表联动）；消息分发延迟在 `use_websocket_request` 管线测量 — 证据：`crates/core/src/realtime/metrics.rs`、`crates/host/src/{metrics,websocket}.rs`、`crates/webx/tests/realtime_metrics_test.rs`
- [x] 2.3-7 IM 参考应用（`imbit/`）：登录/建群/入群/WS 消息即请求/房间广播/幂等重试/断线重连+离线补拉（WS history 与 HTTP 补拉一致）/presence，真实 HTTP+WS 客户端端到端测试 — 证据：`imbit/`、`docs/design/im-reference.md`、`imbit/tests/e2e_im_test.rs`

### v2.4 —— 媒体控制面与稳定收官

> **v2.4-1..6 + 2.4-8 + 2.4-9 已完成，2.4-7 契约脚手架已交付（2026-11-21）**：信令控制面 + TURN REST 凭据 + 对象存储端口（续传/
> 两实现）与录制索引/回放 + 媒体面适配契约与 QoS 指标摄入（内存参考适配器通过契约测试）
> + 中间件统一 `Next` 语义（旧式零迁移、默认管道保持零分配、超时/重试/熔断可组合）
> + `TestHost` 进程内测试基座（零端口、可控时钟、chaos 注入）+ 语义稳定面/LTS/弃用政策
> 与升级指南；IM 参考应用端到端测试覆盖信令与录制。契约/迁移/测试/LTS 文档见
> [`media-contract.md`](media-contract.md)、[`middleware-next.md`](middleware-next.md)、
> [`test-host.md`](test-host.md)、[`compatibility.md`](compatibility.md)；框架税
> 17.42 → 10.42（CI 分配门禁 18 → 12），≤8 拉伸项记入 2.5/3.0；稳定性门禁
> 见 [`stability-gates.md`](stability-gates.md)（chaos 5 / fuzz-lite / p99 / soak）；
> starter 契约脚手架见 [`starters.md`](starters.md)；逐版本证据矩阵见
> [`release-evidence.md`](release-evidence.md)。

- [x] 2.4-1 信令与房间控制面：`SignalingMessage`/`SignalingKind` + TURN REST 凭据（`AppOptions.turn` 配置节、Host 启动校验、`use_turn_config` 覆盖）+ 参考应用 1:1/群组信令 — 证据：`crates/core/src/realtime/signaling.rs`、`crates/core/src/config.rs`、`crates/webx/tests/turn_config_test.rs`、`imbit/src/{contracts,handlers,lib}.rs`、`imbit/tests/signaling_e2e_test.rs`、`docs/design/media-contract.md`
- [x] 2.4-2 对象存储端口 + 录制元数据：`IObjectStore`（put/get/head/delete + 分片续传 begin/put_part/list_parts/complete/abort + 可选 presign）与两个实现（内存 fake / 本地文件 real：路径校验、分片合并、SHA1 etag）+ `RecordingMeta` 播放索引（重试替换重算）+ IM 应用分片上传/完成/回放端点与端到端测试 — 证据：`crates/core/src/storage.rs`、`imbit/src/recording.rs`、`imbit/tests/recording_e2e_test.rs`、`docs/design/media-contract.md` §6
- [x] 2.4-3 媒体面适配契约 + QoS 指标摄入：`IMediaPlaneAdapter`（join/leave/publish/subscribe/stats/close_room）+ `MediaSession`/`MediaTrackInfo`/`MediaStats` + 默认 `record_qos` 写 `media_*`（rtt/jitter/loss/bitrate/丢帧，丢包钳制）+ `InMemoryMediaPlaneAdapter` 参考实现与契约测试 — 证据：`crates/core/src/media.rs`、`docs/design/media-contract.md` §4
- [x] 2.4-4 中间件统一 `Next` 语义（D11）：`NextStage` continuation + `IMiddleware::invoke_next` 默认桥接（旧 `invoke/after` 零迁移）+ `continuation()` 标记；引擎构建期检测并只在存在新式阶段时切换 continuation 执行器（默认管道保持零分配快路径）；错误在出现位置转响应、外层 `after` 观察到真实状态；内置 `TimeoutMiddleware`（504 取消）示范超时/重试/熔断组合 — 证据：`crates/core/src/{middleware,http}.rs`、`crates/engine/src/pipeline.rs`、`crates/webx/tests/middleware_next_test.rs`、`docs/design/middleware-next.md`
- [x] 2.4-5 `TestHost` + 可控时钟 + chaos 钩子：`TestHost`（不 bind 端口，经 Mediator/HandlerCache/DI scope 分发 typed 请求，`dispatch`/`dispatch_as`、每 dispatch 新鲜 scope、`shutdown`）+ `IClock`/`SystemClock`/`TestClock`（`HostBuilder::use_clock`、Host 默认注册）+ `ChaosControl`（fail_next/delay_next/reset/dispatched）；确定性 runtime 指引（current-thread / start_paused）— 证据：`crates/core/src/time.rs`、`crates/host/src/testhost.rs`、`crates/webx/tests/test_host_test.rs`、`docs/design/test-host.md`
- [x] 2.4-6 语义稳定与 LTS：稳定面清单（HTTP 错误/绑定路由/中间件/WS 协议与关闭码/指标名/配置键/端口语义）、2.x 不新增弃用 + 3.x 弃用窗口 ≥2 minor 与登记表、LTS 分支/backport/补丁承诺、发布检查单、升级到 2.4 指南、semver-checks 例外维护流程；N-2 冒烟（2.0.0 stable / 当前 workspace）本地双通过 — 证据：`docs/design/{compatibility,deprecations}.md`、`docs/rust-webx/16-migration/upgrade-to-2.4.md`；**降级项**：Mediator 可选 crate / core 拆分推迟到 2.5+（以分配/税收证据驱动，理由见 compatibility 例外维护流程）
- [~] 2.4-7 starter 家族：**契约脚手架已交付**——`crates/contract-tests` 提供 `object_store_contract`/`media_plane_contract` 行为套件，内存参考实现在 PR CI 跑通；事件传输适配器（Redis/NATS）契约复用 `IEventPublisher`+`EventDispatcher`（`event_bus_test` + nightly 服务容器）。starter 清单/配置/装配/集成计划见 [`starters.md`](starters.md)。**剩余**：真实 SDK 适配器（postgres/redis/nats/s3/otel）需外部服务，排入 2.5
- [x] 2.4-8 收官门禁：chaos 扩到 5 场景（连接风暴 503 / 优雅停机 / 静默上游 / handler panic 隔离 / 超限 body 413 且连接存活）；新增线路级 fuzz-lite（240 条畸形输入零 panic/零挂起）与 wire p99 门禁（release p99=165µs ≤ 5ms，nightly job）；短时 soak 通过（含 RSS 断言，24h 由 nightly workflow_dispatch 触发）；安全审计为 PR CI `audit` job（RustSec）；本地复测发现并修复 RUSTSEC-2026-0387（WebAuthn 加密栈 p256 0.13→0.14 / crypto-bigint 0.7.11），`cargo audit` 恢复 0 vulnerabilities / 0 warnings；分配门禁 10.42 ≤ 12。**偏差**：`/hello` ≤8 allocs 拉伸目标未达（10.42），路径记入 alloc-budget §6 — 证据：`docs/design/stability-gates.md`、`crates/webx/tests/{stability_chaos_test,fuzz_lite_test,wire_p99_test}.rs`、`.github/workflows/stability.yml`
- [x] 2.4-9 框架税收敛（原 2.2-4b）：`fast_invoke`（RequestId/CORS 去 async box）+ `has_after`（跳过内置 after box）+ CORS 无 Origin 快路径 + `RouteTarget::direct`（无授权路由去 box）+ `RequestModel::bind_inline` RPITIT（去一层绑定 box）；**tax 17.42 → 10.42**，CI 门禁收紧 18→12；≤8 拉伸目标未达，剩余路径（RespBody 枚举化 / dispatch poll 式 ABI / 请求按值交接）记入 2.5/3.0 — 证据：`docs/design/alloc-budget.md` §6、`crates/engine/tests/` + 全量回归

---

## 附录 A：ADR 模板

```
## ADR-XX：<决策标题>
状态：提议 / 接受 / 废弃
背景：<约束与证据>
选项：A / B / C（各含代价）
裁决：<选择 + 理由>
可逆性：<如何退出>
验收：<可执行门槛>
翻案条件：<什么信号会推翻它>
```

## 附录 B：关键证据索引

| 主题 | 位置 |
|---|---|
| 快路径进程级开关 | `crates/host/src/server.rs:1372`、`crates/core/src/route/ext.rs:346-359` |
| 配置绑定静默回退 | `crates/core/src/config.rs:531-561` |
| 默认无入站 deadline | `crates/core/src/config.rs:389,407` |
| 中间件前向 + after | `crates/engine/src/pipeline.rs:104-149` |
| 限流全局 Mutex | `crates/host/src/rate_limit.rs:93-120` |
| 观测 4 计数器 | `crates/host/src/metrics.rs:11-17` |
| Error 变体 | `crates/core/src/error.rs` |
| 每请求 Box | `crates/core/src/route/scan.rs:199-223` |
| 路由参数 HashMap | `crates/engine/src/context.rs:280-289` |
| SSE 实现 | `crates/core/src/stream.rs` |
| 无 WS 升级 | `crates/engine/src/worker.rs:204-214` |
| JWT 仅 Authorization | `crates/host/src/auth_jwt.rs:199-212` |
| 参考实现反模式 | `docbit/handlers/src/auth.rs:10-11`、`docbit/handlers/src/db.rs:8` |
| 未启用 EF webx feature | `Cargo.toml:71` |
| 无行为公共 API | `crates/core/src/route/ext.rs:361-365` |
| CI 门禁 | `.github/workflows/ci.yml:14-150` |
| 性能报告 | `docs/design/performance-2.1.0.md` |
| 行业事故复盘 | Cloudflare 2019（正则回溯 + 护栏被删）、Slack 2020（陈旧 slot）、Discord 2020（半截 body → 空列表广播）、Roblox 2021（服务发现 73h）、GitHub 2018（分区无 fencing）、AWS S3 2017（手误）、Knight 2012（部分部署） |

## 附录 C：外部参考（复盘与规范）

- Cloudflare 2019-07-02：https://blog.cloudflare.com/details-of-the-cloudflare-outage-on-july-2-2019/
- Slack 2020-05-12：https://slack.engineering/a-terrible-horrible-no-good-very-bad-day-at-slack/
- Discord 2020-03-20：https://discord.statuspage.io/incidents/62gt9cgjwdgf
- Roblox 2021-10-28：https://blog.roblox.com/2022/01/roblox-return-to-service-10-28-10-31-2021/
- GitHub 2018-10-21：https://github.blog/2018-10-30-oct21-post-incident-analysis/
- AWS S3 2017-02-28：https://aws.amazon.com/message/41926/
- Knight Capital（SEC）：https://www.sec.gov/litigation/admin/2013/34-70694.pdf
- AWS 超时/重试/抖动（Builders' Library）：https://aws.amazon.com/builders-library/timeouts-retries-and-backoff-with-jitter/
- Google SRE 级联失效：https://sre.google/sre-book/addressing-cascading-failures/
- RFC 9457（Problem Details）：https://www.rfc-editor.org/rfc/rfc9457
- Tokio `select!` 取消安全：https://docs.rs/tokio/latest/tokio/macro.select.html
- async 阻塞说明：https://ryhl.io/blog/async-what-is-blocking/
- `iai-callgrind`（指令级基准）：https://github.com/iai-callgrind/iai-callgrind
- Django 弃用时间表（弃用制度化的范本）：https://docs.djangoproject.com/en/6.0/internals/deprecation/
