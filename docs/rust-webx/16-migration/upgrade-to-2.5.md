# 升级到 2.5（2.x 加性演进）

> 2.5.0 是 2.x 的加性发布：只使用 2.1 API 的应用升级依赖即可，**不需要改代码**
> （发布时按 N-2 = 2.1 的冒烟门禁覆盖）；新能力全部 opt-in。本页是历史升级路径；
> 当前发布 2.9.3 的升级摘要见[第十六章 INDEX](INDEX.md)。

## 1. 一句话结论

- 已有 2.1 应用：升级依赖版本即可；
- 想用新能力：按需采用下方“可选采用”；
- 唯一的公开结构体字段新增（`engine::Pipeline`）命中
  [`constructible_struct_adds_field = "allow"`](../../design/compatibility.md)，
  用 `Pipeline::new(stages)` 构造不受影响。

## 2. 升级步骤

```toml
# 保持锁步：umbrella 与内部 crate 同版本
rust-webx = "=2.5.0"
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

## 3. 兼容性保障

- 2.1 的公开路由/绑定/错误/身份/分页/中间件语义保持不变；
- 内部 crate 精确钉版本，CI 校验版本一致；
- 事件 DTO 需要 `Serialize`/`Deserialize`，事件处理器使用 `#[handler]`；
- `init_jwt_secret` / `jwt_secret()` 在 2.5 是文档标注的兼容 shim，**已在 2.9.1 删除**（无别名）：
  新代码注入 `Arc<dyn ITokenIssuer>`，密钥来自 `Jwt.SecretFile`（env > `Jwt.Secret` > 文件），
  见 [upgrade-to-2.9.1](upgrade-to-2.9.1.md) §5.1。

## 4. 可选采用（全部加性）

| 能力 | 你得到什么 | 需要做什么 |
|---|---|---|
| **WebSocket / 实时** | `use_websocket`/`use_websocket_request`、连接作用域、`IConnectionRegistry`、跨节点扇出（`RealtimeFanoutEvent`）、背压/连接级限流、`RealtimeMetrics` | 见 [websocket 文档](../05-request-pattern/websocket.md) |
| **IM 参考应用** | 可运行端到端 IM：登录/房间/消息/重连补拉/presence | `cargo run --manifest-path examples/imbit/Cargo.toml -p imbit-host`，见 [im-reference](../../design/im-reference.md) |
| **信令/TURN** | `SignalingMessage`、coturn REST 凭据（`Turn` 配置节，启动期校验）、1:1/群组信令示例 | `TurnRestConfig` + WS `signal`，见 [media-contract](../../design/media-contract.md) |
| **对象存储/录制** | `IObjectStore`（内存/文件系统实现，断点续传）、`RecordingMeta` 播放索引 | 注册实现；用 `IClock` 代替直接读墙钟 |
| **媒体面适配** | `IMediaPlaneAdapter` 契约 + `media_*` QoS 指标 | 实现端口，复用契约测试 |
| **中间件 `Next`** | 超时/重试/熔断式包裹（可选，默认旧语义） | 见 [middleware-next](../../design/middleware-next.md) |
| **TestHost/时钟** | 零端口测试、可控时钟、确定性 chaos | 见 [test-host](../../design/test-host.md) |
| **认证** | JWT/JWKS、Cookie+CSRF、刷新令牌、OIDC/OAuth2、MFA/WebAuthn、mTLS、DPoP | 见 [auth-migration-2.5](../../design/auth-migration-2.5.md) |
| **消息/缓存适配器** | Redis/NATS/RabbitMQ/Kafka `add_*`、Redis `IDistributedCache` | 见 [端口与适配器归属](../../design/integrations.md) |
| **LTS/兼容政策** | 弃用窗口 ≥2 minor、LTS 分支规则、稳定面清单 | 无需动作 |

## 5. 不做的事（2.x 内的承诺）

- 不删除/改名公开项、不收紧泛型 bound、不移除 auto trait；
- 不给既有函数新增 `#[deprecated]`（下一破坏性版本窗口再引入，见
  [弃用登记](../../design/deprecations.md)）；
- 不改变 Error 状态码/`ProblemDetails`/WS 帧协议/指标名等稳定面行为；
- 补丁版本 `x.y.Z` 原则上不发新功能、不改默认值。维护者已登记的例外：**2.9.1 破坏性补丁**
  （整包契约迁移，见 [upgrade-to-2.9.1](upgrade-to-2.9.1.md)）、**2.9.2 加性补丁**（只新增
  opt-in 能力，不改默认行为）、**2.9.3 行为变更补丁**（`start_id` 默认、保留扇出事件
  fail-closed、presence 身份/布局；摘要见[第十六章 INDEX](INDEX.md)）。本条其余部分描述
  2.5 时代的历史约定，不回改。

## 6. 回滚

2.x 加性演进下回滚依赖版本即可；若已用新能力（WS/存储/媒体端口），回滚前移除对应
端点/实现即可，无数据格式变更（除你自己选择的存储实现）。
