# docbit 生产部署

## 环境变量

| 变量 | 必需 (Production) | 说明 |
|------|-------------------|------|
| `APP_ENV` | 是 | 设为 `Production` |
| `DATABASE_URL` | 是 | MySQL 连接串，如 `mysql://user:pass@host:3306/docbit` |
| `JWT_SECRET` | 是 | ≥32 字符强密钥；也可使用 `APP__Jwt__Secret` |
| `APP__App__Urls` | 否 | 监听地址 JSON 数组，默认 `http://0.0.0.0:8100` |
| `APP__Cors__Origins` | 建议 | 生产 CORS 白名单，勿使用 `*` |

开发模式（默认）使用 SQLite（`<app_base>/app.db`），无需 `DATABASE_URL`。

## Docker Compose

```bash
cp docbit/.env.example docbit/.env
# 编辑 JWT_SECRET 与 MYSQL_ROOT_PASSWORD

docker compose -f docbit/docker-compose.yml --env-file docbit/.env up --build
```

服务监听 `http://localhost:8100`（可通过 `DOCBIT_PORT` 调整）。

## 裸机发布

```bash
# Linux / macOS
chmod +x docbit/publish.sh
./docbit/publish.sh /opt/docbit --production

# Windows
.\docbit\publish.ps1 -Destination D:\deploy\docbit -Production
```

启动前设置 `DATABASE_URL` 与 `JWT_SECRET`，或编辑生成的 `run.sh` / `run.cmd`。

## 生产中间件

`APP_ENV=Production` 时 docbit 自动启用：

- 速率限制（20 req/s，burst 40）
- Gzip 压缩
- 请求耗时（`TimingMiddleware`）
- 结构化请求追踪（`RequestTracing`）

框架默认已包含安全响应头、`X-Request-Id` 与 JWT 认证。

## 健康检查

- `GET /health` — 存活探针
- `GET /health/ready` — 就绪探针（含已注册 health check）

Docker 镜像内置 `HEALTHCHECK` 指向 `/health`。
