# rust-webapp 文档

本目录为 **rust-webapp 开发者手册** 的 canonical 源。

## 结构

```
docs/
└── rust-webapp/          # Docbit 作品 slug，对应 /api/docs/rust-webapp/*
    ├── FOREWORD.md       # 前言
    ├── INDEX.md          # 全书目录
    ├── INDEX.json        # 文档网站左侧菜单
    └── 01-introduction/  # 各章节（共 16 章）
        ├── INDEX.md
        └── *.md
```

## 阅读

- **本地站点**：`cargo run -p docbit` → 作品集 → rust-webapp → 文档
- **API**：`GET /api/docs/rust-webapp/index`、`GET /api/docs/rust-webapp/content/{path}`

## 维护

编辑 `docs/rust-webapp/` 下的 Markdown 即可；Docbit 启动时会自动确保 `INDEX.json` 存在。
