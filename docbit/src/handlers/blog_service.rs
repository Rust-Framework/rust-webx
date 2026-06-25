//! User blog filesystem service — `blog-data/{user_id}/INDEX.json` + markdown posts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::common::bootstrap::AppPaths;
use crate::contracts::blog::{
    BlogCategoryCount, BlogCategoryDef, BlogPostModel, BlogPostSummary, IBlogService,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlogUserIndex {
    pub title: String,
    pub posts: Vec<BlogPostMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BlogPostMeta {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub category: String,
    pub tags: Vec<String>,
    pub published_at: String,
    pub path: String,
    pub author_id: String,
    pub author_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BlogCategoriesRegistry {
    #[serde(default)]
    items: Vec<BlogCategoryDef>,
}

#[rust_dicore::inject_attr(singleton, as = dyn IBlogService)]
pub struct BlogService {
    paths: Arc<AppPaths>,
}

impl BlogService {
    fn root(&self) -> &Path {
        &self.paths.blog_root
    }

    fn user_dir(&self, user_id: &str) -> PathBuf {
        self.root().join(sanitize_user_id(user_id))
    }
}

impl IBlogService for BlogService {
    fn list_all_posts(&self) -> Result<Vec<BlogPostSummary>, String> {
        let mut posts = Vec::new();
        if !self.root().is_dir() {
            return Ok(posts);
        }
        for entry in fs::read_dir(self.root()).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let user_id = entry.file_name().to_string_lossy().to_string();
            let index = self.load_index(&user_id)?;
            posts.extend(index.posts.into_iter().map(BlogPostSummary::from));
        }
        posts.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        Ok(posts)
    }

    fn list_user_posts(&self, user_id: &str) -> Result<Vec<BlogPostSummary>, String> {
        let index = self.load_index(user_id)?;
        let mut posts: Vec<_> = index.posts.into_iter().map(BlogPostSummary::from).collect();
        posts.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        Ok(posts)
    }

    fn list_categories(&self) -> Result<Vec<BlogCategoryCount>, String> {
        let posts = self.list_all_posts()?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for post in &posts {
            *counts.entry(post.category.clone()).or_insert(0) += 1;
        }

        let mut names: HashMap<String, String> = self
            .load_category_registry()?
            .items
            .into_iter()
            .map(|c| (c.id.clone(), c.name))
            .collect();

        for id in counts.keys() {
            names.entry(id.clone()).or_insert_with(|| blog_category_label(&id));
        }

        let mut categories: Vec<_> = names
            .into_iter()
            .map(|(id, name)| BlogCategoryCount {
                name,
                count: counts.get(&id).copied().unwrap_or(0),
                id,
            })
            .collect();
        categories.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(categories)
    }

    fn upsert_category(&self, id: &str, name: &str) -> Result<BlogCategoryDef, String> {
        validate_slug(id)?;
        let name = name.trim();
        if name.is_empty() {
            return Err("Category name cannot be empty".into());
        }

        let mut registry = self.load_category_registry().unwrap_or_default();
        if let Some(item) = registry.items.iter_mut().find(|c| c.id == id) {
            item.name = name.to_string();
        } else {
            registry.items.push(BlogCategoryDef {
                id: id.to_string(),
                name: name.to_string(),
            });
        }
        registry.items.sort_by(|a, b| a.id.cmp(&b.id));
        self.save_category_registry(&registry)?;
        Ok(BlogCategoryDef {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    fn get_post(&self, slug: &str) -> Result<BlogPostModel, String> {
        let (user_id, meta) = self
            .find_post_meta(slug)?
            .ok_or_else(|| format!("Blog post not found: {}", slug))?;
        let content = self.read_post_content(&user_id, &meta.path)?;
        Ok(BlogPostModel {
            id: format!("{}:{}", user_id, meta.slug),
            slug: meta.slug,
            title: meta.title,
            summary: meta.summary,
            content,
            tags: meta.tags,
            category: meta.category,
            published_at: meta.published_at.clone(),
            created_at: meta.published_at,
            author_id: meta.author_id,
            author_name: meta.author_name,
        })
    }

    fn create_post(
        &self,
        user_id: &str,
        user_name: &str,
        slug: &str,
        title: &str,
        summary: &str,
        content: &str,
        tags: &[String],
        category: &str,
        published_at: &str,
    ) -> Result<BlogPostModel, String> {
        validate_slug(slug)?;
        if self.find_post_meta(slug)?.is_some() {
            return Err(format!("Slug already exists: {}", slug));
        }

        let dir = self.ensure_user_dir(user_id, user_name)?;
        let file_name = format!("{}.md", slug);
        let file_path = dir.join(&file_name);
        fs::write(&file_path, content).map_err(|e| e.to_string())?;

        let mut index = self.load_index(user_id).unwrap_or(BlogUserIndex {
            title: user_name.to_string(),
            posts: vec![],
        });
        let meta = BlogPostMeta {
            slug: slug.to_string(),
            title: title.to_string(),
            summary: summary.to_string(),
            category: category.to_string(),
            tags: tags.to_vec(),
            published_at: published_at.to_string(),
            path: file_name,
            author_id: user_id.to_string(),
            author_name: user_name.to_string(),
        };
        index.posts.push(meta.clone());
        self.save_index(user_id, &index)?;

        Ok(BlogPostModel {
            id: format!("{}:{}", user_id, slug),
            slug: slug.to_string(),
            title: title.to_string(),
            summary: summary.to_string(),
            content: content.to_string(),
            tags: tags.to_vec(),
            category: category.to_string(),
            published_at: published_at.to_string(),
            created_at: published_at.to_string(),
            author_id: user_id.to_string(),
            author_name: user_name.to_string(),
        })
    }

    fn update_post(
        &self,
        actor_id: &str,
        actor_role: &str,
        slug: &str,
        title: Option<&str>,
        summary: Option<&str>,
        content: Option<&str>,
        tags: Option<&[String]>,
        category: Option<&str>,
        published_at: Option<&str>,
    ) -> Result<BlogPostModel, String> {
        let (owner_id, _) = self
            .find_post_meta(slug)?
            .ok_or_else(|| format!("Blog post not found: {}", slug))?;
        if actor_role != "admin" && actor_id != owner_id {
            return Err("Forbidden".into());
        }

        let mut index = self.load_index(&owner_id)?;
        let pos = index
            .posts
            .iter()
            .position(|p| p.slug == slug)
            .ok_or_else(|| format!("Blog post not found: {}", slug))?;
        let meta = &mut index.posts[pos];

        if let Some(t) = title {
            meta.title = t.to_string();
        }
        if let Some(s) = summary {
            meta.summary = s.to_string();
        }
        if let Some(c) = category {
            meta.category = c.to_string();
        }
        if let Some(t) = tags {
            meta.tags = t.to_vec();
        }
        if let Some(p) = published_at {
            meta.published_at = p.to_string();
        }

        if let Some(body) = content {
            let file_path = self.user_dir(&owner_id).join(&meta.path);
            fs::write(&file_path, body).map_err(|e| e.to_string())?;
        }

        self.save_index(&owner_id, &index)?;
        self.get_post(slug)
    }

    fn delete_post(&self, actor_id: &str, actor_role: &str, slug: &str) -> Result<(), String> {
        let (owner_id, meta) = self
            .find_post_meta(slug)?
            .ok_or_else(|| format!("Blog post not found: {}", slug))?;
        if actor_role != "admin" && actor_id != owner_id {
            return Err("Forbidden".into());
        }

        let mut index = self.load_index(&owner_id)?;
        index.posts.retain(|p| p.slug != slug);
        self.save_index(&owner_id, &index)?;

        let file_path = self.user_dir(&owner_id).join(&meta.path);
        if file_path.is_file() {
            let _ = fs::remove_file(file_path);
        }
        Ok(())
    }
}

impl BlogService {
    fn categories_registry_path(&self) -> PathBuf {
        self.root().join("categories.json")
    }

    fn load_category_registry(&self) -> Result<BlogCategoriesRegistry, String> {
        let path = self.categories_registry_path();
        if !path.is_file() {
            return Ok(BlogCategoriesRegistry::default());
        }
        let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    }

    fn save_category_registry(&self, registry: &BlogCategoriesRegistry) -> Result<(), String> {
        fs::create_dir_all(self.root()).map_err(|e| e.to_string())?;
        let raw = serde_json::to_string_pretty(registry).map_err(|e| e.to_string())?;
        fs::write(self.categories_registry_path(), raw).map_err(|e| e.to_string())
    }

    fn ensure_user_dir(&self, user_id: &str, user_name: &str) -> Result<PathBuf, String> {
        fs::create_dir_all(self.root()).map_err(|e| e.to_string())?;
        let dir = self.user_dir(user_id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let index_path = dir.join("INDEX.json");
        if !index_path.exists() {
            let index = BlogUserIndex {
                title: user_name.to_string(),
                posts: vec![],
            };
            self.save_index(user_id, &index)?;
        }
        Ok(dir)
    }

    fn load_index(&self, user_id: &str) -> Result<BlogUserIndex, String> {
        let path = self.user_dir(user_id).join("INDEX.json");
        if !path.is_file() {
            return Err(format!("Blog index not found for user '{}'", user_id));
        }
        let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    }

    fn save_index(&self, user_id: &str, index: &BlogUserIndex) -> Result<(), String> {
        let dir = self.user_dir(user_id);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let raw = serde_json::to_string_pretty(index).map_err(|e| e.to_string())?;
        fs::write(dir.join("INDEX.json"), raw).map_err(|e| e.to_string())
    }

    fn read_post_content(&self, user_id: &str, path: &str) -> Result<String, String> {
        let dir = self.user_dir(user_id);
        let normalized = path.trim_start_matches('/');
        let file_path = dir.join(normalized);
        if !file_path.starts_with(&dir) {
            return Err("Invalid post path".into());
        }
        fs::read_to_string(&file_path).map_err(|e| e.to_string())
    }

    fn find_post_meta(&self, slug: &str) -> Result<Option<(String, BlogPostMeta)>, String> {
        if !self.root().is_dir() {
            return Ok(None);
        }
        for entry in fs::read_dir(self.root()).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let user_id = entry.file_name().to_string_lossy().to_string();
            let index = match self.load_index(&user_id) {
                Ok(i) => i,
                Err(_) => continue,
            };
            if let Some(meta) = index.posts.into_iter().find(|p| p.slug == slug) {
                return Ok(Some((user_id, meta)));
            }
        }
        Ok(None)
    }
}

impl From<BlogPostMeta> for BlogPostSummary {
    fn from(m: BlogPostMeta) -> Self {
        Self {
            id: format!("{}:{}", m.author_id, m.slug),
            slug: m.slug,
            title: m.title,
            summary: m.summary,
            tags: m.tags,
            category: m.category,
            published_at: m.published_at,
            author_id: m.author_id,
            author_name: m.author_name,
        }
    }
}

fn blog_category_label(id: &str) -> String {
    match id {
        "rust" => "Rust 生态".into(),
        "webapi" => "Web 开发".into(),
        "tutorial" => "教程实践".into(),
        "portfolio" => "作品集".into(),
        "news" => "动态资讯".into(),
        _ => id.to_string(),
    }
}

fn sanitize_user_id(user_id: &str) -> String {
    user_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn validate_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty() || slug.len() > 120 {
        return Err("Invalid slug".into());
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Slug may only contain letters, numbers, hyphens and underscores".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_slug_accepts_hyphens() {
        assert!(validate_slug("hello-world").is_ok());
    }
}
