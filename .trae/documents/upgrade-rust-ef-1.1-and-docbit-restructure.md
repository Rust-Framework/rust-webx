# rust-ef 1.1.0 升级 + docbit 4-crate 重构方案

> **状态**：已更新（含 Resource 重新设计 + 审计字段/软删除规范 + rust-ef 1.1.0 API 修正）。步骤 1-2 已完成，步骤 3 domain crate 进行中。
> **目标**：将 docbit 从 rust-ef 0.3.5 升级到 1.1.0 GA，按 REF 最新规范重构为 4-crate 架构，博客改用数据库存储，新增 10 张表（含审计字段与软删除），本地 SQLite / 线上 MySQL 双 Provider。

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

### 3.2 审计字段与软删除约定（适用大部分主表）

为满足运维审计与数据安全需求，**主表**（users / roles / categories / blogs / comments / exhibitions / resources）统一追加以下 5 个审计 + 软删除字段；**联结表**（role\_users / authorizes）与**日志表**（tracking）不追加，保持轻量。

| 字段          | 类型          | 约束                                      | 说明                                  |
| ----------- | ----------- | --------------------------------------- | ----------------------------------- |
| created\_id | Option<i32> | `#[index]`（**不加** **`#[foreign_key]`**） | 创建人 user id；无 FK 约束，软删除用户后仍可保留创建者信息 |
| created\_at | i64         | `#[required]`                           | 创建时间戳                               |
| updated\_id | Option<i32> | `#[index]`（**不加** **`#[foreign_key]`**） | 更新人 user id                         |
| updated\_at | i64         | `#[required]`                           | 更新时间戳                               |
| is\_deleted | bool        | `#[required]` `#[index]`                | 软删除标记，默认 false                      |

> **设计决策**：
>
> 1. `created_id` / `updated_id` **不使用** **`#[foreign_key(User)]`** —— 避免与 User 表自引用产生重复 `FK_User` 常量（详见 3.4 节 rust-ef 限制），且软删除用户后审计记录仍需可读。
> 2. `is_deleted` 加 `#[index]`，所有列表查询须带 `where is_deleted = false` 过滤（handlers 层统一封装）。
> 3. 首条 users 记录（初始化 admin）的 `created_id` / `updated_id` 为 `None`。

### 3.3 10 张表设计（i32 自增主键，无冗余命名）

#### 3.3.1 users 表

| 列名             | 类型          | 约束                                             | 说明                   |
| -------------- | ----------- | ---------------------------------------------- | -------------------- |
| id             | i32         | `#[primary_key]` `#[auto_increment]`           | 主键                   |
| name           | String      | `#[required]` `#[max_length(100)]`             | 昵称                   |
| email          | String      | `#[required]` `#[max_length(200)]` `#[unique]` | 邮箱（唯一）               |
| password\_hash | String      | `#[required]` `#[max_length(200)]`             | bcrypt 哈希            |
| created\_id    | Option<i32> | `#[index]`（无 FK）                               | 创建人（首条 admin 为 None） |
| created\_at    | i64         | `#[required]`                                  | Unix 时间戳             |
| updated\_id    | Option<i32> | `#[index]`（无 FK）                               | 更新人                  |
| updated\_at    | i64         | `#[required]`                                  | 更新时间戳                |
| is\_deleted    | bool        | `#[required]` `#[index]`                       | 软删除标记                |

> **变更**：移除 `role` 字段（改为通过 RoleUser 关联表实现多角色）；`id` 从 String 改为 i32 自增；`created_at` 从 String 改为 i64；新增审计字段与软删除。

#### 3.3.2 roles 表

| 列名          | 类型          | 约束                                            | 说明                    |
| ----------- | ----------- | --------------------------------------------- | --------------------- |
| id          | i32         | `#[primary_key]` `#[auto_increment]`          | 主键                    |
| name        | String      | `#[required]` `#[max_length(50)]` `#[unique]` | 角色名（admin/user/guest） |
| description | String      | `#[max_length(200)]`                          | 描述                    |
| created\_id | Option<i32> | `#[index]`（无 FK）                              | 创建人                   |
| created\_at | i64         | `#[required]`                                 | 创建时间                  |
| updated\_id | Option<i32> | `#[index]`（无 FK）                              | 更新人                   |
| updated\_at | i64         | `#[required]`                                 | 更新时间                  |
| is\_deleted | bool        | `#[required]` `#[index]`                      | 软删除标记                 |

#### 3.3.3 role\_users 表（角色分配）

| 列名       | 类型  | 约束                                              | 说明    |
| -------- | --- | ----------------------------------------------- | ----- |
| id       | i32 | `#[primary_key]` `#[auto_increment]`            | 主键    |
| user\_id | i32 | `#[required]` `#[foreign_key(User)]` `#[index]` | 用户 FK |
| role\_id | i32 | `#[required]` `#[foreign_key(Role)]` `#[index]` | 角色 FK |

> 同时在 `User` 实体加 `#[navigation] pub roles: HasMany<Role, RoleUser>`，在 `Role` 实体加 `#[navigation] pub users: HasMany<User, RoleUser>`，实现多对多。

#### 3.3.4 categories 表（层级分类）

| 列名          | 类型          | 约束                                             | 说明                 |
| ----------- | ----------- | ---------------------------------------------- | ------------------ |
| id          | i32         | `#[primary_key]` `#[auto_increment]`           | 主键                 |
| name        | String      | `#[required]` `#[max_length(100)]`             | 分类名                |
| slug        | String      | `#[required]` `#[max_length(100)]` `#[unique]` | URL 友好标识           |
| parent\_id  | Option<i32> | `#[foreign_key(Category)]`                     | 父分类 FK（自外键，可空表根分类） |
| sort\_order | i32         | `#[required]`                                  | 排序                 |
| created\_id | Option<i32> | `#[index]`（无 FK）                               | 创建人                |
| created\_at | i64         | `#[required]`                                  | 创建时间               |
| updated\_id | Option<i32> | `#[index]`（无 FK）                               | 更新人                |
| updated\_at | i64         | `#[required]`                                  | 更新时间               |
| is\_deleted | bool        | `#[required]` `#[index]`                       | 软删除标记              |

> 实体加 `#[navigation] pub parent: BelongsTo<Category>` 和 `#[navigation] pub children: HasMany<Category>`。

#### 3.3.5 blogs 表（博客）

| 列名            | 类型          | 约束                                                  | 说明                             |
| ------------- | ----------- | --------------------------------------------------- | ------------------------------ |
| id            | i32         | `#[primary_key]` `#[auto_increment]`                | 主键                             |
| slug          | String      | `#[required]` `#[max_length(200)]` `#[unique]`      | URL 友好标识                       |
| title         | String      | `#[required]` `#[max_length(200)]`                  | 标题                             |
| summary       | String      | `#[max_length(500)]`                                | 摘要                             |
| content       | String      | `#[required]`                                       | Markdown 正文                    |
| tags          | String      | `#[required]`                                       | JSON 数组字符串（如 `["rust","web"]`） |
| category\_id  | i32         | `#[required]` `#[foreign_key(Category)]` `#[index]` | 分类 FK                          |
| author\_id    | i32         | `#[required]` `#[foreign_key(User)]` `#[index]`     | 作者 FK                          |
| published\_at | i64         | `#[required]`                                       | 发布时间戳                          |
| created\_at   | i64         | `#[required]`                                       | 创建时间戳                          |
| updated\_at   | i64         | `#[required]`                                       | 更新时间戳                          |
| created\_id   | Option<i32> | `#[index]`（无 FK）                                    | 创建人（author\_id 已存作者，此为运维审计创建人） |
| updated\_id   | Option<i32> | `#[index]`（无 FK）                                    | 更新人                            |
| is\_deleted   | bool        | `#[required]` `#[index]`                            | 软删除标记                          |

> 实体加 `#[navigation] pub category: BelongsTo<Category>` 和 `#[navigation] pub author: BelongsTo<User>` 和 `#[navigation] pub comments: HasMany<Comment>`。
> `tags` 用 String 存 JSON，读写时 `serde_json::to_string` / `from_str`。
> 审计字段 `created_id`/`updated_id` 与业务字段 `author_id` 语义不同：`author_id` 是博客作者，`created_id`/`updated_id` 是运维侧记录的创建/更新操作人。

#### 3.3.6 comments 表（评论，双自外键）

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
| updated\_id | Option<i32> | `#[index]`（无 FK）                                | 更新人（编辑评论时记录）     |
| updated\_at | i64         | `#[required]`                                   | 更新时间戳            |
| is\_deleted | bool        | `#[required]` `#[index]`                        | 软删除标记（审核隐藏）      |

> **双外键设计**：`parent_id` 用于"回复 @某人"的层级结构；`quoted_id` 用于"引用某条评论"的块引用。两者均可空，可同时存在（既回复又引用）。
> 实体加 `#[navigation] pub blog: BelongsTo<Blog>`、`#[navigation] pub user: BelongsTo<User>`、`#[navigation] pub parent: BelongsTo<Comment>`、`#[navigation] pub quoted: BelongsTo<Comment>`。

#### 3.3.7 exhibitions 表（作品展，存储 INDEX.json 数据）

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
| created\_id  | Option<i32>    | `#[index]`（无 FK）                                    | 创建人                |
| updated\_id  | Option<i32>    | `#[index]`（无 FK）                                    | 更新人                |
| is\_deleted  | bool           | `#[required]` `#[index]`                            | 软删除标记              |

> 实体加 `#[navigation] pub category: BelongsTo<Category>`。
> 此表存储原 `docs/rust-ef/INDEX.json` 等作品集元数据，便于检索。

#### 3.3.8 resources 表（权限资源，通用资源模型）

> **重新设计**：取消原 `route_pattern + method` 双列硬编码，改为通用 `type + value + properties` 三段式模型，可表达应用/模块/页面/操作/数据/其他六类资源，配合 `Authorize` 表支撑框架动态鉴权。

| 列名          | 类型          | 约束                                           | 说明                                         |
| ----------- | ----------- | -------------------------------------------- | ------------------------------------------ |
| id          | i32         | `#[primary_key]` `#[auto_increment]`         | 唯一标识                                       |
| name        | String      | `#[required]` `#[max_length(100)]`           | 资源名称                                       |
| description | String      | `#[max_length(500)]`                         | 资源描述                                       |
| type        | String      | `#[required]` `#[max_length(20)]` `#[index]` | 资源分类：`应用`/`模块`/`页面`/`操作`/`数据`/`其他`         |
| value       | String      | `#[required]` `#[max_length(200)]`           | 资源值；`页面`/`操作` 类型时为路由（如 `/api/blog/{slug}`） |
| properties  | String      | `#[required]`                                | 配置属性 JSON；`操作` 类型存 `{"method":"GET"}` 等    |
| created\_id | Option<i32> | `#[index]`                                   | 创建人（审计字段，无 FK）                             |
| created\_at | i64         | `#[required]`                                | 创建时间                                       |
| updated\_id | Option<i32> | `#[index]`                                   | 更新人（审计字段，无 FK）                             |
| updated\_at | i64         | `#[required]`                                | 更新时间                                       |
| is\_deleted | bool        | `#[required]` `#[index]`                     | 软删除标记                                      |

> **动态鉴权匹配规则**（host/authorizer.rs 实现）：
>
> 1. 加载所有 `type ∈ {页面, 操作}` 且 `is_deleted = false` 的 Resource。
> 2. 对入站请求：用 `value`（路由模式，支持 `{slug}` 占位与 `*` 通配）匹配请求路径；`操作` 类型再从 `properties` 解析 `method` 匹配 HTTP 方法。
> 3. 命中 Resource → 查 `Authorize` 表得 `role_id` 集合 → 校验当前用户角色是否命中。

#### 3.3.9 authorizes 表（授权，Role ↔ Resource 多对多）

| 列名           | 类型  | 约束                                                  | 说明    |
| ------------ | --- | --------------------------------------------------- | ----- |
| id           | i32 | `#[primary_key]` `#[auto_increment]`                | 主键    |
| role\_id     | i32 | `#[required]` `#[foreign_key(Role)]` `#[index]`     | 角色 FK |
| resource\_id | i32 | `#[required]` `#[foreign_key(Resource)]` `#[index]` | 资源 FK |

> 实体加导航：`Role` 加 `#[navigation] pub resources: HasMany<Resource, Authorize>`，`Resource` 加 `#[navigation] pub roles: HasMany<Role, Authorize>`。

#### 3.3.10 tracking 表（站点跟踪）

| 列名           | 类型             | 约束                                            | 说明      |
| ------------ | -------------- | --------------------------------------------- | ------- |
| id           | i32            | `#[primary_key]` `#[auto_increment]`          | 主键      |
| path         | String         | `#[required]` `#[max_length(500)]` `#[index]` | 访问路径    |
| method       | String         | `#[required]` `#[max_length(10)]`             | HTTP 方法 |
| ip           | String         | `#[required]` `#[max_length(64)]`             | 客户端 IP  |
| user\_agent  | String         | `#[required]` `#[max_length(500)]`            | UA      |
| referer      | Option<String> | `#[max_length(500)]`                          | 来源页     |
| status       | i32            | `#[required]`                                 | 响应状态码   |
| duration\_ms | i32            | `#[required]`                                 | 耗时(ms)  |
| visited\_at  | i64            | `#[required]` `#[index]`                      | 访问时间戳   |

> 日志表，不追加审计字段与软删除（按需定期清理）。

***

## 四、rust-ef 1.1.0 API 关键修正（源码实证，解决 7 个编译阻塞）

> 以下结论来自 rust-ef 1.1.0 crate 源码（`C:\Users\lusid\.cargo\registry\src\rsproxy.cn-e3de039b2554c837\rust-ef-1.1.0\`）与宏源码（gitcode master）逐行核验。

### 4.1 导航属性访问 API

| 类型             | 错误写法                  | 正确写法                     | 方法签名                              | 源码位置             |
| -------------- | --------------------- | ------------------------ | --------------------------------- | ---------------- |
| `HasMany<T,J>` | `e.roles.iter()`      | `e.roles.items().iter()` | `pub fn items(&self) -> &[T]`     | relations.rs:181 |
| `BelongsTo<T>` | `e.category.as_ref()` | `e.category.get()`       | `pub fn get(&self) -> Option<&T>` | relations.rs:63  |

* `HasMany` **未实现** `IntoIterator`/`Deref`，必须先 `.items()` 取 `&[T]` 再 `.iter()`。

* `BelongsTo` **未实现** `Deref`，用 `.get()` 返回 `Option<&T>`。

* 调用前须 `linq!(...; include b.nav)` 预加载，否则 `items()` 为空、`get()` 为 `None`。

### 4.2 重复 `FK_<Target>` 常量（E0592）规避

* **根因**：`#[derive(EntityType)]` 为每个 `#[foreign_key(Target)]` 生成 `pub const FK_{Target}: &'static str = {column};`，命名仅取目标实体名，与字段名无关；同实体两个 FK 指向同目标 → 重复定义。

* **宏不支持** `name=`/`as=` 消歧参数（`extract_foreign_key_target` 仅字符串化括号内全部 token）。

* **规避方案**：`Comment` 的 `parent_id` 保留 `#[foreign_key(Comment)]`，`quoted_id` 改为**裸** **`#[foreign_key]`**（无参数）。裸形式经 `has_attr` 仍置 `is_foreign_key=true`，但 `extract_foreign_key_target` 对 `Meta::Path` 返回 `None`，跳过常量生成。`FK_*` 常量无外部引用，安全。

* **连带风险**：`parent`/`quoted` 两个 `BelongsTo<Comment>` 的 `fk_column` 元数据都会被宏设为首个 FK 列（`blog_id`），导致 `include b.parent`/`b.quoted` 运行时按错误列关联。**规避**：不在 `linq!` 中 `include` `parent`/`quoted`，改为查询后按 `parent_id`/`quoted_id` 二次查询手动装配。

### 4.3 `linq!` include 语法

* 正确：`linq!(ctx.set::<Blog>(); include b.posts)` —— 分号分隔，点访问，`b` 隐式绑定。

* 嵌套：`linq!(ctx.set::<Blog>(); include b.posts then b.comments)`。

* 错误：`include b => b.posts`（`=>` 会被 order 子句解析器误消费）。

### 4.4 其他已确认 API

* `has_data(&mut self, data: &[T])` —— 取切片引用，非 `Vec`。

* `set::<T>()` 用于 CRUD；`model().entity::<T>()` 仅用于配置/种子。

* `DbContext::from_options(&options)?` 同步返回 `EFResult<DbContext>`，无 `.await`。

* 错误类型：`rust_ef::error::{EFResult, EFError}`（非 `LrefResult`）。

* 实体 derive 仅 `#[derive(Debug, Clone, EntityType)]`，**不加** `Serialize, Deserialize`（导航字段未实现这些 trait）。

* `on_save_failed` 返回 `()`，非 `EFResult<()>`（与 `on_saving`/`on_saved` 不同）。

***

## 五、实体定义（已修正，含审计字段 + 简化命名）

> 命名规则：实体类型简化（`Blog` 非 `BlogEntity`）；DTO 保留 `*Model` 后缀。审计字段 `created_id`/`updated_id` 一律 `Option<i32>` + `#[index]`，**不加** **`#[foreign_key]`**。

### 5.1 [domain/src/entities/user.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/user.rs)

```rust
use rust_ef::prelude::*;
use super::role::{Role, RoleUser};

#[derive(Debug, Clone, EntityType)]
#[table("users")]
pub struct User {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[max_length(100)] pub name: String,
    #[required] #[max_length(200)] #[unique] pub email: String,
    #[required] #[max_length(200)] pub password_hash: String,
    #[index] pub created_id: Option<i32>,   // 无 FK
    #[required] pub created_at: i64,
    #[index] pub updated_id: Option<i32>,   // 无 FK
    #[required] pub updated_at: i64,
    #[required] #[index] pub is_deleted: bool,
    #[navigation] pub roles: HasMany<Role, RoleUser>,
}
```

### 5.2 [domain/src/entities/role.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/role.rs)

```rust
use rust_ef::prelude::*;
use super::user::User;
use super::resource::{Resource, Authorize};

#[derive(Debug, Clone, EntityType)]
#[table("roles")]
pub struct Role {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[max_length(50)] #[unique] pub name: String,
    #[max_length(200)] pub description: String,
    #[index] pub created_id: Option<i32>,
    #[required] pub created_at: i64,
    #[index] pub updated_id: Option<i32>,
    #[required] pub updated_at: i64,
    #[required] #[index] pub is_deleted: bool,
    #[navigation] pub users: HasMany<User, RoleUser>,
    #[navigation] pub resources: HasMany<Resource, Authorize>,
}

#[derive(Debug, Clone, EntityType)]
#[table("role_users")]
pub struct RoleUser {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[foreign_key(User)] #[index] pub user_id: i32,
    #[required] #[foreign_key(Role)] #[index] pub role_id: i32,
    #[required] pub created_at: i64,  // 联结表仅留 created_at
}
```

### 5.3 [domain/src/entities/category.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/category.rs)

```rust
use rust_ef::prelude::*;

#[derive(Debug, Clone, EntityType)]
#[table("categories")]
pub struct Category {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[max_length(100)] pub name: String,
    #[required] #[max_length(100)] #[unique] pub slug: String,
    #[foreign_key(Category)] pub parent_id: Option<i32>,  // 自外键
    #[required] pub sort_order: i32,
    #[index] pub created_id: Option<i32>,
    #[required] pub created_at: i64,
    #[index] pub updated_id: Option<i32>,
    #[required] pub updated_at: i64,
    #[required] #[index] pub is_deleted: bool,
    #[navigation] pub parent: BelongsTo<Category>,
    #[navigation] pub children: HasMany<Category>,
}
```

### 5.4 [domain/src/entities/blog.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/blog.rs)

```rust
use rust_ef::prelude::*;
use super::category::Category;
use super::user::User;
use super::comment::Comment;

#[derive(Debug, Clone, EntityType)]
#[table("blogs")]
pub struct Blog {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[max_length(200)] #[unique] pub slug: String,
    #[required] #[max_length(200)] pub title: String,
    #[max_length(500)] pub summary: String,
    #[required] pub content: String,
    #[required] pub tags: String,           // JSON 数组字符串
    #[required] #[foreign_key(Category)] #[index] pub category_id: i32,
    #[required] #[foreign_key(User)] #[index] pub author_id: i32,
    #[required] pub published_at: i64,
    #[required] pub created_at: i64,
    #[required] pub updated_at: i64,
    #[index] pub created_id: Option<i32>,   // 运维审计创建人（无 FK）
    #[index] pub updated_id: Option<i32>,   // 运维审计更新人（无 FK）
    #[required] #[index] pub is_deleted: bool,
    #[navigation] pub category: BelongsTo<Category>,
    #[navigation] pub author: BelongsTo<User>,
    #[navigation] pub comments: HasMany<Comment>,
}
```

### 5.5 [domain/src/entities/comment.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/comment.rs)（双自外键规避）

```rust
use rust_ef::prelude::*;
use super::blog::Blog;
use super::user::User;

#[derive(Debug, Clone, EntityType)]
#[table("comments")]
pub struct Comment {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[foreign_key(Blog)] #[index] pub blog_id: i32,
    #[required] #[foreign_key(User)] #[index] pub user_id: i32,
    #[required] #[max_length(100)] pub user_name: String,
    #[required] pub content: String,
    #[foreign_key(Comment)] pub parent_id: Option<i32>,   // 命名 FK 常量
    #[foreign_key] pub quoted_id: Option<i32>,            // 裸形式，跳过 FK_ 常量生成
    #[required] pub created_at: i64,
    #[index] pub updated_id: Option<i32>,
    #[required] pub updated_at: i64,
    #[required] #[index] pub is_deleted: bool,
    #[navigation] pub blog: BelongsTo<Blog>,
    #[navigation] pub user: BelongsTo<User>,
    #[navigation] pub parent: BelongsTo<Comment>,   // 运行时勿 include，手动二次查询
    #[navigation] pub quoted: BelongsTo<Comment>,  // 运行时勿 include，手动二次查询
}
```

### 5.6 [domain/src/entities/exhibition.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/exhibition.rs)

```rust
use rust_ef::prelude::*;
use super::category::Category;

#[derive(Debug, Clone, EntityType)]
#[table("exhibitions")]
pub struct Exhibition {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[max_length(100)] #[unique] pub slug: String,
    #[required] #[max_length(200)] pub title: String,
    #[max_length(200)] pub subtitle: String,
    #[required] pub description: String,
    #[required] #[foreign_key(Category)] #[index] pub category_id: i32,
    #[required] pub tags: String,
    #[max_length(500)] pub repo_url: Option<String>,
    #[max_length(500)] pub demo_url: Option<String>,
    #[max_length(100)] pub docs_slug: Option<String>,
    #[required] pub featured: bool,
    #[required] pub sort_order: i32,
    #[max_length(500)] pub logo_url: Option<String>,
    #[required] pub created_at: i64,
    #[required] pub updated_at: i64,
    #[index] pub created_id: Option<i32>,
    #[index] pub updated_id: Option<i32>,
    #[required] #[index] pub is_deleted: bool,
    #[navigation] pub category: BelongsTo<Category>,
}
```

### 5.7 [domain/src/entities/resource.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/resource.rs)（重新设计）

```rust
use rust_ef::prelude::*;
use super::role::Role;

#[derive(Debug, Clone, EntityType)]
#[table("resources")]
pub struct Resource {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[max_length(100)] pub name: String,
    #[max_length(500)] pub description: String,
    #[required] #[max_length(20)] #[index] pub r#type: String,   // 应用/模块/页面/操作/数据/其他
    #[required] #[max_length(200)] pub value: String,            // 路由或资源值
    #[required] pub properties: String,                          // JSON 配置
    #[index] pub created_id: Option<i32>,
    #[required] pub created_at: i64,
    #[index] pub updated_id: Option<i32>,
    #[required] pub updated_at: i64,
    #[required] #[index] pub is_deleted: bool,
    #[navigation] pub roles: HasMany<Role, Authorize>,
}

#[derive(Debug, Clone, EntityType)]
#[table("authorizes")]
pub struct Authorize {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[foreign_key(Role)] #[index] pub role_id: i32,
    #[required] #[foreign_key(Resource)] #[index] pub resource_id: i32,
    #[required] pub created_at: i64,
}
```

> **注意**：`type` 是 Rust 关键字，字段名用 `r#type` 转义；rust-ef 按字段名生成列名 `type`（SQLite/MySQL 均为合法列名，无需引号）。

### 5.8 [domain/src/entities/tracking.rs](file:///e:/GitCode/RF/rust-webapp/docbit/domain/src/entities/tracking.rs)

```rust
use rust_ef::prelude::*;

#[derive(Debug, Clone, EntityType)]
#[table("tracking")]
pub struct Tracking {
    #[primary_key] #[auto_increment] pub id: i32,
    #[required] #[max_length(500)] #[index] pub path: String,
    #[required] #[max_length(10)] pub method: String,
    #[required] #[max_length(64)] pub ip: String,
    #[required] #[max_length(500)] pub user_agent: String,
    #[max_length(500)] pub referer: Option<String>,
    #[required] pub status: i32,
    #[required] pub duration_ms: i32,
    #[required] #[index] pub visited_at: i64,
}
```

### 5.9 conversions.rs 修正（导航访问 API）

```rust
// HasMany：items().iter()
let roles: Vec<String> = e.roles.items().iter().map(|r| r.name.clone()).collect();

// BelongsTo：get().map(...)
let category_name: String = e.category.get().map(|c| c.name.clone()).unwrap_or_default();
```

### 5.10 seed.rs 修正

* `has_data(&[...])` 取切片引用。

* 种子：2 个 Role（admin/user）+ 1 个 Category（uncategorized）。admin 用户在 host/startup.rs 用 bcrypt 运行时生成 hash 后插入（不硬编码 hash）。

* Resource 种子：写入核心操作资源（type=操作，value=各 API 路由，properties=`{"method":"GET"}` 等）。

***

## 六、handlers 层（11 模块，各含 service.rs + handler.rs）

> 每个 service 用 `#[rust_dicore::inject_attr]` 注册，实现 contracts 中的 trait 或直接消费 `Mutex<DbContext>`。所有列表查询带 `is_deleted = false` 过滤；写操作在 `save_changes` 前由 AuditInterceptor 注入 `created_id`/`updated_id`/时间戳。

| 模块         | service 职责                                                                  | handler 路由（已在 contracts 定义）                            |
| ---------- | --------------------------------------------------------------------------- | ------------------------------------------------------ |
| auth       | 注册(bcrypt)/登录(JWT)/me/忘记密码/重置密码                                             | POST /api/auth/register、/login、GET /api/auth/me 等      |
| blog       | 实现 `IBlogService`：linq! 查 Blog + include category/author/comments；slug 唯一校验 | GET /api/blog、/api/blog/{slug} 等                       |
| comment    | 按 blog\_id 列表；parent\_id/quoted\_id 二次查询手动装配（勿 include）                     | POST /api/comments、GET /api/comments/{blog\_id}、DELETE |
| category   | CRUD + 树构建（递归 children）                                                     | GET /api/categories、POST/PUT/DELETE                    |
| exhibition | CRUD + slug 唯一；`list_portfolio`/`get_portfolio` 实现                          | /api/exhibitions 系列（替换原 /api/works）                    |
| user       | 6 个 CRUD；不返回 password\_hash；assign/revoke 角色                                | /api/users 系列 + /api/role-users                        |
| rbac       | Role/Resource/Authorize CRUD；Resource 写入新字段结构                               | /api/roles、/api/resources、/api/authorizes              |
| tracking   | 汇总统计 + 列表（仅 admin）                                                          | GET /api/tracking、/api/tracking/summary                |
| docs       | 保留文件系统文档读取；`list_portfolio`/`get_portfolio` 转调 exhibition service           | /api/docs、/api/docs/{work}/index、/content              |
| site       | 读 appsettings.json 返回 SiteConfig                                            | GET /api/site                                          |
| cache      | 演示 MemoryCache get-or-create                                                | GET /api/cache/stats                                   |

***

## 七、host 层（组合根）

| 文件             | 职责                                                                                                                              |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| main.rs        | `Host::builder()` 入口，读取环境变量 `ASPNETCORE_ENVIRONMENT` 选 Development/Production                                                   |
| config.rs      | `AppSettings`：App/Jwt/Cors/Database 配置；Database.Provider = sqlite\|mysql                                                        |
| paths.rs       | AppPaths：wwwroot/db\_path/docs\_root（移除 blog\_root）                                                                             |
| bootstrap.rs   | `configure(svc)`：注册 AppPaths、`Mutex<DbContext>`、选 Provider（SQLite 本地 / MySQL 线上）                                                |
| interceptor.rs | `AuditInterceptor`（ISaveChangesInterceptor）：注入 created\_id/updated\_id/时间戳；错误类型用 EFResult/EFError                               |
| authorizer.rs  | `DynamicAuthorizer`：加载 type∈{页面,操作} Resource + Authorize → 缓存 AuthzMatrix；按 value 路由匹配 + properties.method 匹配；`#[authorize]` 接入 |
| middleware.rs  | `TrackingMiddleware`：记录访问到 tracking 表（异步，不阻塞响应）                                                                                 |
| startup.rs     | `DbInitService`（IHostedService）：`ensure_created()` + `has_data()` 种子 + docs.ensure\_all\_indexes + admin 用户 bcrypt 初始化          |

**数据库 Provider 选择**：

* Development：SQLite，`db_path = docbit/host/docbit.db`

* Production：MySQL `gz-cdb-g7aefwbv.sql.tencentcdb.com:63675`，用户 root，密码见 appsettings.Production.json（不入库）

***

## 八、实现步骤（8 步，含当前进度）

### 步骤 1 ✅ workspace 配置

根 [Cargo.toml](file:///e:/GitCode/RF/rust-webapp/Cargo.toml) 已加 4 个 docbit 子 crate member + workspace deps（rust-ef 1.1、rust-ef-sqlite 1.1、rust-ef-mysql 1.1）。

### 步骤 2 ✅ contracts crate（已完成编译）

11 个模块全部完成，已修复：path 参数 String 类型、inventory/async-trait 依赖、RevokeRole/ListComments/ListTracking 路由适配。

### 步骤 3 ⏳ domain crate（进行中，7 个错误待修）

已完成：9 实体定义骨架、conversions.rs、seed.rs。
**待修（本方案已给出修正）**：

1. comment.rs：`quoted_id` 改裸 `#[foreign_key]`（见 5.5）
2. conversions.rs：`HasMany` 用 `.items().iter()`、`BelongsTo` 用 `.get().map(...)`（见 5.9）
3. 全部实体补审计字段 + 软删除（见第五章）
4. resource.rs 按新设计重写（见 5.7）
5. seed.rs：`has_data(&[...])` 已修；补 Resource 种子
6. password\_reset.rs：保留（auth 模块用）

### 步骤 4 ⬜ handlers crate（11 模块，service + handler）

按第六章表逐模块实现。优先级：auth → blog → exhibition → category → comment → user → rbac → tracking → docs → site → cache。

### 步骤 5 ⬜ host crate

按第七章表实现 8 个文件。关键：authorizer.rs 的新 Resource 匹配逻辑、startup.rs 的 admin bcrypt 初始化。

### 步骤 6 ⬜ 配置 + wwwroot 迁移 + JS 路由

* appsettings.json / Development.json / Production.json 落到 docbit/host/

* wwwroot 迁到 docbit/host/wwwroot/

* 前端 JS：`/api/works` → `/api/exhibitions` 全局替换

### 步骤 7 ⬜ 删除旧文件

* docbit/src/（整树）

* docbit/Cargo.toml（旧单 crate）

* docbit/appsettings\*.json

* docbit/docbit.db

### 步骤 8 ⬜ 编译验证

* `cargo build --workspace`

* `cargo build -p docbit-host`

* 运行 docbit-host，验证：登录、博客 CRUD、评论引用/回复、分类树、作品列表、动态鉴权、访问统计

***

## 九、验证清单

* [ ] contracts crate `cargo build -p docbit-contracts` 通过

* [ ] domain crate `cargo build -p docbit-domain` 通过（含 9 实体 + 审计字段 + Resource 新设计）

* [ ] handlers crate `cargo build -p docbit-handlers` 通过

* [ ] host crate `cargo build -p docbit-host` 通过

* [ ] `cargo build --workspace` 全绿

* [ ] 运行后 SQLite 自动建 10 张表（含审计字段）

* [ ] admin 用户初始化（bcrypt hash 运行时生成）

* [ ] 博客从 DB 读取（非文件系统）

* [ ] 评论 parent\_id/quoted\_id 引用/回复工作（手动二次查询）

* [ ] 动态鉴权：未授权角色访问 /api/users 被拒

* [ ] 软删除：删除博客后列表不显示，DB 中 is\_deleted=true

* [ ] 线上 MySQL 连接成功（Production 配置）

* [ ] 前端 /api/exhibitions 正常返回作品列表

***

## 十、关键决策汇总

1. **实体命名简化**：`Blog` 非 `BlogEntity`；DTO 保留 `*Model`。
2. **审计字段不加 FK**：`created_id`/`updated_id` 用 `Option<i32>` + `#[index]`，避免自引用重复 `FK_User` 常量 + 软删除友好。
3. **Resource 重新设计**：`type + value + properties` 三段式通用模型，替换 `route_pattern + method`。
4. **双自外键规避**：`parent_id` 命名 FK，`quoted_id` 裸 `#[foreign_key]`；运行时勿 include parent/quoted，手动二次查询。
5. **导航访问 API**：`HasMany.items().iter()`、`BelongsTo.get().map(...)`。
6. **blogs.created\_id vs author\_id**：author\_id 是博客作者，created\_id/updated\_id 是运维审计操作人，语义分离。
7. **联结表/日志表不加审计**：role\_users、authorizes 仅 created\_at；tracking 仅 visited\_at。

