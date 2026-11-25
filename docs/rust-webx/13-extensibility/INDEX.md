# 第十三章 扩展与自定义封装

## 本章小节

| 小节 | 内容 |
|------|------|
| [自定义组件封装](custom-components.md) | 可复用中间件与服务 |
| [自定义 Endpoint](custom-endpoints.md) | IEndpoint 扩展 |
| [样式与约定封装](style-patterns.md) | 项目级编码规范 |
| [第三方库集成](third-party-integration.md) | 数据库与外部服务 |
| [端口与适配器（下游接入）](integrations.md) | 端口清单、默认实现、复制参考适配器、契约测试与 outbox 边界 |
| [出站 HTTP（rust-webx-client）](outbound-http.md) | 命名客户端、连接池、流式响应与 SSE 解码 |
| [错误响应写入器](error-writers.md) | 按路由接管错误信封（problem+json / OpenAI） |
| [Mock 上游](mock-upstream.md) | 脚本化 HTTP/SSE 上游与请求录制 |
| [支付](payments.md) | 统一支付端口、运行时配置/多账户、回调入口与 inbox |
| [事务性 outbox](outbox.md) | 同事务写事件行、租约派发、重试与死信 |

## 下一步

从 [自定义组件封装](custom-components.md) 开始。
