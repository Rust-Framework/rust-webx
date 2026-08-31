# 第四章 架构全景

本章从全局视角理解 rust-webx 的内部构造与请求流转。

## 本章小节

| 小节 | 内容 |
|------|------|
| [Crate 分层结构](crate-layout.md) | 各 Crate 职责与依赖方向 |
| [请求生命周期](request-lifecycle.md) | 从 TCP 到 JSON 响应的完整路径 |
| [分层模型与依赖方向](layering-model.md) | 应用内部分层与依赖规则 |
| [编译时扫描机制](compile-time-scan.md) | inventory 如何收集路由与 Handler |

## 学习目标

读完本章，你应能画出一张从 HTTP 请求到 Handler 返回的完整流程图，并说出每个环节的职责归属。

## 下一步

从 [Crate 分层结构](crate-layout.md) 开始。
