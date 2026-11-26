# rust-webx 2.5.0 发布说明

> **历史文档（2026-11 存档）**：本文记录 2.5.0 发布时的状态，不是当前契约。2.9.1 已删除
> `init_jwt_secret` / `jwt_secret()` 兼容层并完成契约迁移；当前版本、兼容政策与边界以
> `CHANGELOG.md`、[compatibility.md](compatibility.md) 与
> [known-boundaries.md](known-boundaries.md) 为准。

> 发布日期：2026-11-21。2.5.0 是 2.x 线上的**加性发布**：在 2.1 的公开 API 之上
> 增加运行信任、实时执行模型、认证体系、消息/缓存适配器与 AI 网关能力。
> 只使用 2.1 API 的应用升级依赖即可，无需改代码（N-2 门禁覆盖）。

## 1. 亮点

| 领域 | 交付 |
|---|---|
| 运行信任 | 严格配置绑定（未知键/类型错误启动失败）、默认 30s 入站 deadline、准入超时 503 + `Retry-After`、按路由 RED 指标、RFC 9457 `code`/`retryable`/`traceId`、分片限流端口 |
| 实时 | WebSocket 升级、连接级作用域、消息即请求、连接注册表/Hub、跨节点 fan-out（复用 `IEventPublisher`）、每连接背压与限流、realtime 指标 |
| 事件 | `IMediator::publish` 异步 fire-and-forget、有界内存传输、`#[handler]` 事件处理器自动注册、死信与重投上限 |
| 认证 | JWT 校验硬化、Cookie+CSRF、登录硬化、刷新令牌轮换、JWKS/非对称、OIDC、OAuth2 社交登录、TOTP、WebAuthn、mTLS、DPoP、introspection/revocation |
| 适配器 | Redis（缓存 + Streams 事件）、NATS、RabbitMQ（quorum/DLX）、Kafka（纯 Rust follower） |
| AI | `rust-webx-ai` 领域包、EF 用量账本、OpenAI 兼容 `/v1` 表面、配额/韧性/观测 |
| 测试基座 | `TestHost` 进程内分发、`IClock`/`TestClock`、chaos 钩子、契约测试 crate |
| 工具链 | 分配预算、soak/chaos/fuzz-lite/wire p99、三平台 CI、crate 归属与发布顺序门禁 |

## 2. 新增 crate

`rust-webx-auth-ef`、`rust-webx-oidc`、`rust-webx-oauth2`、`rust-webx-mfa`、
`rust-webx-webauthn`、`rust-webx-redis`、`rust-webx-nats`、`rust-webx-rabbitmq`、
`rust-webx-kafka`。

所有适配器均为 opt-in：`core`/`host`/umbrella 不依赖任何 broker/缓存/AI/认证协议 crate。

## 3. 兼容性

- 旧 `Jwt.Secret` + HS256 配置、无 `jti` 的旧 token 继续可用；
- `init_jwt_secret` / `jwt_secret()` 仍可用但已 deprecated，下一个 major 删除（**历史记录：
  2.9.1 已随兼容层一并删除，未等到下一个 major**，见 [compatibility.md](compatibility.md) §1）；
- bcrypt 哈希不直接被 Argon2 hasher 接受：在下次成功校验后重哈希，或保留自定义 legacy hasher；
- 事件发布改为异步：事件 DTO 需要 `Serialize`/`Deserialize`，事件处理器需要 `#[handler]` 或显式 DI；
- 详情见 `auth-migration-2.5.md` 与 `compatibility.md`。

## 4. 验证

- 全 workspace `fmt` / `clippy -D warnings` / `test` 绿（三平台 CI）；
- HS256 p50 **2.3µs** / p99 **3.9µs**（release，20k）；
- 认证层增量分配 **0**；
- 框架税 **10.42 allocs/request**（hyper 地板 6.00，CI 上限 12）；
- `cargo audit` 0 漏洞、0 拒绝告警；
- 30min / 1h Linux soak 0 失败（RSS 稳定 ~17.7MB）；chaos 3/3；fuzz-lite 零 panic；
- 认证一致性矩阵见 `auth-conformance.md`。

## 5. 已知边界

- 外部 IdP：Duende（授权码+PKCE）、GitHub OAuth2、Google/Microsoft discovery/JWKS 已联网验证，
  其他厂商需 client 注册后复测；
- WebAuthn 仅 ES256 + `none` attestation；物理认证器与更多 attestation 未纳入门禁；
- Kafka 集成 crate 为单分区 follower，不做 consumer-group offset 提交，仅明文连接；
- DPoP 对象型 `cnf` 需自定义 issuer；mTLS 仅 WebPKI 链。

## 6. 升级步骤

1. 依赖改为 `rust-webx = "2.5.0"`（子 crate 同版本，精确钉 `=2.5.0`）；
2. 如需认证/适配器，按需引入对应 opt-in crate 并调用 `add_*`；
3. 运行 `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`；
4. 生产环境检查 `APP_ENV`、CORS 白名单、`Jwt.Secret`/`Jwt.SecretFile` 与健康探针。
