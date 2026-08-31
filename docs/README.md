# rust-webx 文档

本目录为 **rust-webx 开发者手册** 的 canonical 源，以及 Docbit 文档服务的 workspace 根。

## 结构

```
docs/
├── README.md           # 本文件
├── ARCHITECTURE_REMEDIATION.md
└── rust-webx/          # 唯一提交到 git 的手册（Docbit slug: rust-webx）
    ├── FOREWORD.md
    ├── INDEX.md
    ├── INDEX.json
    └── 01-introduction/ … 16-migration/
```

**不在 git 中提交**（见根 `.gitignore`）：

- `docs/rust-dix/`、`docs/rust-ef/`、`docs/rust-agent-framework/`、`docs/rust-gpui-rml/`
- 这些是 **复制** 镜像（`scripts/sync-docs.ps1` / `sync-docs.sh`），**不是符号链接**（Windows 与 standalone clone 不支持 git symlink）。

## 文档解析（DocService）

| 场景 | 行为 |
|------|------|
| **Monorepo 开发** | 实时读取 sibling 仓库（如 `../rust-ef/docs/rust-ef`），可通过 `RUST_FRAMEWORK_ROOT` 指定根目录 |
| **GitHub / standalone clone** | 仅 `docs/rust-webx/` 可用；其他 slug 需 sibling checkout 或 publish 前 sync |
| **生产 publish bundle** | 运行 `sync-docs` 后 `publish.sh` 将整个 `docs/` 树复制进部署目录（不提交） |

优先级：`{app_base}/docs/{slug}` → `rust-webx/docs/{slug}` → `{framework_root}/{sibling}/docs/...`

## 阅读

- **本地站点**：`cargo run -p docbit-host` → 作品集 → rust-webx → 文档
- **API**：`GET /api/docs/rust-webx/index`、`GET /api/docs/rust-webx/content/{path}`

## 维护

- 编辑 **`docs/rust-webx/`** 下的 Markdown；Docbit 启动时会自动确保 `INDEX.json` 存在。
- 更新 sibling 手册：在上游仓库改，或在发布前运行 `scripts/sync-docs.*` 生成本地镜像（勿 commit 镜像目录）。
