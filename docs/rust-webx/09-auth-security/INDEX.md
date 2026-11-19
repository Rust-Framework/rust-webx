# 第九章 认证与授权

## 本章小节

| 小节 | 内容 |
|------|------|
| [JWT Bearer 认证](jwt-authentication.md) | Token 认证流程 |
| [API Key 认证](api-key-auth.md) | 认证 scheme 链、sk- 校验与 JWT 共存 |
| [基于资源的授权](resource-authorization.md) | 自定义策略/旧式中间件 |
| [authorize 宏与声明式授权](authorize-macro.md) | 静态规则 + IDynamicAuthorizer 动态授权 |
| [令牌签发、会话吊销与口令哈希](token-session-password.md) | ITokenIssuer / ISessionStore / IPasswordHasher 端口，DeclaredRules 与统一入口 |
| [密钥存储](secret-store.md) | ISecretStore / SecretValue：env、文件与链式解析 |
| [安全最佳实践](security-best-practices.md) | 生产安全清单 |

## 下一步

从 [JWT Bearer 认证](jwt-authentication.md) 开始。
