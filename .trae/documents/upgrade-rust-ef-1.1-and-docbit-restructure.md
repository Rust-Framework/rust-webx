# rust-ef 1.1.0 升级 + docbit 4-crate 重构方案

> **状态**：待执行
> **目标**：将 docbit 从 rust-ef 0.3.5 升级到 1.1.0 GA，按 REF 最新规范重构为 4-crate 架构，博客改用数据库存储，新增 10 张表，本地 SQLite / 线上 MySQL 双 Provider。

***

## 一、Summary 概述

本方案对 `docbit` 应用进行三件事：

1. **依赖升级**：`rust-ef` 0.3.5 → 1.1.0，`rust-ef-sqlite` 0.3.5 → 1.1.0，新增 `rust-ef-mysql` 1.1.0；错误类型从 `LrefResult`/`LrefError` 迁移到 `EFResult`/`EFError`。
2. **架构重构**：将单一 `docbit` crate 拆分为 4 个子 crate（contracts / domain / handlers / host），移除 `common` 模块，职责严格分层。
3. **数据库化**：博客从文件系统存储迁移到数据库；按用户需求设计 10 张表（Blog / Comment / Category / User / Role / RoleUser / Resource / Authorize / tracking / Exhibition）；接入框架动态鉴权方案；本地 SQLite 开发，线上 MySQL 发布。

***

## 二、Current State Analysis 现状分析

### 2.1 当前依赖与版本

* 根 [Cargo.toml](file:///e:/GitCode/RF/rust-webapp/Cargo.toml#L4-L12) workspace members 包含 `"docbit"`（单一 crate）

* [docbit/Cargo.toml](file:///e:/GitCode/RF/rust-webapp/docbit/Cargo.toml#L10-L11) 依赖 `rust-ef = "0.3.5"` + `rust-ef-sqlite = "0.3.5"`

* workspace 依赖 `rust-dicore = "0.3.2"`

### 2.2 当前架构（待废弃）

```
docbit/
├── Cargo.toml              # 单一 crate
├── appsettings.json
├── appsettings.Development.json
├── wwwroot/                # 前端静态资源（保留迁移）
└── src/
    ├── main.rs             # 22 行入口
    ├── startup.rs          # DbInitService（IHostedService，跑 m001-m004 迁移）
    ├── common/             # escape_sql + RoleAuthorizer + AuditInterceptor + AppPaths
    │   ├── bootstrap.rs
    │   ├── mod.rs
    │   └── paths.rs
    ├── contracts/          # 8 个文件：auth/blog/cache/docs/site/user/work
    ├── domain/             # 2 实体 + 4 迁移（m001-m004）
    │   ├── user.rs         # UserEntity（id: String PK，role: String）
    │   ├── comment.rs      # BlogCommentEntity（扁平结构，无 parent_id/quoted_id）
    │   └── migrations/
    └── handlers/           # auth/blog/blog_service/comment/doc_service/docs/site/user/work
```

### 2.3 当前实现的痛点（必须解决）

1. **错误类型过时**：`common/mod.rs` 使用 `LrefResult`/`LrefError`，1.1.0 已重命名为 `EFResult`/`EFError`
2. **原始 SQL 泛滥**：[handlers/auth.rs](file:///e:/GitCode/RF/rust-webapp/docbit/src/handlers/auth.rs#L112-L120) 与 [handlers/user.rs](file:///e:/GitCode/RF/rust-webapp/docbit/src/handlers/user.rs#L95-L109) 用 `escape_sql()` + 字符串拼接 INSERT/UPDATE/DELETE，违背 1.1.0 的 `set::<T>().add()` / `update()` / `save_changes()` Unit of Work 模式
3. **博客文件存储**：[handlers/blog\_service.rs](file:///e:/GitCode/RF/rust-webapp/docbit/src/handlers/blog_service.rs#L40-L53) 用 `blog-data/{user_id}/INDEX.json` + markdown 文件，需改为数据库
4. **User 表设计陈旧**：`id: String`（hex 时间戳）、`role: String`（单角色字符串），需改为 `id: i32` 自增 + 多角色 RBAC
5. **Comment 表无引用/回复**：[domain/comment.rs](file:///e:/GitCode/RF/rust-webapp/docbit/src/domain/comment.rs#L6-L18) 无 `parent_id` / `quoted_id`，需双自外键
6. **鉴权粗放**：[common/mod.rs](file:///e:/GitCode/RF/rust-webapp/docbit/src/common/mod.rs#L24-L49) `RoleAuthorizer` 仅判断 admin，需接入 Resource + Authorize 动态鉴权
7. **m001-m004 迁移冗余**：将废弃，改用 `ensure_created()` + `has_data()` 种子

### 2.4 保留项（不动）

* `wwwroot/` 全部前端静态资源

* `appsettings.json` 的 `App` / `Jwt` / `Cors` 配置结构

* `rust-webapp` 框架的 `#[handler(inject)]` / `#[get]` / `#[post]` / `#[authorize]` / `IRequestHandler` / `IHostedService` / `Host::builder()` 模式

* `rust-dicore` 的 `#[rust_dicore::inject_attr]` 自动注册模式

***

## 三、Proposed Changes 拟议变更

### 3.1 目标架构（4-crate 分层）

```
docbit/
├── contracts/   # 契约层（最内层）：DTO + IRequest 请求类型 + 服务 trait，不依赖 rust-ef
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── auth.rs          # RegisterRequest/LoginRequest/AuthResponse/UserView
│       ├── user.rs          # UserModel + 6 个请求（移除 password_hash/role 字段，加 roles: Vec<String>）
│       ├── blog.rs          # BlogPostModel/Summary/CommentModel/CategoryDef/CategoryCount + IBlogService trait + 请求
│       ├── comment.rs       # CommentModel（含 parent_id/quoted_id）+ 请求
│       ├── category.rs      # CategoryModel（含 parent_id/level）+ 请求
│       ├── exhibition.rs    # ExhibitionModel + 请求（替换 work.rs）
│       ├── docs.rs          # DocIndex/DocContent/IDocumentService trait + 请求
│       ├── rbac.rs          # RoleModel/ResourceModel/AuthorizeModel + 请求
│       ├── tracking.rs      # TrackingModel + 请求
│       ├── site.rs          # SiteConfig/SiteLinks/SiteStats/SiteFooter + SiteInfoRequest
│       └── cache.rs         # CacheStatsRequest
│
├── domain/      # 领域层：实体定义 + 与 contracts 的转换 + 种子
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── entities/
│       │   ├── mod.rs
│       │   ├── user.rs          # UserEntity（id: i32 自增，无 role 字段）
│       │   ├── role.rs          # RoleEntity + RoleUserEntity
│       │   ├── resource.rs      # ResourceEntity + AuthorizeEntity
│       │   ├── category.rs      # CategoryEntity（parent_id 自外键）
│       │   ├── blog.rs          # BlogEntity（author_id FK, category_id FK）
│       │   ├── comment.rs       # CommentEntity（parent_id + quoted_id 双自外键）
│       │   ├── exhibition.rs    # ExhibitionEntity（category_id FK）
│       │   └── tracking.rs      # TrackingEntity
│       ├── conversions.rs       # Entity → Model 的 From 实现
│       └── seed.rs              # has_data 种子数据（admin 用户 + 默认角色 + 默认资源）
│
├── handlers/    # 处理层：服务实现 + HTTP Handler
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── auth/                # service.rs + handler.rs（注册/登录/me/忘记密码/重置密码）
│       ├── blog/                # service.rs + handler.rs（DB-backed IBlogService）
│       ├── comment/             # service.rs + handler.rs
│       ├── category/            # service.rs + handler.rs
│       ├── exhibition/          # service.rs + handler.rs（替换 work/）
│       ├── user/                # service.rs + handler.rs（6 个 CRUD）
│       ├── rbac/                # service.rs + handler.rs（角色/资源/授权 CRUD）
│       ├── tracking/            # service.rs + handler.rs（中间件 + 统计查询）
│       ├── docs/                # service.rs + handler.rs（保留文件系统文档读取）
│       ├── site/                # handler.rs
│       └── cache/               # handler.rs
│
└── host/        # 宿主层（组合根）：配置 + 路径 + 引导 + 拦截器 + 鉴权 + 中间件 + 启动 + 入口
    ├── Cargo.toml
    └── src/
        ├── main.rs              # Host::builder() 入口
        ├── config.rs            # AppSettings 读取 appsettings.{Env}.json + Database 配置
        ├── paths.rs             # AppPaths（wwwroot/db_path/docs_root，移除 blog_root）
        ├── bootstrap.rs         # configure(svc)：注册 AppPaths + Mutex<DbContext> + 选择 Provider
        ├── interceptor.rs       # AuditInterceptor（ISaveChangesInterceptor，错误类型改 EFResult/EFError）
        ├── authorizer.rs        # DynamicAuthorizer（查 Resource + Authorize 表，缓存 AuthzMatrix）
        ├── middleware.rs        # TrackingMiddleware（记录访问到 tracking 表）
        └── startup.rs           # DbInitService（ensure_created + has_data 种子 + docs.ensure_all_indexes）
```

**依赖方向（严格分层）**：

```
contracts  ──→  rust-webapp, serde
    ▲
domain     ──→  contracts, rust-ef
    ▲
handlers   ──→  contracts, domain, rust-ef, rust-webapp, rust-dicore
    ▲
host       ──→  contracts, domain, handlers, rust-ef, rust-ef-sqlite, rust-ef-mysql, rust-webapp
```

### 3.2 10 张表设计（i32 自增主键，无冗余命名）

#### 3.2.1 users 表

| 列名             | 类型     | 约束                                             | 说明        |
| -------------- | ------ | ---------------------------------------------- | --------- |
| id             | i32    | `#[primary_key]` `#[auto_increment]`           | 主键        |
| name           | String | `#[required]` `#[max_length(100)]`             | 昵称        |
| email          | String | `#[required]` `#[max_length(200)]` `#[unique]` | 邮箱（唯一）    |
| password\_hash | String | `#[required]` `#[max_length(200)]`             | bcrypt 哈希 |
| created\_at    | i64    | `#[required]`                                  | Unix 时间戳  |

> **变更**：移除 `role` 字段（改为通过 RoleUser 关联表实现多角色）；`id` 从 String 改为 i32 自增；`created_at` 从 String 改为 i64。

#### 3.2.2 roles 表

| 列名          | 类型     | 约束                                            | 说明                    |
| ----------- | ------ | --------------------------------------------- | --------------------- |
| id          | i32    | `#[primary_key]` `#[auto_increment]`          | 主键                    |
| name        | String | `#[required]` `#[max_length(50)]` `#[unique]` | 角色名（admin/user/guest） |
| description | String | `#[max_length(200)]`                          | 描述                    |

#### 3.2.3 role\_users 表（角色分配）

| 列名       | 类型  | 约束                                              | 说明    |
| -------- | --- | ----------------------------------------------- | ----- |
| id       | i32 | `#[primary_key]` `#[auto_increment]`            | 主键    |
| user\_id | i32 | `#[required]` `#[foreign_key(User)]` `#[index]` | 用户 FK |
| role\_id | i32 | `#[required]` `#[foreign_key(Role)]` `#[index]` | 角色 FK |

> 同时在 `User` 实体加 `#[navigation] pub roles: HasMany<Role, RoleUser>`，在 `Role` 实体加 `#[navigation] pub users: HasMany<User, RoleUser>`，实现多对多。

#### 3.2.4 categories 表（层级分类）

| 列名          | 类型          | 约束                                             | 说明                 |
| ----------- | ----------- | ---------------------------------------------- | ------------------ |
| id          | i32         | `#[primary_key]` `#[auto_increment]`           | 主键                 |
| name        | String      | `#[required]` `#[max_length(100)]`             | 分类名                |
| slug        | String      | `#[required]` `#[max_length(100)]` `#[unique]` | URL 友好标识           |
| parent\_id  | Option<i32> | `#[foreign_key(Category)]`                     | 父分类 FK（自外键，可空表根分类） |
| sort\_order | i32         | `#[required]`                                  | 排序                 |
| created\_at | i64         | `#[required]`                                  | 创建时间               |

> 实体加 `#[navigation] pub parent: BelongsTo<Category>` 和 `#[navigation] pub children: HasMany<Category>`。

#### 3.2.5 blogs 表（博客）

| 列名            | 类型     | 约束                                                  | 说明                             |
| ------------- | ------ | --------------------------------------------------- | ------------------------------ |
| id            | i32    | `#[primary_key]` `#[auto_increment]`                | 主键                             |
| slug          | String | `#[required]` `#[max_length(200)]` `#[unique]`      | URL 友好标识                       |
| title         | String | `#[required]` `#[max_length(200)]`                  | 标题                             |
| summary       | String | `#[max_length(500)]`                                | 摘要                             |
| content       | String | `#[required]`                                       | Markdown 正文                    |
| tags          | String | `#[required]`                                       | JSON 数组字符串（如 `["rust","web"]`） |
| category\_id  | i32    | `#[required]` `#[foreign_key(Category)]` `#[index]` | 分类 FK                          |
| author\_id    | i32    | `#[required]` `#[foreign_key(User)]` `#[index]`     | 作者 FK                          |
| published\_at | i64    | `#[required]`                                       | 发布时间戳                          |
| created\_at   | i64    | `#[required]`                                       | 创建时间戳                          |
| updated\_at   | i64    | `#[required]`                                       | 更新时间戳                          |

> 实体加 `#[navigation] pub category: BelongsTo<Category>` 和 `#[navigation] pub author: BelongsTo<User>` 和 `#[navigation] pub comments: HasMany<Comment>`。
> `tags` 用 String 存 JSON，读写时 `serde_json::to_string` / `from_str`。

#### 3.2.6 comments 表（评论，双自外键）

| 列名          | 类型          | 约束                                              | 说明               |
| ----------- | ----------- | ----------------------------------------------- | ---------------- |
| id          | i32         | `#[primary_key]` `#[auto_increment]`            | 主键               |
| blog\_id    | i32         | `#[required]` `#[foreign_key(Blog)]` `#[index]` | 博客 FK            |
| user\_id    | i32         | `#[required]` `#[foreign_key(User)]` `#[index]` | 评论者 FK           |
| user\_name  | String      | `#[required]` `#[max_length(100)]`              | 评论者昵称冗余（避免 JOIN） |
| content     | String      | `#[required]`                                   | 评论正文             |
| parent\_id  | Option<i32> | `#[foreign_key(Comment)]`                       | 回复目标评论 FK（直接回复）  |
| quoted\_id  | Option<i32> | `#[foreign_key(Comment)]`                       | 引用评论 FK（引用某条）    |
| created\_at | i64         | `#[required]`                                   | 创建时间戳            |

> **双外键设计**：`parent_id` 用于"回复 @某人"的层级结构；`quoted_id` 用于"引用某条评论"的块引用。两者均可空，可同时存在（既回复又引用）。
> 实体加 `#[navigation] pub blog: BelongsTo<Blog>`、`#[navigation] pub user: BelongsTo<User>`、`#[navigation] pub parent: BelongsTo<Comment>`、`#[navigation] pub quoted: BelongsTo<Comment>`。

#### 3.2.7 exhibitions 表（作品展，存储 INDEX.json 数据）

| 列名           | 类型             | 约束                                                  | 说明                 |
| ------------ | -------------- | --------------------------------------------------- | ------------------ |
| id           | i32            | `#[primary_key]` `#[auto_increment]`                | 主键                 |
| slug         | String         | `#[required]` `#[max_length(100)]` `#[unique]`      | 标识                 |
| title        | String         | `#[required]` `#[max_length(200)]`                  | 标题                 |
| subtitle     | String         | `#[max_length(200)]`                                | 副标题                |
| description  | String         | `#[required]`                                       | 描述                 |
| category\_id | i32            | `#[required]` `#[foreign_key(Category)]` `#[index]` | 分类 FK（复用 Category） |
| tags         | String         | `#[required]`                                       | JSON 数组字符串         |
| repo\_url    | Option<String> | `#[max_length(500)]`                                | 仓库 URL             |
| demo\_url    | Option<String> | `#[max_length(500)]`                                | 演示 URL             |
| docs\_slug   | Option<String> | `#[max_length(100)]`                                | 关联文档 slug          |
| featured     | bool           | `#[required]`                                       | 是否推荐               |
| sort\_order  | i32            | `#[required]`                                       | 排序                 |
| logo\_url    | Option<String> | `#[max_length(500)]`                                | Logo URL           |
| created\_at  | i64            | `#[required]`                                       | 创建时间戳              |
| updated\_at  | i64            | `#[required]`                                       | 更新时间戳              |

> 实体加 `#[navigation] pub category: BelongsTo<Category>`。
> 此表存储原 `docs/rust-ef/INDEX.json` 等作品集元数据，便于检索。

#### 3.2.8 resources 表（权限资源）

| 列名             | 类型     | 约束                                             | 说明                                        |
| -------------- | ------ | ---------------------------------------------- | ----------------------------------------- |
| id             | i32    | `#[primary_key]` `#[auto_increment]`           | 主键                                        |
| route\_pattern | String | `#[required]` `#[max_length(200)]` `#[unique]` | 路由模式（如 `/api/blog/{slug}`、`/api/users/*`） |
| method         | String | `#[required]` `#[max_length(10)]`              | HTTP 方法（GET/POST/PUT/DELETE/\*）           |
| description    | String | `#[max_length(200)]`                           | 资源描述                                      |
| created\_at    | i64    | `#[required]`                                  | 创建时间戳                                     |

#### 3.2.9 authorizes 表（授权，Role ↔ Resource 多对多）

| 列名           | 类型  | 约束                                                  | 说明    |
| ------------ | --- | --------------------------------------------------- | ----- |
| id           | i32 | `#[primary_key]` `#[auto_increment]`                | 主键    |
| role\_id     | i32 | `#[required]` `#[foreign_key(Role)]` `#[index]`     | 角色 FK |
| resource\_id | i32 | `#[required]` `#[foreign_key(Resource)]` `#[index]` | 资源 FK |

> 实体加导航：`Role` 加 `#[navigation] pub resources: HasMany<Resource, Authorize>`，`Resource` 加 `#[navigation] pub roles: HasMany<Role, Authorize>`。

#### 3.2.10 tracking 表（站点跟踪）

| 列名     | 类型     | 约束                                            | 说明      |
| ------ | ------ | --------------------------------------------- | ------- |
| id     | i32    | `#[primary_key]` `#[auto_increment]`          | 主键      |
| path   | String | `#[required]` `#[max_length(500)]` `#[index]` | 访问路径    |
| method | String | `#[required]` `#[max_length(10)]`             | HTTP 方法 |

