//! Documentation filesystem service — scans ecosystem docs and serves INDEX.json + markdown.
//!
//! 实现合约层的 `IDocumentService`。docs 目录按优先级解析：
//! 1. `<app_base>/docs/{work}` — 发布 bundle
//! 2. `<workspace>/docs/{work}` — rust-webx 手册（git 仅提交 rust-webx/；可选本地 staging）
//! 3. `<framework_root>/{sibling}/docs/...` — monorepo  sibling 实时路径
//!
//! `list_portfolio` / `get_portfolio` 从文件系统 INDEX.json 读取元数据并
//! 返回 `ExhibitionModel`；DB 专属字段（id、category_id、created_at 等）
//! 填占位值，运行时实际展示用 exhibition handlers 从 DB 取。
//!
//! 架构分层：服务实现归属 `handlers` 层（contracts 定义 `IDocumentService`，
//! 此处提供 `DocService` 实现，通过 `#[rust_dix::inject]` 自动注册为
//! `dyn IDocumentService` 单例，供 `docs.rs` handlers 与 `startup.rs`
//! `DbInitService` 注入使用）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use docbit_contracts::docs::{DocContent, DocIndex, DocIndexItem, IDocumentService};
use docbit_contracts::exhibition::ExhibitionModel;
use rust_webx::{app_base, framework_root, inject, Inject};

/// Ecosystem documentation slugs served by docbit.
const WORK_SLUGS: &[&str] = &[
    "rust-dix",
    "rust-ef",
    "rust-webx",
    "rust-agent-framework",
    "rust-gpui-rml",
];

// `#[derive(Inject)]` 生成 `__rdi_construct_DocService` 构造器。
#[derive(Inject)]
pub struct DocService;

impl DocService {
    /// Relative doc path inside the Rust-Framework monorepo for a work slug.
    fn sibling_doc_relative(work: &str) -> Option<&'static str> {
        match work {
            "rust-dix" => Some("rust-dix/docs/rust-dix"),
            "rust-ef" => Some("rust-ef/docs/rust-ef"),
            "rust-webx" => Some("rust-webx/docs/rust-webx"),
            "rust-agent-framework" => Some("rust-agent-framework/docs"),
            "rust-gpui-rml" => Some("rust-gpui-rml/docs"),
            _ => None,
        }
    }

    /// Workspace-level docs mirror (`rust-webx/docs/`).
    fn workspace_docs_root() -> Option<PathBuf> {
        let app = app_base();
        let mut candidates = vec![app.join("docs")];
        if let Some(parent) = app.parent() {
            candidates.push(parent.join("docs"));
            if let Some(grandparent) = parent.parent() {
                candidates.push(grandparent.join("docs"));
            }
        }
        candidates.into_iter().find(|path| path.is_dir())
    }

    /// Legacy aggregate docs root (first existing parent docs directory).
    fn root() -> PathBuf {
        Self::workspace_docs_root()
            .unwrap_or_else(|| app_base().join("docs"))
    }

    /// Resolve the directory for one work slug (deploy mirror → workspace mirror → live sibling).
    fn work_dir(work: &str) -> PathBuf {
        let deploy = app_base().join("docs").join(work);
        if deploy.is_dir() {
            return deploy;
        }

        if let Some(mirror_root) = Self::workspace_docs_root() {
            let mirrored = mirror_root.join(work);
            if mirrored.is_dir() {
                return mirrored;
            }
        }

        if let Some(framework) = framework_root() {
            if let Some(rel) = Self::sibling_doc_relative(work) {
                let live = framework.join(rel);
                if live.is_dir() {
                    return live;
                }
            }
        }

        deploy
    }

    fn write_index(&self, work: &str, index: &DocIndex) -> Result<(), String> {
        let path = Self::work_dir(work).join("INDEX.json");
        let json = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())
    }

    fn build_index(&self, work: &str) -> Result<DocIndex, String> {
        let dir = Self::work_dir(work);
        let items = self.scan_dir(&dir, &dir)?;
        let title = humanize_slug(work);
        Ok(DocIndex { title, items })
    }

    fn scan_dir(&self, base: &Path, current: &Path) -> Result<Vec<DocIndexItem>, String> {
        let mut entries: Vec<_> = fs::read_dir(current)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        let mut items = Vec::new();
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "INDEX.json" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                let children = self.scan_dir(base, &path)?;
                if !children.is_empty() {
                    items.push(DocIndexItem {
                        title: humanize_slug(&name),
                        path: None,
                        children: Some(children),
                    });
                }
            } else if name.ends_with(".md") {
                let rel = path
                    .strip_prefix(base)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                let title = title_from_markdown(&path).unwrap_or_else(|| stem_title(&name));
                items.push(DocIndexItem {
                    title,
                    path: Some(rel),
                    children: None,
                });
            }
        }
        Ok(items)
    }

    fn load_portfolio_item(&self, dir_slug: &str) -> Result<ExhibitionModel, String> {
        let raw = fs::read_to_string(Self::work_dir(dir_slug).join("INDEX.json"))
            .map_err(|e| e.to_string())?;
        let raw = strip_json_preamble(&raw)?;

        if let Ok(root) = serde_json::from_str::<IndexRootV2>(raw) {
            return Ok(meta_to_exhibition_model(dir_slug, &root.meta, !root.parts.is_empty()));
        }

        let legacy: DocIndex =
            serde_json::from_str(raw).map_err(|e| format!("Invalid INDEX.json: {}", e))?;
        Ok(ExhibitionModel {
            id: String::new(),
            slug: dir_slug.to_string(),
            title: legacy.title,
            subtitle: String::new(),
            description: String::new(),
            category_id: String::new(),
            category_name: "product".into(),
            category: "product".into(),
            tags: vec![],
            repo_url: None,
            demo_url: None,
            docs_slug: if legacy.items.is_empty() {
                None
            } else {
                Some(dir_slug.to_string())
            },
            featured: false,
            sort_order: 0,
            logo_url: self.logo_public_url(dir_slug, "logo.svg"),
            created_at: 0,
            updated_at: 0,
        })
    }

    fn logo_public_url(&self, dir_slug: &str, logo_name: &str) -> Option<String> {
        let path = Self::work_dir(dir_slug).join(logo_name);
        if !path.is_file() {
            return None;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("svg");
        Some(format!("/assets/works/{}.{}", dir_slug, ext))
    }

    fn resolve_logo_file(&self, dir_slug: &str, logo_name: &str) -> Result<Option<PathBuf>, String> {
        let path = Self::work_dir(dir_slug).join(logo_name);
        if path.is_file() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }
}

// `#[inject]` 在 trait impl 上：复用 `__rdi_construct_DocService` 构造器，
// 注册为 `dyn IDocumentService`（默认 singleton），供 handlers/hosted service 注入。
#[inject]
impl IDocumentService for DocService {
    fn list_works(&self) -> Result<Vec<String>, String> {
        let mut works: Vec<String> = WORK_SLUGS
            .iter()
            .filter(|slug| Self::work_dir(slug).is_dir())
            .map(|slug| (*slug).to_string())
            .collect();

        if let Some(root) = Self::workspace_docs_root() {
            for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !works.iter().any(|w| w == &name) {
                        works.push(name);
                    }
                }
            }
        }

        works.sort();
        Ok(works)
    }

    fn index(&self, work: &str) -> Result<DocIndex, String> {
        let dir = Self::work_dir(work);
        if !dir.is_dir() {
            return Err(format!("Documentation not found for work '{}'", work));
        }
        let index_path = dir.join("INDEX.json");
        if !index_path.exists() {
            let generated = self.build_index(work)?;
            self.write_index(work, &generated)?;
            return Ok(generated);
        }
        let raw = fs::read_to_string(&index_path).map_err(|e| e.to_string())?;
        parse_index_json(&raw)
    }

    fn content(&self, work: &str, path: &str) -> Result<DocContent, String> {
        let dir = Self::work_dir(work);
        if !dir.is_dir() {
            return Err(format!("Documentation not found for work '{}'", work));
        }
        let normalized = path.trim_start_matches('/');
        let file_path = dir.join(normalized);
        if !file_path.starts_with(&dir) {
            return Err("Invalid document path".into());
        }
        if !file_path.is_file() {
            return Err(format!("Document not found: {}", normalized));
        }
        let content = fs::read_to_string(&file_path).map_err(|e| e.to_string())?;
        Ok(DocContent {
            path: normalized.replace('\\', "/"),
            content,
        })
    }

    fn ensure_all_indexes(&self) -> Result<(), String> {
        let mut any = false;
        for slug in WORK_SLUGS {
            let dir = Self::work_dir(slug);
            if !dir.is_dir() {
                continue;
            }
            any = true;
            let index_path = dir.join("INDEX.json");
            if index_path.exists() {
                tracing::info!("[DocService] Using existing INDEX.json for '{}'", slug);
            } else {
                let generated = self.build_index(slug)?;
                self.write_index(slug, &generated)?;
                tracing::info!("[DocService] Generated INDEX.json for '{}'", slug);
            }
        }

        if !any {
            tracing::warn!(
                "[DocService] No documentation directories found (checked deploy mirror, workspace docs/, and monorepo siblings); skipping index generation"
            );
        }
        Ok(())
    }

    fn list_portfolio(&self) -> Result<Vec<ExhibitionModel>, String> {
        let mut items = Vec::new();
        for slug in self.list_works()? {
            let index_path = Self::work_dir(&slug).join("INDEX.json");
            if !index_path.is_file() {
                continue;
            }
            items.push(self.load_portfolio_item(&slug)?);
        }
        items.sort_by_key(|w| (w.sort_order, w.slug.clone()));
        Ok(items)
    }

    fn get_portfolio(&self, slug: &str) -> Result<ExhibitionModel, String> {
        let index_path = Self::work_dir(slug).join("INDEX.json");
        if !index_path.is_file() {
            return Err(format!("Portfolio item not found: {}", slug));
        }
        self.load_portfolio_item(slug)
    }

    fn sync_portfolio_assets(&self, wwwroot: &Path) -> Result<(), String> {
        let dest_dir = wwwroot.join("assets/works");
        fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;

        for slug in self.list_works()? {
            let index_path = Self::work_dir(&slug).join("INDEX.json");
            if !index_path.is_file() {
                continue;
            }
            let raw = fs::read_to_string(&index_path).map_err(|e| e.to_string())?;
            let raw = strip_json_preamble(&raw).unwrap_or("");
            let logo_name = serde_json::from_str::<IndexRootV2>(raw)
                .ok()
                .and_then(|r| r.meta.logo)
                .unwrap_or_else(|| "logo.svg".to_string());
            let Some(logo_file) = self.resolve_logo_file(&slug, &logo_name)? else {
                continue;
            };
            let ext = logo_file
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("svg");
            let dest = dest_dir.join(format!("{}.{}", slug, ext));
            fs::copy(&logo_file, &dest).map_err(|e| e.to_string())?;
            tracing::info!(
                "[DocService] Synced logo for '{}' -> {}",
                slug,
                dest.display()
            );
        }
        Ok(())
    }
}

// ── INDEX.json v2 (meta + parts + pathRules) ──

#[derive(Debug, Deserialize)]
struct IndexRootV2 {
    meta: IndexMeta,
    parts: Vec<IndexPart>,
}

#[derive(Debug, Deserialize)]
struct IndexMeta {
    title: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    subtitle: String,
    #[serde(default, rename = "docTitle")]
    doc_title: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_category")]
    category: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, rename = "repoUrl")]
    repo_url: String,
    #[serde(default, rename = "demoUrl")]
    demo_url: String,
    #[serde(default)]
    featured: bool,
    #[serde(default, rename = "sortOrder")]
    sort_order: i32,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    foreword: Option<String>,
    #[serde(rename = "pathRules")]
    path_rules: Option<IndexPathRules>,
}

fn default_category() -> String {
    "product".into()
}

fn opt_str(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[derive(Debug, Deserialize)]
struct IndexPathRules {
    #[serde(rename = "chapterIndex")]
    chapter_index: String,
    #[serde(rename = "sectionFile")]
    section_file: String,
}

#[derive(Debug, Deserialize)]
struct IndexPart {
    title: String,
    chapters: Vec<IndexChapter>,
}

#[derive(Debug, Deserialize)]
struct IndexChapter {
    id: String,
    title: String,
    sections: Vec<IndexSection>,
}

#[derive(Debug, Deserialize)]
struct IndexSection {
    id: String,
    title: String,
}

/// Strip UTF-8 BOM / leading whitespace so editors that save with BOM still parse.
fn strip_json_preamble(raw: &str) -> Result<&str, String> {
    let trimmed = raw.strip_prefix('\u{FEFF}').unwrap_or(raw).trim();
    if trimmed.is_empty() {
        return Err("Invalid INDEX.json: file is empty".into());
    }
    Ok(trimmed)
}

fn parse_index_json(raw: &str) -> Result<DocIndex, String> {
    let raw = strip_json_preamble(raw)?;
    if let Ok(root) = serde_json::from_str::<IndexRootV2>(raw) {
        return Ok(expand_index_v2(root));
    }
    serde_json::from_str(raw).map_err(|e| {
        let hint = if raw.starts_with('<') {
            " (got HTML/non-JSON — check deploy docs path or SPA fallback)"
        } else {
            ""
        };
        format!("Invalid INDEX.json: {}{}", e, hint)
    })
}

fn expand_index_v2(root: IndexRootV2) -> DocIndex {
    let rules = match &root.meta.path_rules {
        Some(r) => r,
        None => {
            return DocIndex {
                title: root.meta.display_doc_title(),
                items: vec![],
            };
        }
    };
    let mut items = Vec::new();

    if let Some(ref foreword) = root.meta.foreword {
        items.push(DocIndexItem {
            title: "前言".into(),
            path: Some(foreword.clone()),
            children: None,
        });
    }

    for part in &root.parts {
        let mut part_children = Vec::new();

        for chapter in &part.chapters {
            let mut chapter_children = Vec::new();

            let chapter_index_path = apply_path_rule(&rules.chapter_index, chapter.id.as_str(), "");
            chapter_children.push(DocIndexItem {
                title: "章节大纲".into(),
                path: Some(chapter_index_path),
                children: None,
            });

            for section in &chapter.sections {
                let path = apply_path_rule(&rules.section_file, &chapter.id, &section.id);
                chapter_children.push(DocIndexItem {
                    title: section.title.clone(),
                    path: Some(path),
                    children: None,
                });
            }

            part_children.push(DocIndexItem {
                title: chapter.title.clone(),
                path: None,
                children: Some(chapter_children),
            });
        }

        items.push(DocIndexItem {
            title: part.title.clone(),
            path: None,
            children: Some(part_children),
        });
    }

    DocIndex {
        title: root.meta.display_doc_title(),
        items,
    }
}

impl IndexMeta {
    fn display_doc_title(&self) -> String {
        if self.doc_title.is_empty() {
            self.title.clone()
        } else {
            self.doc_title.clone()
        }
    }
}

fn apply_path_rule(rule: &str, chapter_id: &str, section_id: &str) -> String {
    rule.replace("{chapterId}", chapter_id)
        .replace("{sectionId}", section_id)
}

fn meta_to_exhibition_model(dir_slug: &str, meta: &IndexMeta, has_docs: bool) -> ExhibitionModel {
    let slug = meta
        .slug
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| dir_slug.to_string());

    let subtitle = if meta.subtitle.is_empty() {
        meta.title.clone()
    } else {
        meta.subtitle.clone()
    };

    let description = if meta.description.is_empty() {
        subtitle.clone()
    } else {
        meta.description.clone()
    };

    ExhibitionModel {
        id: String::new(),
        slug: slug.clone(),
        title: meta.title.clone(),
        subtitle,
        description,
        category_id: String::new(),
        category_name: meta.category.clone(),
        category: meta.category.clone(),
        tags: meta.tags.clone(),
        repo_url: opt_str(meta.repo_url.clone()),
        demo_url: opt_str(meta.demo_url.clone()),
        docs_slug: if has_docs {
            Some(dir_slug.to_string())
        } else {
            None
        },
        featured: meta.featured,
        sort_order: meta.sort_order,
        logo_url: None, // 由 DocService.logo_public_url 在 load_portfolio_item 中补填
        created_at: 0,
        updated_at: 0,
    }
}

fn humanize_slug(s: &str) -> String {
    s.replace('-', " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn stem_title(filename: &str) -> String {
    let stem = filename.strip_suffix(".md").unwrap_or(filename);
    humanize_slug(stem)
}

fn title_from_markdown(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines().take(10) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_V2: &str = r#"{
  "meta": {
    "title": "Demo",
    "docTitle": "Demo Docs",
    "pathRules": {
      "chapterIndex": "{chapterId}/INDEX.md",
      "sectionFile": "{chapterId}/{sectionId}.md"
    }
  },
  "parts": [
    {
      "title": "Part 1",
      "chapters": [
        {
          "id": "01-intro",
          "title": "Intro",
          "sections": [{ "id": "hello", "title": "Hello" }]
        }
      ]
    }
  ]
}"#;

    #[test]
    fn parse_index_accepts_utf8_bom() {
        let with_bom = format!("\u{FEFF}{}", SAMPLE_V2);
        let index = parse_index_json(&with_bom).expect("BOM should be stripped");
        assert_eq!(index.title, "Demo Docs");
        assert!(!index.items.is_empty());
    }

    #[test]
    fn parse_index_rejects_empty() {
        let err = parse_index_json("\u{FEFF}  \n").unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn parse_index_hints_html() {
        let err = parse_index_json("<!DOCTYPE html>").unwrap_err();
        assert!(err.contains("HTML"), "{err}");
    }

    #[test]
    fn work_dir_prefers_deploy_mirror() {
        // app_base() in tests may vary; just ensure sibling mapping is defined.
        assert_eq!(
            DocService::sibling_doc_relative("rust-webx"),
            Some("rust-webx/docs/rust-webx")
        );
        assert!(DocService::sibling_doc_relative("unknown").is_none());
    }
}
