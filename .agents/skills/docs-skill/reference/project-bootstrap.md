# 项目文档 Bootstrap

每个仓库动手写文档前，先完成本清单。目标：从**当前项目**读取约定，而非复用其他框架的硬编码值。

## 发现路径

按顺序检查（存在则读取）：

| 优先级 | 路径 | 提取信息 |
|--------|------|---------|
| 1 | `docs/README.md` | docs 根布局、slug、预览命令、API 路径 |
| 2 | `docs/{slug}/FOREWORD.md` | 读者画像、阅读路径、符号约定、示例项目 |
| 3 | `docs/{slug}/INDEX.md` | 全书结构、已有章/小节链接 |
| 4 | `docs/{slug}/INDEX.json` | meta、parts、pathRules、section 元数据 |
| 5 | 仓库根 `README.md` / `Cargo.toml` workspace | 框架名、crate 名、版本 |
| 6 | 示例 crate 或 quickstart 章 | 可运行命令、导入路径 |

### 确定 `{slug}`

- `docs/` 下每个**子目录**若含 `INDEX.md` 或 `INDEX.json`，通常即一本书的 slug
- Docbit：`/api/docs/{slug}/index` 与目录名对应
- 用户指定框架名时，优先匹配目录名而非 crate 名（二者可能不同）

### 确定站点类型

| 迹象 | 类型 | 索引要求 |
|------|------|---------|
| `INDEX.json` + pathRules + Docbit | Docbit v2 | 三处 INDEX 全同步 |
| `SUMMARY.md` + mdBook | mdBook | 改 SUMMARY，无 INDEX.json |
| 仅 `INDEX.md` 链接 | 静态 Markdown | 根 + 章 INDEX 即可 |

本技能默认 **Docbit v2**；若为其他站点，跳过 `INDEX.json` 步骤，保留渐进式披露与 Markdown 结构。

---

## 文档 Profile 模板

编写正文前，在脑中或对话中确认（**从仓库读取，勿臆填**）：

```markdown
## 文档 Profile · {仓库名}

- slug: `{slug}`
- 框架/产品名: 
- 书籍标题: （INDEX.json meta.docTitle 或 INDEX.md 标题）
- 文档根路径: `docs/{slug}/`
- 本地预览: （如 `cargo run -p docbit`）
- 默认代码导入: （如 `use my_framework::*;`）
- 示例项目/crate: 
- 路径参数写法: （如 `{id}`）
- 推荐/反模式符号: （通常 ✅ / ❌）
- 正文语言: （默认 zh-CN，以 FOREWORD 为准）
- 已有 parts 数: 
- 当前任务章节: 
```

Profile 中任一字段在仓库找不到时：**向用户确认**，或从最接近的现有小节推断并注明。

---

## 绿场：新建第一本书

仓库尚无 `docs/{slug}/` 时：

1. 与用户确认 `{slug}`（kebab-case，与 Docbit/API 路径一致）
2. 创建骨架：

```
docs/
├── README.md              # 说明 slug、预览、维护方式
└── {slug}/
    ├── FOREWORD.md        # 定位、读者、阅读路径、约定与符号
    ├── INDEX.md           # 空目录框架，随章增长
    ├── INDEX.json         # meta + parts（可先 2 章：introduction + quickstart）
    └── 01-introduction/
        ├── INDEX.md
        └── what-is-{slug}.md
```

3. `INDEX.json` meta 至少包含：`title`、`docTitle`、`description`、`foreword`、`pathRules`
4. FOREWORD 必须写清：**渐进式披露阅读路径**、**代码导入约定**、**示例项目**
5. 第一章 + 第二章优先于 deep-dive 章（见 progressive-disclosure.md）

---

## 多框架 Monorepo

```
docs/
├── README.md
├── framework-a/
│   ├── FOREWORD.md
│   ├── INDEX.md
│   └── INDEX.json
└── framework-b/
    └── ...
```

- 每次任务只激活**一个** slug 的 Profile
- 不跨 slug 复制章节正文；可复用**结构模式**，内容必须重写为对应框架 API
- 共享 Docbit 站点时，各 slug 独立 `INDEX.json`

---

## Bootstrap 检查清单

- [ ] 已定位 `docs/{slug}/` 且 slug 与用户意图一致
- [ ] 已读 FOREWORD 中的约定与符号
- [ ] 已读根 INDEX.md 确认章节归属
- [ ] 若存在 INDEX.json，已确认 pathRules 与现有章节 id 一致
- [ ] 已确认预览/验证命令
- [ ] 已确认示例代码的 crate 名与 import 路径

通过后，再加载 book-structure / writing-style 开始写作。
