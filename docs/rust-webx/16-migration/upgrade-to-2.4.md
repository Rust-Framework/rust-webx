# 升级到 2.4（2.x 加性演进）

> 版本线说明：仓库当前发布线为 **2.5.0**，其中 2.2「运行信任」、2.3「实时执行
> 模型」、2.4「媒体控制面与稳定性收官」是三条功能线。**按 SemVer 全部为加性演进**：
> 只使用 2.1 API 的应用升级到 2.4 零代码改动（N-2 冒烟门禁覆盖）。

## 1. 一句话结论

- 已有 2.1 应用：升级依赖版本即可，**不需要改代码**；
- 想用新能力：按需采用下方"可选迁移"；
- 唯一的公开结构体字段新增（`engine::Pipeline`）命中
  [`constructible_struct_adds_field = "allow"`](../../design/compatibility.md#3-已知例外配置-section-的新增字段)，
  用 `Pipeline::new(stages)` 构造不受影响。

## 2. 升级步骤

```toml
# 保持锁步：umbrella 与内部 crate 同版本
rust-webx = "=2.5.0"      # 或你实际发布的同版本线
```

```text
cargo update -p rust-webx
cargo test
cargo semver-checks --package your-crate   # 可选：检查自己的公开 API
```

检查清单：

1. 内部 crate 精确版本（`=x.y.z`）保持一致，不要混版本；
2. `cargo audit` 无新增告警；
3. 若用 `appsettings.json` 配置，新增键都可选（默认值兼容）。

## 3. 2.2 修复的兼容性问题（行为不变，仅回退收紧）

| 问题 | 结果 |
|---|---|
| `IEventRequest` 误加 serde supertrait | 已移除，bound 下放到 `EventMessage::from_event`/`IMediator::publish` |
| `JwtAuth` 失去 `UnwindSafe`/`RefUnwindSafe` | 恢复显式 impl |
| `init_jwt_secret`/`jwt_secret` 误标弃用 | 已撤回属性 |
| 内部 crate caret 依赖导致混版本 | 改为精确版本钉 + CI 一致性检查 |

## 4. 可选采用（全部加性）

| 能力 | 你得到什么 | 需要做什么 |
|---|---|---|
| **WebSocket / 实时（2.3）** | `use_websocket`/`use_websocket_request`、连接作用域、`IConnectionRegistry`、跨节点扇出（`RealtimeFanoutEvent`）、背压/连接级限流、`RealtimeMetrics` | 见 [websocket 文档](../05-request-pattern/websocket.md) |
| **IM 参考应用（2.3-7）** | 可运行端到端 IM：登录/房间/消息/重连补拉/presence | `cargo run -p imbit`，见 [im-reference](../../design/im-reference.md) |
| **信令/TURN（2.4-1）** | `SignalingMessage`、coturn REST 凭据（`Turn` 配置节，启动期校验）、1:1/群组信令示例 | `TurnRestConfig` + WS `signal`，见 [media-contract](../../design/media-contract.md) |
| **对象存储/录制（2.4-2）** | `IObjectStore`（内存/文件系统实现，断点续传）、`RecordingMeta` 播放索引 | 注册实现，替换 `SystemTime::now()` 记账为 `IClock` |
| **媒体面适配（2.4-3）** | `IMediaPlaneAdapter` 契约 + `media_*` QoS 指标 | 实现端口，复用契约测试 |
| **中间件 `Next`（2.4-4）** | 超时/重试/熔断式包裹（可选，默认旧语义） | 见 [middleware-next](../../design/middleware-next.md) |
| **TestHost/时钟（2.4-5）** | 零端口测试、可控时钟、确定性 chaos | 见 [test-host](../../design/test-host.md) |
| **LTS/兼容政策（2.4-6）** | 弃用窗口 ≥2 minor、LTS 分支规则、稳定面清单 | 无需动作 |

## 5. 不做的事（2.x 内的承诺）

- 不删除/改名公开项、不收紧泛型 bound、不移除 auto trait；
- 不给既有函数加 `#[deprecated]`（3.0 窗口再引入，见
  [弃用登记](../../design/deprecations.md)）；
- 不改变 Error 2.0 状态码/`ProblemDetails`/WS 帧协议/指标名等稳定面行为；
- 补丁版本 `x.y.Z` 不发新功能、不改默认值。

## 6. 回滚

2.x 加性演进下回滚依赖版本即可；若已用新能力（WS/存储/媒体端口），回滚前移除对应
端点/实现即可，无数据格式变更（除你自己选择的存储实现）。
