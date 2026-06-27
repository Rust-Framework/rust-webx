//! Documentation filesystem service — scans `docs/` and serves INDEX.json + markdown.
//!
//! 实现合约层的 `IDocumentService`。需要文件系统访问与 `AppPaths`，
//! 因此放在 host crate 而非 handlers crate。
//!
//! `list_portfolio` / `get_portfolio` 从文件系统 INDEX.json 读取元数据并
//! 返回 `ExhibitionModel`；DB 专属字段（id、category_id、created_at 等）
//! 填占位值，运行时实际展示用 exhibition handlers 从 DB 取。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use docbit_contracts::docs::{DocContent, DocIndex, DocIndexItem, IDocumentService};
use docbit_contracts::exhibition::ExhibitionModel;

use crate::paths::AppPaths;

#[rust_dicore::inject_attr(singleton, as = dyn IDocumentService)]
pub struct DocService {
    paths: Arc<AppPaths>,
}

impl DocService {
    fn root(&self) -> &Path {
        &self.paths.docs_root
    }

    fn work_dir(&self, work: &str) -> PathBuf {
        self.root().join(work)
    }

    fn write_index(&self, work: &str, index: &DocIndex) -> Result<(), String> {
        let path = self.work_dir(work).join("INDEX.json");
        let json = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
        fs::write(path, json).map_err(|e| e.to_string())
    }

    fn build_index(&self, work: &str) -> Result<DocIndex, String> {
        let dir = self.work_dir(work);
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
        let raw = fs::read_to_string(self.work_dir(dir_slug).join("INDEX.json"))
            .map_err(|e| e.to_string())?;

        if let Ok(root) = serde_json::from_str::<IndexRootV2>(&raw) {
            return Ok(meta_to_exhibition_model(dir_slug, &root.meta, !root.parts.is_empty()));
        }

        let legacy: DocIndex =
            serde_json::from_str(&raw).map_err(|e| format!("Invalid INDEX.json: {}", e))?;
        Ok(ExhibitionModel {
            id: 0,
            slug: dir_slug.to_string(),
            title: legacy.title,
            subtitle: String::new(),
            description: String::new(),
            category_id: 0,
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
        let path = self.work_dir(dir_slug).join(logo_name);
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
        let path = self.work_dir(dir_slug).join(logo_name);
        if path.is_file() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }
}

impl IDocumentService for DocService {
    fn list_works(&self) -> Result<Vec<String>, String> {
        if !self.root().is_dir() {
            return Ok(vec![]);
        }
        let mut works = Vec::new();
        for entry in fs::read_dir(self.root()).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                works.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        works.sort();
        Ok(works)
    }

    fn index(&self, work: &str) -> Result<DocIndex, String> {
        let dir = self.work_dir(work);
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
        let dir = self.work_dir(work);
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
        if !self.root().is_dir() {
            return Err(format!(
                "Docs directory not found: {}",
                self.root().display()
            ));
        }
        for entry in fs::read_dir(self.root()).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let work = entry.file_name().to_string_lossy().to_string();
            let index_path = entry.path().join("INDEX.json");
            if index_path.exists() {
                tracing::info!("[DocService] Using existing INDEX.json for '{}'", work);
            } else {
                let generated = self.build_index(&work)?;
                self.write_index(&work, &generated)?;
                tracing::info!("[DocService] Generated INDEX.json for '{}'", work);
            }
        }
        Ok(())
    }

    fn list_portfolio(&self) -> Result<Vec<ExhibitionModel>, String> {
        let mut items = Vec::new();
        for slug in self.list_works()? {
            let index_path = self.work_dir(&slug).join("INDEX.json");
            if !index_path.is_file() {
                continue;
            }
            items.push(self.load_portfolio_item(&slug)?);
        }
        items.sort_by_key(|w| (w.sort_order, w.slug.clone()));
        Ok(items)
    }

    fn get_portfolio(&self, slug: &str) -> Result<ExhibitionModel, String> {
        let index_path = self.work_dir(slug).join("INDEX.json");
        if !index_path.is_file() {
            return Err(format!("Portfolio item not found: {}", slug));
        }
        self.load_portfolio_item(slug)
    }

    fn sync_portfolio_assets(&self, wwwroot: &Path) -> Result<(), String> {
        let dest_dir = wwwroot.join("assets/works");
        fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;

        for slug in self.list_works()? {
            let index_path = self.work_dir(&slug).join("INDEX.json");
            if !index_path.is_file() {
                continue;
            }
            let raw = fs::read_to_string(&index_path).map_err(|e| e.to_string())?;
            let logo_name = serde_json::from_str::<IndexRootV2>(&raw)
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

fn parse_index_json(raw: &str) -> Result<DocIndex, String> {
    if let Ok(root) = serde_json::from_str::<IndexRootV2>(raw) {
        return Ok(expand_index_v2(root));
    }
    serde_json::from_str(raw).map_err(|e| format!("Invalid INDEX.json: {}", e))
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
        id: 0,
        slug: slug.clone(),
        title: meta.title.clone(),
        subtitle,
        description,
        category_id: 0,
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
