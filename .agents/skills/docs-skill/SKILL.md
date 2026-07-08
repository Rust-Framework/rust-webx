---
name: docs-skill
description: >
  为框架源码仓库生成或更新开发者手册式文档。遵循渐进式披露原则与标准书籍结构
  （FOREWORD / INDEX.md / INDEX.json / 章 INDEX + 小节 Markdown）。适用于多项目、
  多框架 docs 编写与维护；在用户提及 docs、文档、开发者手册、书籍结构、
  INDEX.json、Docbit、渐进式披露或新建/修订框架文档章节时使用。
---

# 文档编写技能

跨仓库通用的**框架开发者手册**编写规范。每个项目先建立 **文档 Profile**，再按 Profile 写正文；详细规则按需加载支持文件，勿一次性全部读入。

---

## 何时激活

- 为任意框架/库仓库编写、更新、重构 `docs/` 下书籍正文
- 新建文档站点条目、章节、小节
- 用户提及：开发者手册、书籍结构、`INDEX.json`、Docbit、渐进式披露

**不激活：** 纯代码实现、与书籍无关的根 README、一次性 API 注释、非 Markdown 书籍形态（除非用户明确要求适配）。

---

## 支持文件索引

| 文件 | 加载条件 | 内容 |
|------|---------|------|
| [reference/project-bootstrap.md](reference/project-bootstrap.md) | **每个项目首次编写前必载** | 发现 slug、约定、预览方式、Profile 模板 |
| [reference/book-structure.md](reference/book-structure.md) | 新建/改目录、同步 INDEX | 通用目录布局、命名、三处 INDEX 同步 |
| [reference/progressive-disclosure.md](reference/progressive-disclosure.md) | 规划章节顺序、控密度 | 披露层级、单节编排、边界写法 |
| [reference/writing-style.md](reference/writing-style.md) | 撰写或润色正文 | 模板、符号、代码块、导航 |
| [examples/chapter-patterns.md](examples/chapter-patterns.md) | 需要结构范例 | 通用章/节模式 |
| [examples/rust-webx-reference.md](examples/rust-webx-reference.md) | 需要完整落地样例 | rust-webx 参考实现路径 |

---

## 核心工作流

```
任务进度：
- [ ] 0. 建立当前项目的文档 Profile（bootstrap）
- [ ] 1. 确认目标（新书 / 新章 / 新小节 / 修订）
- [ ] 2. 对照源码与现有 docs，验证内容真实存在
- [ ] 3. 确定披露层级与在全书中的位置
- [ ] 4. 创建/编辑 Markdown（遵循 Profile 与 book-structure）
- [ ] 5. 同步三处索引（章 INDEX、根 INDEX.md、INDEX.json）
- [ ] 6. 质量检查（链接、披露顺序、项目约定）
```

### 步骤 0：建立文档 Profile（必做）

**先读** [reference/project-bootstrap.md](reference/project-bootstrap.md)，在当前对话中填写 Profile（或 mentally track）：

| 字段 | 来源 |
|------|------|
| `{slug}` | `docs/{slug}/` 目录名 |
| 框架/产品名 | `INDEX.json` meta 或 FOREWORD |
| 预览命令 | `docs/README.md` 或仓库说明 |
| 代码导入约定 | FOREWORD「约定与符号」 |
| 示例项目 | FOREWORD 或 quickstart 章 |
| 全书 parts 结构 | 根 `INDEX.md` + `INDEX.json` |

**无现成书籍时：** 按 [reference/book-structure.md](reference/book-structure.md) 绿场初始化；parts 划分参考 [reference/progressive-disclosure.md](reference/progressive-disclosure.md) 通用六段式，按框架能力裁剪。

**禁止**未读 Profile 就套用其他项目的 crate 名、导入语句或章号。

### 步骤 1–6

1. **目标** — 属于全书哪一部分？教程章还是概念章？（见 progressive-disclosure 通用分段）
2. **验证** — 能力已在源码实现；运行项目构建/测试后再写「已支持」
3. **披露** — 先能跑（L0）→ 常用配置（L1–2）→ 子系统深入（L3）→ 扩展边界（L4）
4. **文件** — 标准布局见 book-structure；路径中的 `{slug}` 替换为 Profile 值
5. **正文** — 按 writing-style 选模板 A/B/C
6. **检查** — 见下方清单

---

## 标准书籍布局（摘要）

```
docs/
├── README.md                 # 维护说明
└── {slug}/                   # 一本书 = 一个 slug
    ├── FOREWORD.md
    ├── INDEX.md
    ├── INDEX.json
    └── {NN-chapter}/
        ├── INDEX.md
        └── {section-id}.md
```

新增小节**必须**同步： `{chapter}/INDEX.md`、根 `INDEX.md`、`INDEX.json` → `sections`。

`INDEX.json` pathRules（Docbit v2 通用）：

```json
"pathRules": {
  "chapterIndex": "{chapterId}/INDEX.md",
  "sectionFile": "{chapterId}/{sectionId}.md"
}
```

完整字段与命名规则 → [reference/book-structure.md](reference/book-structure.md)。

---

## 质量检查清单

- [ ] 内容对照**当前仓库**源码，非臆造 API
- [ ] 遵循**当前项目** FOREWORD 中的符号与导入约定
- [ ] 每节单一主题；小节含 **小结** + **下一节/章** 导航
- [ ] `section.id` === 文件名（无 `.md`）=== 三处索引一致
- [ ] 相对链接正确；跨章用 `../`
- [ ] 入门章无 L3+ 深度内容（链到对应章）
- [ ] 开发者指南形态：讲「何时用、为何这样」，非 rustdoc 堆砌

---

## 全书分段（通用模板）

新建书籍时可按框架能力映射以下 **parts**（章号与标题因项目而异）：

| Part | 典型内容 | 披露 |
|------|---------|------|
| 入门与认知 | 是什么、边界、生态 | L0 认知 |
| 快速上手 | 安装、Hello World、首个完整示例 | L0–L1 |
| 设计思想与架构 | 原则、架构图、模块划分 | L1 理解 |
| 核心开发模式 | 各子系统按主题分章 | L2–L3 |
| 配置、安全与生产 | 配置、认证、部署、观测 | L2 |
| 工程化与进阶 | 项目结构、扩展点、最佳实践 | L3–L4 |
| 案例与迁移 | 参考应用、从 X 迁移 | L4 融会贯通 |

已有 `INDEX.json` 的项目：**以现有 parts 为准**，勿强行改成上表。

---

## 注意事项

- 一本书一个 `{slug}`； monorepo 多框架 → 多个 `docs/{slug}/` 并列
- `INDEX.json` 与 Markdown 不同步会导致 Docbit 导航缺失
- 需要完整样例时读 [examples/rust-webx-reference.md](examples/rust-webx-reference.md)，勿把样例约定硬编码到其他项目
- API 大全指向 crate rustdoc 或项目 reference；书籍保持指南形态
