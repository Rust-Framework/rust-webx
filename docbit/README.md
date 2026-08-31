# Docbit — Rust-Framework 生态作品集

Docbit 是 rust-webx 的参考应用：作品集 SPA + 博客 + 五项目完整文档浏览。

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

## 五项目文档

启动后访问：

| 项目 | 首页入口 | 文档 URL |
|------|---------|----------|
| rust-dix | `/works/rust-dix` | `/works/rust-dix/docs` |
| rust-ef | `/works/rust-ef` | `/works/rust-ef/docs` |
| rust-webx | `/works/rust-webx` | `/works/rust-webx/docs` |
| rust-agent-framework | `/works/rust-agent-framework` | `/works/rust-agent-framework/docs` |
| rust-gpui-rml | `/works/rust-gpui-rml` | `/works/rust-gpui-rml/docs` |

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
├── domain/      # EF 实体 + seed（五项目 exhibition）
├── host/        # build_host() + main
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

发布脚本会从 monorepo 源仓库复制五项目文档到 bundle（需完整 checkout 或设置 `RUST_FRAMEWORK_ROOT`）：

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

- 邮箱：`admin@docbit.local`
- 密码：`admin123`

## 数据库与种子数据

展览（五项目作品）数据在 **首次创建数据库** 时由 EF seed 写入。若你之前已运行过 docbit，升级后看不到新作品入口，请删除本地 SQLite 文件后重启：

```bash
# 默认位于运行目录下的 app.db（及 -shm / -wal 伴随文件）
rm app.db app.db-shm app.db-wal
cargo run -p docbit-host
```
