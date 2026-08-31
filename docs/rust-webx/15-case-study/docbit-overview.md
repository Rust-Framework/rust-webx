# Docbit 项目概览

## 是什么

Docbit 是基于 rust-webx 构建的**开发者作品集全栈站点**，展示框架的生产级能力：

- 作品集展示（Works）
- 技术博客（Blog）
- 框架文档系统（Docs）— 即本书
- 用户认证（Auth）
- SPA 前端（wwwroot）

## 功能模块

| 模块 | API 前缀 | 说明 |
|------|---------|------|
| Site | `/api/site` | 站点配置信息 |
| Auth | `/api/auth` | 注册、登录、获取当前用户 |
| Works | `/api/works` | 作品集 CRUD |
| Blog | `/api/blog` | 博客文章 |
| Docs | `/api/docs` | 文档索引与内容 API |
| Cache | `/api/cache` | 缓存演示 |

## 技术栈

| 层 | 技术 |
|----|------|
| 框架 | rust-webx |
| 数据库 | rust-ef + SQLite |
| 认证 | JWT + bcrypt |
| 前端 | 原生 HTML/CSS/JS（wwwroot） |
| 文档 | Markdown + INDEX.json |

## 默认账户

启动后自动种子数据：

- 邮箱：`admin@docbit.dev`
- 密码：`admin123`

## 文档系统

Docbit 的 `DocService` 扫描仓库根目录 `docs/`：

```
docs/
└── rust-webx/          ← 本书
    ├── FOREWORD.md
    ├── INDEX.md
    ├── INDEX.json
    └── 01-introduction/
        ├── INDEX.md
        └── *.md
```

API：
- `GET /api/docs` — 列出所有文档作品
- `GET /api/docs/rust-webx/index` — 获取目录树
- `GET /api/docs/rust-webx/content/{path}` — 获取 Markdown 内容

## 小结

Docbit 是一个真实的全栈产品，不是玩具示例。它的代码模式可直接复用到你的项目。

下一节：[架构与模块划分](docbit-architecture.md)
