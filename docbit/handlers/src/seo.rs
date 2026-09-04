//! Server-side HTML shell + robots/sitemap for crawlable public pages.
//!
//! Runs before `SpaMiddleware` (DI `IMiddleware` registration order) and
//! short-circuits GET requests for public HTML routes with a full document that
//! includes title/description/Open Graph tags and visible body content. SPA
//! scripts remain so browsers hydrate into the normal client app.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Arc;

use rust_ef::{db_context::DbContext, prelude::*};
use rust_webx::*;

use docbit_contracts::docs::{DocIndexItem, IDocumentService};
use docbit_contracts::exhibition::ExhibitionModel;
use docbit_contracts::site::SiteConfig;
use docbit_domain::entities::Blog;
use docbit_domain::prepare_context;

use crate::db::EfResultExt;

#[derive(Inject)]
pub struct SeoHtmlMiddleware {
    #[inject]
    docs: Arc<dyn IDocumentService>,
    #[inject]
    site: Arc<SiteConfig>,
}

#[inject]
#[async_trait]
impl IMiddleware for SeoHtmlMiddleware {
    async fn invoke(&self, ctx: &mut dyn IHttpContext) -> Result<ControlFlow<()>> {
        if ctx.request().method().to_uppercase() != "GET" {
            return Ok(ControlFlow::Continue(()));
        }

        let path = ctx.request().path().to_string();
        if should_skip(&path) {
            return Ok(ControlFlow::Continue(()));
        }

        if path == "/robots.txt" {
            return write_text(ctx, "text/plain; charset=utf-8", robots_txt(&self.site)).await;
        }
        if path == "/sitemap.xml" {
            let body = self.build_sitemap().await;
            return write_text(ctx, "application/xml; charset=utf-8", body).await;
        }

        if !wants_html(ctx) {
            return Ok(ControlFlow::Continue(()));
        }

        let Some(page) = parse_public_page(&path) else {
            return Ok(ControlFlow::Continue(()));
        };

        let html = self.render_page(&page).await;
        write_text(ctx, "text/html; charset=utf-8", html).await
    }
}

fn should_skip(path: &str) -> bool {
    path.starts_with("/api/")
        || path.starts_with("/assets/")
        || path.starts_with("/admin")
        || path == "/metrics"
        || path == "/health"
        || path == "/openapi.json"
        || path == "/swagger"
}

fn wants_html(ctx: &dyn IHttpContext) -> bool {
    let accept = ctx.request().header("accept").unwrap_or("*/*");
    if accept.contains("application/json") && !accept.contains("text/html") {
        return false;
    }
    accept.contains("text/html") || accept.contains("*/*") || accept.is_empty()
}

#[derive(Debug)]
enum PublicPage {
    Home,
    About,
    Work {
        slug: String,
    },
    Docs {
        slug: String,
        doc_path: Option<String>,
    },
    BlogList,
    BlogPost {
        slug: String,
    },
}

fn parse_public_page(path: &str) -> Option<PublicPage> {
    let trimmed = path.trim_end_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Some(PublicPage::Home);
    }
    match parts.as_slice() {
        ["about"] => Some(PublicPage::About),
        ["blog"] => Some(PublicPage::BlogList),
        ["blog", slug] => Some(PublicPage::BlogPost {
            slug: decode_seg(slug),
        }),
        ["works", slug] => Some(PublicPage::Work {
            slug: decode_seg(slug),
        }),
        ["works", slug, "docs"] => Some(PublicPage::Docs {
            slug: decode_seg(slug),
            doc_path: None,
        }),
        ["works", slug, "docs", rest @ ..] if !rest.is_empty() => Some(PublicPage::Docs {
            slug: decode_seg(slug),
            doc_path: Some(
                rest.iter()
                    .map(|s| decode_seg(s))
                    .collect::<Vec<_>>()
                    .join("/"),
            ),
        }),
        _ => None,
    }
}

fn decode_seg(s: &str) -> String {
    percent_decode(s)
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

impl SeoHtmlMiddleware {
    fn site_origin(&self) -> String {
        let raw = self.site.footer.site_url.trim();
        if raw.is_empty() {
            return "https://www.lusida.net".into();
        }
        if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.trim_end_matches('/').to_string()
        } else {
            format!("https://{}", raw.trim_end_matches('/'))
        }
    }

    async fn render_page(&self, page: &PublicPage) -> String {
        let origin = self.site_origin();
        let brand = if self.site.brand_name.is_empty() {
            self.site.title.as_str()
        } else {
            self.site.brand_name.as_str()
        };

        let (title, description, canonical, body, body_class) = match page {
            PublicPage::Home => {
                let works = self.docs.list_portfolio().unwrap_or_default();
                let desc = if self.site.bio.is_empty() {
                    self.site.tagline.clone()
                } else {
                    self.site.bio.clone()
                };
                let mut cards = String::new();
                for w in &works {
                    cards.push_str(&format!(
                        r#"<article class="work-card-v2"><h2><a href="/works/{slug}">{title}</a></h2><p>{desc}</p></article>"#,
                        slug = escape_attr(&w.slug),
                        title = escape_html(&w.title),
                        desc = escape_html(&w.description),
                    ));
                }
                (
                    format!("{} — 作品集", brand),
                    desc,
                    format!("{origin}/"),
                    format!(
                        r#"<div class="page page-home" data-ssr="1"><h1>{brand}</h1><p>{tag}</p><section><h2>精选作品</h2><div class="works-grid">{cards}</div></section></div>"#,
                        brand = escape_html(brand),
                        tag = escape_html(&self.site.tagline),
                        cards = cards,
                    ),
                    "layout-home",
                )
            }
            PublicPage::About => (
                format!("关于 — {brand}"),
                self.site.bio.clone(),
                format!("{origin}/about"),
                format!(
                    r#"<div class="page" data-ssr="1"><h1>关于</h1><p>{bio}</p><p>{author}</p></div>"#,
                    bio = escape_html(&self.site.bio),
                    author = escape_html(&self.site.author),
                ),
                "",
            ),
            PublicPage::Work { slug } => {
                let work = self.resolve_work(slug);
                let title = work
                    .as_ref()
                    .map(|w| format!("{} — 作品", w.title))
                    .unwrap_or_else(|| format!("{slug} — 作品"));
                let desc = work
                    .as_ref()
                    .map(|w| w.description.clone())
                    .unwrap_or_default();
                let body = match &work {
                    Some(w) => format!(
                        r#"<div class="page page-work" data-ssr="1"><h1>{title}</h1><p class="meta">{sub}</p><p>{desc}</p>{docs}</div>"#,
                        title = escape_html(&w.title),
                        sub = escape_html(&w.subtitle),
                        desc = escape_html(&w.description),
                        docs = if w.docs_slug.as_ref().filter(|s| !s.is_empty()).is_some() {
                            format!(
                                r#"<p><a href="/works/{}/docs">查看文档</a></p>"#,
                                escape_attr(&w.slug)
                            )
                        } else {
                            String::new()
                        },
                    ),
                    None => format!(
                        r#"<div class="page" data-ssr="1"><h1>作品未找到</h1><p>{}</p></div>"#,
                        escape_html(slug)
                    ),
                };
                (
                    title,
                    desc,
                    format!("{origin}/works/{}", url_encode_path(slug)),
                    body,
                    "",
                )
            }
            PublicPage::Docs { slug, doc_path } => {
                let work = self.resolve_work(slug);
                let docs_slug = work
                    .as_ref()
                    .and_then(|w| w.docs_slug.clone())
                    .unwrap_or_else(|| slug.clone());
                let index = self.docs.index(&docs_slug).ok();
                let resolved_path = doc_path
                    .clone()
                    .or_else(|| index.as_ref().and_then(first_doc_path));
                let content = resolved_path
                    .as_ref()
                    .and_then(|p| self.docs.content(&docs_slug, p).ok());
                let work_title = work
                    .as_ref()
                    .map(|w| w.title.as_str())
                    .unwrap_or(slug.as_str());
                let page_title = content
                    .as_ref()
                    .and_then(|c| first_markdown_heading(&c.content))
                    .unwrap_or_else(|| format!("{work_title} — 文档"));
                let desc = work
                    .as_ref()
                    .map(|w| w.description.clone())
                    .unwrap_or_else(|| format!("{work_title} 文档"));
                let nav = index
                    .as_ref()
                    .map(|idx| render_doc_nav_html(&idx.items, slug, resolved_path.as_deref()))
                    .unwrap_or_default();
                let article = content
                    .as_ref()
                    .map(|c| simple_md_html(&c.content))
                    .unwrap_or_else(|| "<p>文档暂不可用</p>".into());
                let canon = match &resolved_path {
                    Some(p) => format!(
                        "{origin}/works/{}/docs/{}",
                        url_encode_path(slug),
                        url_encode_doc_path(p)
                    ),
                    None => format!("{origin}/works/{}/docs", url_encode_path(slug)),
                };
                (
                    page_title,
                    desc,
                    canon,
                    format!(
                        r#"<div class="content-shell page-docs" data-ssr="1"><aside><nav><ul class="doc-nav">{nav}</ul></nav></aside><main><article class="markdown-body">{article}</article></main></div>"#,
                        nav = nav,
                        article = article,
                    ),
                    "layout-shell",
                )
            }
            PublicPage::BlogList => {
                let posts = self.list_blog_summaries().await;
                let mut list = String::new();
                for p in &posts {
                    list.push_str(&format!(
                        r#"<article><h2><a href="/blog/{slug}">{title}</a></h2><p>{sum}</p></article>"#,
                        slug = escape_attr(&p.0),
                        title = escape_html(&p.1),
                        sum = escape_html(&p.2),
                    ));
                }
                (
                    format!("博客 — {brand}"),
                    format!("{brand} 的技术博客"),
                    format!("{origin}/blog"),
                    format!(
                        r#"<div class="page" data-ssr="1"><h1>博客</h1>{list}</div>"#,
                        list = list
                    ),
                    "layout-shell",
                )
            }
            PublicPage::BlogPost { slug } => {
                let post = self.get_blog(slug).await;
                match post {
                    Some((title, summary, content)) => (
                        format!("{title} — 博客"),
                        summary.clone(),
                        format!("{origin}/blog/{}", url_encode_path(slug)),
                        format!(
                            r#"<div class="page" data-ssr="1"><article><h1>{title}</h1><p>{sum}</p><div class="markdown-body">{body}</div></article></div>"#,
                            title = escape_html(&title),
                            sum = escape_html(&summary),
                            body = simple_md_html(&content),
                        ),
                        "layout-shell",
                    ),
                    None => (
                        format!("文章未找到 — {brand}"),
                        String::new(),
                        format!("{origin}/blog/{}", url_encode_path(slug)),
                        format!(
                            r#"<div class="page" data-ssr="1"><h1>文章未找到</h1><p>{}</p></div>"#,
                            escape_html(slug)
                        ),
                        "layout-shell",
                    ),
                }
            }
        };

        assemble_html(
            &title,
            &description,
            &canonical,
            brand,
            &body,
            body_class,
            &self.load_shell_template(),
        )
    }

    fn resolve_work(&self, slug: &str) -> Option<ExhibitionModel> {
        self.docs.get_portfolio(slug).ok().or_else(|| {
            self.docs
                .list_portfolio()
                .ok()
                .into_iter()
                .flatten()
                .find(|w| w.slug == slug)
        })
    }

    async fn list_blog_summaries(&self) -> Vec<(String, String, String)> {
        let Ok(mut ctx) = owned_db() else {
            return Vec::new();
        };
        let Ok(blogs) = linq!(ctx.set::<Blog>(); order_by b.published_at desc)
            .to_list()
            .await
            .map_ef()
        else {
            return Vec::new();
        };
        blogs
            .into_iter()
            .map(|b| (b.slug, b.title, b.summary))
            .collect()
    }

    async fn get_blog(&self, slug: &str) -> Option<(String, String, String)> {
        let mut ctx = owned_db().ok()?;
        let q = slug.to_string();
        let blog = linq!(ctx.set::<Blog>(), |b: Blog| b.slug == q)
            .first_or_default()
            .await
            .ok()??;
        Some((blog.title, blog.summary, blog.content))
    }

    async fn build_sitemap(&self) -> String {
        let origin = self.site_origin();
        let mut urls = vec![
            format!("{origin}/"),
            format!("{origin}/about"),
            format!("{origin}/blog"),
        ];

        if let Ok(works) = self.docs.list_portfolio() {
            for w in works {
                urls.push(format!("{origin}/works/{}", url_encode_path(&w.slug)));
                let docs_slug = w.docs_slug.as_deref().unwrap_or(&w.slug);
                if let Ok(index) = self.docs.index(docs_slug) {
                    urls.push(format!("{origin}/works/{}/docs", url_encode_path(&w.slug)));
                    collect_doc_urls(&origin, &w.slug, &index.items, &mut urls);
                }
            }
        }

        for (slug, _, _) in self.list_blog_summaries().await {
            urls.push(format!("{origin}/blog/{}", url_encode_path(&slug)));
        }

        let mut out = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        out.push_str(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
        for u in urls {
            out.push_str("<url><loc>");
            out.push_str(&escape_xml(&u));
            out.push_str("</loc></url>");
        }
        out.push_str("</urlset>");
        out
    }

    fn load_shell_template(&self) -> String {
        let path = spa_index_path();
        std::fs::read_to_string(&path).unwrap_or_else(|_| FALLBACK_SHELL.to_string())
    }
}

fn owned_db() -> Result<DbContext> {
    let mut ctx: DbContext = dispatch_provider()
        .get_owned()
        .map_err(|e| Error::Internal(format!("DbContext resolution failed: {e}")))?;
    prepare_context(&mut ctx);
    Ok(ctx)
}

fn spa_index_path() -> PathBuf {
    app_base().join("wwwroot").join("index.html")
}

fn robots_txt(site: &SiteConfig) -> String {
    let origin = {
        let raw = site.footer.site_url.trim();
        if raw.is_empty() {
            "https://www.lusida.net".into()
        } else if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.trim_end_matches('/').to_string()
        } else {
            format!("https://{}", raw.trim_end_matches('/'))
        }
    };
    format!(
        "User-agent: *\nAllow: /\nDisallow: /api/\nDisallow: /admin\nDisallow: /login\nDisallow: /register\nSitemap: {origin}/sitemap.xml\n"
    )
}

fn collect_doc_urls(origin: &str, work_slug: &str, items: &[DocIndexItem], urls: &mut Vec<String>) {
    for item in items {
        if let Some(ref path) = item.path {
            urls.push(format!(
                "{origin}/works/{}/docs/{}",
                url_encode_path(work_slug),
                url_encode_doc_path(path)
            ));
        }
        if let Some(ref children) = item.children {
            collect_doc_urls(origin, work_slug, children, urls);
        }
    }
}

fn first_doc_path(index: &docbit_contracts::docs::DocIndex) -> Option<String> {
    fn walk(items: &[DocIndexItem]) -> Option<String> {
        for item in items {
            if let Some(ref p) = item.path {
                return Some(p.clone());
            }
            if let Some(ref children) = item.children {
                if let Some(p) = walk(children) {
                    return Some(p);
                }
            }
        }
        None
    }
    walk(&index.items)
}

fn render_doc_nav_html(items: &[DocIndexItem], work_slug: &str, active: Option<&str>) -> String {
    let mut out = String::new();
    for item in items {
        if let Some(ref path) = item.path {
            let active_cls = if active == Some(path.as_str()) {
                " active"
            } else {
                ""
            };
            out.push_str(&format!(
                r#"<li><a href="/works/{}/docs/{}" class="{}">{}</a></li>"#,
                escape_attr(work_slug),
                escape_attr(&url_encode_doc_path(path)),
                active_cls.trim(),
                escape_html(&item.title),
            ));
        } else if let Some(ref children) = item.children {
            out.push_str(&format!(
                "<li><span>{}</span><ul>{}</ul></li>",
                escape_html(&item.title),
                render_doc_nav_html(children, work_slug, active)
            ));
        }
    }
    out
}

fn assemble_html(
    title: &str,
    description: &str,
    canonical: &str,
    brand: &str,
    body: &str,
    body_class: &str,
    template: &str,
) -> String {
    let meta = format!(
        r#"
        <title>{title}</title>
        <meta name="description" content="{desc}" />
        <link rel="canonical" href="{canon}" />
        <meta property="og:type" content="website" />
        <meta property="og:title" content="{title}" />
        <meta property="og:description" content="{desc}" />
        <meta property="og:url" content="{canon}" />
        <meta property="og:site_name" content="{brand}" />
        <meta name="twitter:card" content="summary" />
        <meta name="twitter:title" content="{title}" />
        <meta name="twitter:description" content="{desc}" />
"#,
        title = escape_html(title),
        desc = escape_attr(description),
        canon = escape_attr(canonical),
        brand = escape_attr(brand),
    );

    let mut html = template.to_string();
    if let Some(start) = html.find("<title>") {
        if let Some(end) = html[start..].find("</title>") {
            let end = start + end + "</title>".len();
            html.replace_range(start..end, meta.trim());
        }
    } else if let Some(head_end) = html.find("</head>") {
        html.insert_str(head_end, &meta);
    }

    if !body_class.is_empty() {
        if html.contains("<body>") {
            html = html.replacen("<body>", &format!(r#"<body class="{body_class}">"#), 1);
        } else if let Some(idx) = html.find("<body ") {
            if let Some(gt) = html[idx..].find('>') {
                let insert_at = idx + gt;
                let open = &html[idx..insert_at];
                if open.contains("class=\"") {
                    html = html.replacen("class=\"", &format!("class=\"{body_class} "), 1);
                } else {
                    html.insert_str(insert_at, &format!(r#" class="{body_class}""#));
                }
            }
        }
    }

    if let Some(app_start) = html.find(r#"id="app""#) {
        if let Some(open_end) = html[app_start..].find('>') {
            let content_start = app_start + open_end + 1;
            if let Some(close_rel) = html[content_start..].find("</main>") {
                let content_end = content_start + close_rel;
                html.replace_range(content_start..content_end, body);
            }
        }
    }

    html
}

async fn write_text(
    ctx: &mut dyn IHttpContext,
    content_type: &str,
    body: String,
) -> Result<ControlFlow<()>> {
    ctx.response_mut().set_status(HttpStatus::OK);
    ctx.response_mut().set_header("content-type", content_type);
    ctx.response_mut()
        .set_header("cache-control", "public, max-age=60");
    ctx.response_mut().write_bytes(body.into_bytes()).await?;
    Ok(ControlFlow::Break(()))
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_attr(s: &str) -> String {
    escape_html(s).replace('\'', "&#39;")
}

fn escape_xml(s: &str) -> String {
    escape_html(s).replace('\'', "&apos;")
}

fn url_encode_path(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn url_encode_doc_path(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(url_encode_path)
        .collect::<Vec<_>>()
        .join("/")
}

fn first_markdown_heading(md: &str) -> Option<String> {
    for line in md.lines().take(20) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn simple_md_html(md: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    let mut para = String::new();

    let flush_para = |out: &mut String, para: &mut String| {
        if !para.is_empty() {
            out.push_str("<p>");
            out.push_str(&escape_html(para.trim()));
            out.push_str("</p>");
            para.clear();
        }
    };

    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            flush_para(&mut out, &mut para);
            if in_code {
                out.push_str("</code></pre>");
                in_code = false;
            } else {
                out.push_str("<pre><code>");
                in_code = true;
            }
            continue;
        }
        if in_code {
            out.push_str(&escape_html(line));
            out.push('\n');
            continue;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            flush_para(&mut out, &mut para);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            flush_para(&mut out, &mut para);
            out.push_str(&format!("<h3>{}</h3>", escape_html(rest)));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            flush_para(&mut out, &mut para);
            out.push_str(&format!("<h2>{}</h2>", escape_html(rest)));
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            flush_para(&mut out, &mut para);
            out.push_str(&format!("<h1>{}</h1>", escape_html(rest)));
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(trimmed);
        }
    }
    flush_para(&mut out, &mut para);
    if in_code {
        out.push_str("</code></pre>");
    }
    out
}

const FALLBACK_SHELL: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="UTF-8" /><meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>Start World</title>
<link rel="stylesheet" href="/app.css" />
</head>
<body>
<header class="site-header"><div class="header-inner"><a href="/" class="brand">Start World</a>
<nav class="main-nav"><a href="/">作品</a><a href="/blog">博客</a><a href="/about">关于</a></nav></div></header>
<main id="app"></main>
<script src="/assets/js/core/utils.js"></script>
<script src="/assets/js/core/api.js"></script>
<script src="/assets/js/core/auth.js"></script>
<script src="/assets/js/core/theme.js"></script>
<script src="/assets/js/core/markdown.js"></script>
<script src="/assets/js/core/loader.js"></script>
<script src="/assets/js/core/ui.js"></script>
<script src="/assets/js/core/footer.js"></script>
<script src="/app.js"></script>
</body></html>"#;
