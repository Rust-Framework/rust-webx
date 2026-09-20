# Docbit — Rust-Framework 生态作品集

Docbit 是 rust-webx 的参考应用：作品集 SPA + 博客 + 六项目完整文档浏览。

## 快速开始

```bash
# 从 rust-webx workspace 根目录
cd D:\Github\Rust-Framework\rust-webx

# monorepo 开发无需 sync — DocService 会读取 sibling 仓库实时文档

# 启动开发服务器（SQLite，http://localhost:5000）
cargo run -p docbit-host

# 路由诊断
cargo run -p docbit-host -- --doctor
```

## 六项目文档

启动后访问：

| 项目 | 首页入口 | 文档 URL |
|------|---------|----------|
| rust-dix | `/works/rust-dix` | `/works/rust-dix/docs` |
| rust-ef | `/works/rust-ef` | `/works/rust-ef/docs` |
| rust-webx | `/works/rust-webx` | `/works/rust-webx/docs` |
| rust-agent-framework | `/works/rust-agent-framework` | `/works/rust-agent-framework/docs` |
| rust-gpui-rml | `/works/rust-gpui-rml` | `/works/rust-gpui-rml/docs` |
| rust-agent-flow | `/works/rust-agent-flow` | `/works/rust-agent-flow/docs` |

> `rust-agent-flow` 的源码仓库是 `rust-flow`，文档 slug 与目录名为 `rust-agent-flow`。

文档源文件各仓库 `docs/` 为 **source of truth**。

**Monorepo 开发（推荐）**：`DocService` 按 slug 实时解析 sibling 仓库文档，**无需**将 sibling 文档复制到 `rust-webx/docs/`：

1. `<app_base>/docs/{slug}/` — 发布 bundle（若存在）
2. `rust-webx/docs/{slug}/` — 仅 `rust-webx` 手册在 git 中；可选本地 staging
3. `{framework_root}/{repo}/docs/...` — sibling 实时路径（如 `rust-ef/docs/rust-ef`）

可通过 `RUST_FRAMEWORK_ROOT` 显式指定 monorepo 根目录。

**Standalone 发布**：`docbit/publish.*` 在打包时**直接从源仓库**复制文档到 bundle 的 `docs/`（无需事先 sync 到 `rust-webx/docs/`）。

**Git**：仅 `docs/rust-webx/` 纳入版本控制；sibling 手册 canonical 源在各生态仓库。

## 架构

```
docbit/
├── contracts/   # 路由 DTO + #[get]/#[authorize]
├── handlers/    # #[handler(inject)] + DocService
├── domain/      # EF 实体 + seed（六项目 exhibition）
├── host/        # Program.cs（main）+ startup/{extensions,hosted,seed} + build.rs
└── wwwroot/     # SPA（pages/docs/ 文档阅读器）
```

`DocService` 按 slug 解析文档根（deploy → workspace 手册 → sibling 实时路径），API：

- `GET /api/docs/{work}/index` — 侧边栏目录
- `GET /api/docs/{work}/content/{path}` — Markdown 正文

## 发布（Linux 裸机，推荐）

主部署方式为 **Linux 可执行文件 + 静态资源**，不使用 Docker。

### 1. 编译

在 Linux 上原生编译：

```bash
cargo build --release -p docbit-host
```

从 Windows 交叉编译 Linux 二进制（需安装对应 target）：

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release -p docbit-host --target x86_64-unknown-linux-gnu
# 静态链接可选：x86_64-unknown-linux-musl（需 musl 工具链）
```

### 2. 打包部署目录

发布脚本会从 monorepo 源仓库复制六项目文档到 bundle（需完整 checkout 或设置 `RUST_FRAMEWORK_ROOT`）：

```bash
chmod +x docbit/publish.sh
./docbit/publish.sh /opt/docbit --production
```

输出：`docbit-host`、`wwwroot/`、`appsettings*.json`、`docs/`、`run.sh`。

Windows 开发机发布：

```powershell
.\docbit\publish.ps1 -Destination D:\deploy\docbit -Production
```

### 可选：本地 staging

若需将文档镜像到 `rust-webx/docs/` 做本地预览（非日常开发必需）：

```bash
./scripts/sync-docs.sh        # Linux/macOS
# .\scripts\sync-docs.ps1     # Windows
```

详见 [PRODUCTION.md](PRODUCTION.md)。Docker 为可选参考，非主路径。

## 默认账号

| 环境 | 行为 |
|------|------|
| 开发（`APP_ENV` 未设或 `Development`） | 自动创建 `admin@docbit.local` / `admin123`，启动日志给出警告 |
| 生产（`APP_ENV=Production`） | **不创建任何账号**；必须设置 `DOCBIT_ADMIN_PASSWORD` 才会按该口令创建 |

**部署前务必设置**：

```bash
DOCBIT_ADMIN_PASSWORD='一个足够长的口令'
```

生产环境不会回落到内置口令，也从不把运维提供的口令写进日志。账号只在缺失时创建一次，改环境变量不会覆盖已存在账号的密码。

## 文档上传与数据自动同步

后台「作品管理」每个作品都有 **上传文档** 按钮：选择该作品的文档 zip（压缩包根目录直接包含 `INDEX.json`），一次请求完成

1. 解压到 `uploads/.tmp/` 下的暂存目录（防 zip-slip / 符号链接 / 解压炸弹）
2. 原子替换 `docs/{slug}/`（失败自动回滚，站点不会半更新）
3. 重新同步作品数据：INDEX.json → DB 作品行、logo → `wwwroot/assets/works/`

因此**不需要重启**：文档正文按请求实时读盘，作品元数据与 logo 在上传返回前已同步。上传新作品（此前无 seed 模板）也会自动建行，分类按 `meta.category` 映射，未知则归「未分类」。

首次部署需带 `[build-dependencies] rust-webx-build`（见 `host/build.rs`）。

## 博客图片与附件

博客编辑器（Vditor）的 `upload`/`insert` 已接到 `POST /api/media`：

- 文件存到 `<app_base>/uploads/{images|files}/{shard}/{id}.{ext}`（可写、与 exe 同级）
- 通过 `GET /api/media/...` 只读回送
- 光栅图片（png/jpg/gif/webp/avif/bmp）内联返回；**SVG 等一律作为下载**，避免同源脚本执行
- 上传接口需登录；`/api/media/...` 公开可读（内容里要能直接引用）

## 数据库与种子数据

展览（六项目作品）数据在 **首次创建数据库** 时由 EF seed 写入，之后每次启动由一个可复用的 catalog 同步例程刷新（与上传走同一条代码路径）。若本地 `app.db` 结构过旧，`DbInitService` 会检测到缺列并重建（**会清空数据**，仅本地开发如此）。

```bash
# 默认位于运行目录下的 app.db（及 -shm / -wal 伴随文件）
rm app.db app.db-shm app.db-wal
cargo run -p docbit-host
```
