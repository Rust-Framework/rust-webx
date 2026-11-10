# 第五章 请求即端点

本章深入 rust-webx 的核心开发模式。

## 本章小节

| 小节 | 内容 |
|------|------|
| [IRequest 与 IRequestHandler](irequest-irequesthandler.md) | 双类型参数模式 |
| [路由宏详解](route-macros.md) | get/post/put/delete、端点名称/分类、端点目录 |
| [Handler 注册策略](handler-registration.md) | handler 宏、手动注册、inject |
| [参数绑定与序列化](parameter-binding.md) | 路径、Query、Body、表单、Header、Claim 的来源声明与错误聚合 |
| [用户身份与多租户](user-identity.md) | 用 `IdentityUser` 直接携带身份：登录、匿名、角色、租户隔离 |
| [文件上传与下载](file-upload-download.md) | multipart 表单、FormFile、文件响应与 Range |
| [错误处理与 ProblemDetails](error-handling.md) | Error 变体与 RFC 7807 |

## 下一步

从 [IRequest 与 IRequestHandler](irequest-irequesthandler.md) 开始。
