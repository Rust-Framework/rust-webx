# docbit 生产部署

**推荐方式：** 在 Linux 上编译并发布裸机目录（`docbit-host` + `wwwroot` + `appsettings` + `docs`），手动部署到服务器。不使用 Docker 作为主路径。

## 环境变量

| 变量 | 必需 (Production) | 说明 |
|------|-------------------|------|
| `APP_ENV` | 是 | 设为 `Production` |
| `JWT_SECRET` | 是 | ≥32 字符强密钥；也可使用 `APP__Jwt__Secret` |
| `APP__App__Urls` | 否 | 监听地址 JSON 数组，默认 `http://0.0.0.0:8100` |
| `APP__Cors__Origins` | 建议 | 生产 CORS 白名单，勿使用 `*` |
| `APP__RateLimit__Enabled` | 建议 | 生产启用速率限制（docbit 默认 true） |
| `APP__Metrics__Enabled` | 建议 | 启用 `GET /metrics` Prometheus 指标 |

开发与生产均使用 SQLite（`<app_base>/app.db`），无需 `DATABASE_URL` 或外部数据库服务。

## Linux 编译与发布

### 生态文档

Monorepo 开发时 `DocService` 会实时解析 sibling 仓库文档，**无需**将 sibling 文档复制到 `rust-webx/docs/`。

**发布时**，`docbit/publish.sh` 直接从源仓库复制五项目文档到 bundle 的 `docs/`：

| 源 | bundle 目标 |
|----|-------------|
| `{framework}/rust-dix/docs/rust-dix` | `docs/rust-dix` |
| `{framework}/rust-ef/docs/rust-ef` | `docs/rust-ef` |
| `{framework}/rust-agent-framework/docs` | `docs/rust-agent-framework` |
| `{framework}/rust-gpui-rml/docs` | `docs/rust-gpui-rml` |
| `{workspace}/docs/rust-webx` | `docs/rust-webx` |

`{framework}` 由 `RUST_FRAMEWORK_ROOT` 或 rust-webx 父目录解析（与 `framework_root()` 一致）。

Git 仓库**只提交** `docs/rust-webx/`；sibling 手册 canonical 源在各生态仓库，**不要** commit 到 `rust-webx/docs/`。

### 编译 release 二进制

```bash
# 本机 Linux
cargo build --release -p docbit-host

# 交叉编译（示例：GNU target）
rustup target add x86_64-unknown-linux-gnu
cargo build --release -p docbit-host --target x86_64-unknown-linux-gnu
# 产物：target/x86_64-unknown-linux-gnu/release/docbit-host
```

### 打包部署目录

```bash
chmod +x docbit/publish.sh
./docbit/publish.sh /opt/docbit --production
```

目录结构：

```
/opt/docbit/
├── docbit-host              # 可执行文件
├── appsettings.json
├── appsettings.Production.json
├── wwwroot/                 # SPA 静态资源
├── docs/                    # 五项目文档（publish 时从源仓库复制）
└── run.sh                   # 生产启动脚本（--production 时生成）
```

编辑 `run.sh`，填入 `JWT_SECRET` 后启动：

```bash
cd /opt/docbit
./run.sh
```

### Windows 开发机打包

```powershell
.\docbit\publish.ps1 -Destination D:\deploy\docbit -Production
```

发布脚本会自动从 monorepo 源仓库复制文档，无需事先运行 `sync-docs`。

### 可选：本地 staging

```powershell
.\scripts\sync-docs.ps1   # 将文档镜像到 rust-webx/docs/ 供本地预览（非发布必需）
```

## Docker（可选，非主路径）

`docbit/Dockerfile` 与 `docker-compose.yml` 仅作参考，**不在 CI 中维护**。若需容器化，请自行验证镜像构建；官方推荐裸机 Linux 发布。

<details>
<summary>Docker Compose 参考</summary>

```bash
cp docbit/.env.example docbit/.env
# 编辑 JWT_SECRET
docker compose -f docbit/docker-compose.yml --env-file docbit/.env up --build
```

</details>

## 生产中间件

`APP_ENV=Production` 时 docbit 自动启用（appsettings）：

- 速率限制（20 req/s，burst 40，见 `RateLimit` 节）
- `GET /metrics` Prometheus 指标

并额外启用以下中间件（代码注册）：
- Gzip 压缩
- 请求耗时（`TimingMiddleware`）
- 结构化请求追踪（`RequestTracing`）

框架默认已包含安全响应头、`X-Request-Id` 与 JWT 认证。

## 健康检查

- `GET /health` — 存活探针
- `GET /health/ready` — 就绪探针（含已注册 health check）

裸机部署可用相同路径做存活/就绪探针。
