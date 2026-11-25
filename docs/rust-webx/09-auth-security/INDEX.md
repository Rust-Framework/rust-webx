# 第九章 认证与授权

## 本章小节

| 小节 | 内容 |
|------|------|
| [快速上手](quickstart.md) | 10 分钟：Host + 登录 + 受保护端点 |
| [JWT Bearer 认证](jwt-authentication.md) | Token 认证流程、iss/aud/严格模式 |
| [Cookie 会话与 CSRF](cookie-session.md) | BFF 不透明会话、ISessionRepository、双提交 CSRF |
| [OpenID Connect 登录](oidc.md) | discovery、PKCE、id_token 校验与会话桥 |
| [OAuth 2.0 社交登录](oauth2-social.md) | GitHub/Google/国内平台、PKCE、userinfo → IdentityUser |
| [TOTP 与恢复码](mfa.md) | RFC 6238、重放防护、恢复码、amr/acr step-up |
| [密钥轮换与应急 Runbook](key-rotation-runbook.md) | JWT/OIDC 密钥轮换、吊销、健康与 5 分钟演练 |
| [DPoP 发送方约束令牌](dpop.md) | RFC 9449 proof、cnf.jkt、重放防护 |
| [WebAuthn/Passkey](webauthn.md) | 注册/断言验签、RP ID/origin/challenge、签名计数器 |
| [Token Introspection/Revocation](introspection.md) | RFC 7662/7009、active=false 语义、JWT/refresh 吊销 |
| [mTLS 客户端证书](mtls.md) | TLS 客户端证书链、指纹身份映射、代理边界 |
| [API Key 认证](api-key-auth.md) | 认证 scheme 链、sk- 校验与 JWT 共存 |
| [资源授权与实例级归属](resource-authorization.md) | 路由级 `#[authorize]` / `IDynamicAuthorizer` 边界、Handler 内 owner/tenant 校验 |
| [authorize 宏与声明式授权](authorize-macro.md) | 静态规则 + IDynamicAuthorizer 动态授权 |
| [令牌签发、会话吊销与口令哈希](token-session-password.md) | ITokenIssuer / ISessionStore / IPasswordHasher 端口，authorize_with_declared_rules 与统一入口 |
| [密钥存储](secret-store.md) | ISecretStore / SecretValue：env、文件与链式解析 |
| [安全最佳实践](security-best-practices.md) | 生产安全清单 |

## 下一步

从 [JWT Bearer 认证](jwt-authentication.md) 开始。
