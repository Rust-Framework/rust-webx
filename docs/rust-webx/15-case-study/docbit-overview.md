# Docbit 项目概览

## 是什么

Docbit 是基于 rust-webx 构建的**开发者作品集全栈站点**，展示框架的生产级能力：

- 作品集展示（Exhibitions）
- 技术博客（Blog）
- 框架文档系统（Docs）— 即本书
- 用户认证（Auth）
- SPA 前端（wwwroot）

## 功能模块

| 模块 | API 前缀 | 说明 |
|------|---------|------|
| Site | `/api/site` | 站点配置信息 |
| Auth | `/api/auth` | 注册、登录、当前用户、忘记/重置密码 |
| Exhibitions | `/api/exhibitions` | 作品集 CRUD |
| Docs | `/api/docs` | 文档索引与内容 API（`/api/docs/{work}/index`、`/content/{*path}`） |
| Works（上传） | `/api/works/{slug}/docs` | 上传某个作品的文档 bundle（admin） |
| Blog | `/api/blog` | 博客文章、分类计数、我的文章 |
| Categories | `/api/categories` | 层级分类 |
| Comments | `/api/comments` | 评论与回复 |
| Media | `/api/media` | 图片/附件上传与访问 |
| Users | `/api/users`、`/api/info` | 用户管理 |
| RBAC | `/api/roles`、`/api/resources`、`/api/authorizes`、`/api/role-users` | 角色、资源与授权 |
| Tracking | `/api/tracking` | 访问统计 |
| Cache | `/api/cache/stats` | 缓存演示 |

> 完整路由定义见 `docbit/contracts/src/*.rs`。

## 技术栈

| 层 | 技术 |
|----|------|
| 框架 | rust-webx |
| 数据库 | rust-ef + SQLite |
| 认证 | JWT + bcrypt |
| 前端 | 原生 HTML/CSS/JS（wwwroot） |
| 文档 | Markdown + INDEX.json |

## 默认账户

| 环境 | 行为 |
|------|------|
| Development | 自动创建 `admin@docbit.local` / `admin123`（启动日志警告） |
| Production | 不创建内置账号；须设置 `DOCBIT_ADMIN_PASSWORD` 才按该口令创建 |

生产不会回落到内置口令，也不会把运维口令写入日志。

## 文档系统

Docbit 的 `DocService` 按优先级解析每个作品（work）的文档目录：

1. `<app_base>/docs/{work}` — 发布/上传的 bundle（可写）
2. `<workspace>/docs/{work}` — rust-webx 手册镜像
3. monorepo sibling 实时路径（如 `rust-webx/docs/rust-webx`）

```
<app_base>/docs/          # 上传/发布目录（优先级最高）
└── rust-webx/
    ├── INDEX.json
    └── 01-introduction/
        ├── INDEX.md
        └── *.md
```

API：
- `GET /api/docs` — 列出所有文档作品
- `GET /api/docs/rust-webx/index` — 获取目录树
- `GET /api/docs/rust-webx/content/{*path}` — 获取 Markdown 内容

## 小结

Docbit 是一个真实的全栈产品，不是玩具示例。它的代码模式可直接复用到你的项目。

下一节：[架构与模块划分](docbit-architecture.md)
