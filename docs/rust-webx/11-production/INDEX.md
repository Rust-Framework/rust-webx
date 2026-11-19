# 第十一章 生产级能力

## 本章小节

| 小节 | 内容 |
|------|------|
| [CORS、TLS 与健康检查](cors-tls-health.md) | 网络层能力 |
| [缓存与速率限制](caching-rate-limit.md) | 性能与防护 |
| [配额与预扣-结算](quota.md) | 按 key/模型的请求、token、并发与预算配额 |
| [用量计量与账本](usage-metering.md) | 幂等 UsageEvent、tokenizer 兜底与窗口聚合 |
| [模型目录与路由](model-routing.md) | provider 端口、别名策略、健康与容量规划 |
| [韧性：重试、熔断与舱壁](resilience.md) | 退避/jitter、熔断状态机、provider 并发上限 |
| [指标（IMeter）](metrics.md) | 请求/TTFT/token/上游错误/配额/active streams |
| [目录热替换](hot-config.md) | `CatalogHandle` + `ArcSwap` 无锁快照替换 |
| [配置热更新](live-options.md) | `IOptionsMonitor`、文件轮询与变更通知 |
| [精确响应缓存](response-cache.md) | 确定性非流请求的租户隔离缓存 |
| [OpenAI 兼容端点](openai-compatible-endpoint.md) | `/v1/chat/completions` 的 JSON/SSE 与错误信封 |
| [OpenAPI 与 SPA 托管](openapi-spa.md) | 文档、前端与内嵌文件读取 |
| [文件服务的生产部署](file-serving.md) | 缓存、断点续传、反代与 fd |
| [优雅关闭与可观测性](graceful-shutdown.md) | 运维能力 |

## 下一步

从 [CORS、TLS 与健康检查](cors-tls-health.md) 开始。
