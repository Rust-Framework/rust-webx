//! Documentation filesystem service — scans `docs/` and serves INDEX.json + markdown.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocIndex {
    pub title: String,
    pub items: Vec<DocIndexItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocIndexItem {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<DocIndexItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocContent {
    pub path: String,
    pub content: String,
}

pub struct DocService {
    root: PathBuf,
}

impl DocService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn work_dir(&self, work: &str) -> PathBuf {
        self.root.join(work)
    }

    /// Load INDEX.json for a work, auto-generating it when missing.
    pub fn index(&self, work: &str) -> Result<DocIndex, String> {
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
        serde_json::from_str(&raw).map_err(|e| format!("Invalid INDEX.json: {}", e))
    }

    /// Read a markdown file relative to the work docs root.
    pub fn content(&self, work: &str, path: &str) -> Result<DocContent, String> {
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

    /// Scan all work directories; only generate INDEX.json when missing.
    pub fn ensure_all_indexes(&self) -> Result<(), String> {
        if !self.root.is_dir() {
            return Err(format!(
                "Docs directory not found: {}",
                self.root.display()
            ));
        }

        for entry in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
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

    pub fn list_works(&self) -> Result<Vec<String>, String> {
        if !self.root.is_dir() {
            return Ok(vec![]);
        }
        let mut works = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                works.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        works.sort();
        Ok(works)
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
