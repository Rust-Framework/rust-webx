# 第三章 设计理念与哲学

本章回答「为什么这样设计」，帮助你从使用者成长为理解框架意图的开发者。

## 本章小节

| 小节 | 内容 |
|------|------|
| [核心设计原则](design-principles.md) | SOLID、高内聚低耦合在框架中的体现 |
| [ASP.NET Core 的启发](aspnet-inspiration.md) | 借鉴什么、Rust 化什么 |
| [Rust 惯用法与类型安全](rust-idioms.md) | 编译期保证与零成本抽象 |
| [渐进式披露与框架边界](progressive-disclosure.md) | 简单场景简单做，复杂场景可扩展 |

## 学习目标

理解设计哲学后，你将能**做出正确的架构决策**——何时用 `#[handler]`，何时手动注册，何时引入 Mediator 事件，而不仅是从模板复制代码。

## 下一步

从 [核心设计原则](design-principles.md) 开始。
