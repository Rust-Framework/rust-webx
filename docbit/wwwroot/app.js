/* =========================================================================
   Docbit — Portfolio SPA Client
   ========================================================================= */

(function () {
  "use strict";

  const API = "";

  // ── API ──

  async function api(path) {
    const res = await fetch(API + path);
    if (!res.ok) {
      const data = await res.json().catch(() => ({ error: res.statusText }));
      throw new Error(data.error || "Request failed");
    }
    return res.json();
  }

  // ── Router ──

  function parseRoute(path) {
    const parts = path.replace(/\/+$/, "").split("/").filter(Boolean);
    if (parts.length === 0) return { page: "home" };
    if (parts[0] === "blog") {
      return parts[1] ? { page: "blog-post", slug: parts[1] } : { page: "blog" };
    }
    if (parts[0] === "about") return { page: "about" };
    if (parts[0] === "works" && parts[1]) {
      if (parts[2] === "docs") {
        const docPath = parts.slice(3).join("/");
        return { page: "docs", slug: parts[1], docPath: docPath || null };
      }
      return { page: "work", slug: parts[1] };
    }
    return { page: "home" };
  }

  function navigate(path) {
    window.history.pushState(null, "", path);
    render();
  }

  // ── Bootstrap ──

  document.addEventListener("DOMContentLoaded", () => {
    document.addEventListener("click", (e) => {
      const link = e.target.closest("a[data-nav], a[href^='/']");
      if (!link) return;
      const href = link.getAttribute("href");
      if (!href || !href.startsWith("/") || href.startsWith("//")) return;
      if (link.getAttribute("target") === "_blank") return;
      e.preventDefault();
      navigate(href);
    });

    window.addEventListener("popstate", render);
    loadSite().then(render);
  });

  let siteInfo = null;

  async function loadSite() {
    try {
      siteInfo = await api("/api/site");
      const brand = document.getElementById("brand-name");
      if (brand && siteInfo.title) brand.textContent = siteInfo.title;
    } catch (_) {
      siteInfo = { title: "Docbit", tagline: "", author: "", bio: "", links: {} };
    }
  }

  function setActiveNav(page) {
    document.querySelectorAll(".main-nav a").forEach((a) => {
      const href = a.getAttribute("href");
      const active =
        (page === "home" && href === "/") ||
        (page.startsWith("work") && href === "/") ||
        (page.startsWith("docs") && href === "/") ||
        (page.startsWith("blog") && href === "/blog") ||
        (page === "about" && href === "/about");
      a.classList.toggle("active", active);
    });
  }

  function showLoading() {
    document.getElementById("app").innerHTML =
      '<div class="loading-state"><div class="spinner"></div></div>';
  }

  function showError(msg) {
    document.getElementById("app").innerHTML =
      `<div class="error-box"><strong>加载失败</strong><p>${escapeHtml(msg)}</p></div>`;
  }

  async function render() {
    const route = parseRoute(window.location.pathname);
    setActiveNav(route.page);
    showLoading();

    try {
      switch (route.page) {
        case "home": await renderHome(); break;
        case "work": await renderWork(route.slug); break;
        case "docs": await renderDocs(route.slug, route.docPath); break;
        case "blog": await renderBlogList(); break;
        case "blog-post": await renderBlogPost(route.slug); break;
        case "about": renderAbout(); break;
        default: await renderHome();
      }
    } catch (err) {
      showError(err.message);
    }
  }

  // ── Helpers ──

  function escapeHtml(str) {
    const div = document.createElement("div");
    div.textContent = str;
    return div.innerHTML;
  }

  function renderMarkdown(md) {
    if (typeof marked !== "undefined") {
      marked.setOptions({ gfm: true, breaks: false });
      return marked.parse(md);
    }
    return `<pre>${escapeHtml(md)}</pre>`;
  }

  function categoryBadge(cat) {
    const labels = { framework: "框架", product: "产品", article: "文章" };
    return `<span class="badge badge-${cat}">${labels[cat] || cat}</span>`;
  }

  function tagList(tags) {
    return (tags || []).map((t) => `<span class="tag">${escapeHtml(t)}</span>`).join("");
  }

  // ── Home ──

  async function renderHome() {
    const works = await api("/api/works");
    const featured = works.filter((w) => w.featured);
    const others = works.filter((w) => !w.featured);
    const site = siteInfo || {};

    const card = (w) => `
      <a class="work-card${w.featured ? " featured" : ""}" href="/works/${escapeHtml(w.slug)}" data-nav>
        <div class="work-card-header">
          <h3>${escapeHtml(w.title)}</h3>
          ${categoryBadge(w.category)}
        </div>
        <div class="subtitle">${escapeHtml(w.subtitle)}</div>
        <div class="desc">${escapeHtml(w.description)}</div>
        <div class="work-card-footer">${tagList(w.tags)}</div>
      </a>`;

    document.getElementById("app").innerHTML = `
      <div class="page">
        <section class="hero">
          <div class="hero-tag">${escapeHtml(site.tagline || "Developer Portfolio")}</div>
          <h1>${escapeHtml(site.title || "个人作品展")}</h1>
          <p>${escapeHtml(site.bio || "展示开源框架、实用产品与技术分享")}</p>
        </section>
        ${featured.length ? `
          <section style="margin-bottom:2.5rem">
            <h2 class="section-title">精选作品</h2>
            <div class="works-grid">${featured.map(card).join("")}</div>
          </section>` : ""}
        ${others.length ? `
          <section>
            <h2 class="section-title">更多作品</h2>
            <div class="works-grid">${others.map(card).join("")}</div>
          </section>` : ""}
        ${works.length === 0 ? '<p style="color:var(--text-muted)">暂无作品，请通过 API 添加。</p>' : ""}
      </div>`;
    document.title = (site.title || "Docbit") + " — 作品集";
  }

  // ── Work detail ──

  async function renderWork(slug) {
    const work = await api(`/api/works/${encodeURIComponent(slug)}`);
    const hasDocs = work.docs_slug && work.docs_slug.length > 0;

    document.getElementById("app").innerHTML = `
      <div class="page">
        <div class="detail-header">
          <div style="margin-bottom:0.75rem">${categoryBadge(work.category)} ${tagList(work.tags)}</div>
          <h1>${escapeHtml(work.title)}</h1>
          <div class="meta">${escapeHtml(work.subtitle)}</div>
        </div>
        <div class="detail-body">
          <p>${escapeHtml(work.description)}</p>
        </div>
        <div class="btn-row">
          ${hasDocs ? `<a class="btn btn-primary" href="/works/${escapeHtml(work.slug)}/docs" data-nav>📖 查看文档</a>` : ""}
          ${work.repo_url ? `<a class="btn" href="${escapeHtml(work.repo_url)}" target="_blank" rel="noopener">源码仓库</a>` : ""}
          ${work.demo_url ? `<a class="btn" href="${escapeHtml(work.demo_url)}" target="_blank" rel="noopener">在线演示</a>` : ""}
          <a class="btn" href="/" data-nav>← 返回首页</a>
        </div>
      </div>`;
    document.title = work.title + " — 作品";
  }

  // ── Docs ──

  async function renderDocs(workSlug, docPath) {
    const work = await api(`/api/works/${encodeURIComponent(workSlug)}`);
    const docsSlug = work.docs_slug || workSlug;
    const index = await api(`/api/docs/${encodeURIComponent(docsSlug)}/index`);

    // Default to FOREWORD.md or first leaf in INDEX.json
    if (!docPath) {
      const preferred = "FOREWORD.md";
      const hasPreferred = index.items.some(
        (i) => i.path === preferred || findPathInItems(index.items, preferred)
      );
      const first = findFirstDoc(index.items);
      const target = hasPreferred ? preferred : first;
      if (target) {
        navigate(`/works/${workSlug}/docs/${target}`);
        return;
      }
    }

    let content = { content: "# 暂无文档\n\n请在 `docs/` 目录添加 Markdown 文件。" };
    if (docPath) {
      const apiPath = docPath.replace(/\//g, ":");
      content = await api(
        `/api/docs/${encodeURIComponent(docsSlug)}/content/${encodeURIComponent(apiPath)}`
      );
    }

    const navHtml = renderDocNav(index.items, workSlug, docPath);
    const breadcrumb = `
      <div class="docs-breadcrumb">
        <a href="/" data-nav>首页</a> /
        <a href="/works/${escapeHtml(workSlug)}" data-nav>${escapeHtml(work.title)}</a> /
        文档
      </div>`;

    document.getElementById("app").innerHTML = `
      <div class="page docs-layout">
        <aside class="docs-sidebar">
          <h4>${escapeHtml(index.title)}</h4>
          <ul class="doc-nav">${navHtml}</ul>
        </aside>
        <div class="docs-content">
          ${breadcrumb}
          <article class="markdown">${renderMarkdown(content.content)}</article>
        </div>
      </div>`;
    document.title = work.title + " — 文档";
  }

  function findFirstDoc(items) {
    for (const item of items || []) {
      if (item.path) return item.path;
      if (item.children) {
        const found = findFirstDoc(item.children);
        if (found) return found;
      }
    }
    return null;
  }

  function findPathInItems(items, path) {
    for (const item of items || []) {
      if (item.path === path) return true;
      if (item.children && findPathInItems(item.children, path)) return true;
    }
    return false;
  }

  function renderDocNav(items, workSlug, activePath) {
    return (items || [])
      .map((item) => {
        if (item.path) {
          const active = item.path === activePath ? " active" : "";
          return `<li><a href="/works/${escapeHtml(workSlug)}/docs/${escapeHtml(item.path)}" class="${active.trim()}" data-nav>${escapeHtml(item.title)}</a></li>`;
        }
        if (item.children) {
          return `<li>
            <span class="nav-section">${escapeHtml(item.title)}</span>
            <ul class="doc-nav children">${renderDocNav(item.children, workSlug, activePath)}</ul>
          </li>`;
        }
        return "";
      })
      .join("");
  }

  // ── Blog ──

  async function renderBlogList() {
    const posts = await api("/api/blog");
    document.getElementById("app").innerHTML = `
      <div class="page">
        <h2 class="section-title">技术博客</h2>
        ${posts.length === 0 ? '<p style="color:var(--text-muted)">暂无文章。</p>' : `
          <div class="blog-list">
            ${posts.map((p) => `
              <a class="blog-item" href="/blog/${escapeHtml(p.slug)}" data-nav>
                <h3>${escapeHtml(p.title)}</h3>
                <div class="summary">${escapeHtml(p.summary)}</div>
                <div class="date">${escapeHtml(p.published_at)} · ${tagList(p.tags)}</div>
              </a>`).join("")}
          </div>`}
      </div>`;
    document.title = "博客 — " + (siteInfo?.title || "Docbit");
  }

  async function renderBlogPost(slug) {
    const post = await api(`/api/blog/${encodeURIComponent(slug)}`);
    document.getElementById("app").innerHTML = `
      <div class="page">
        <div class="detail-header">
          <div style="margin-bottom:0.75rem">${tagList(post.tags)}</div>
          <h1>${escapeHtml(post.title)}</h1>
          <div class="meta">${escapeHtml(post.published_at)}</div>
        </div>
        <article class="markdown">${renderMarkdown(post.content)}</article>
        <div class="btn-row">
          <a class="btn" href="/blog" data-nav>← 返回博客</a>
        </div>
      </div>`;
    document.title = post.title + " — 博客";
  }

  // ── About ──

  function renderAbout() {
    const site = siteInfo || {};
    const links = site.links || {};
    document.getElementById("app").innerHTML = `
      <div class="page">
        <div class="about-card">
          <h2>${escapeHtml(site.author || site.title || "关于")}</h2>
          <div class="tagline">${escapeHtml(site.tagline || "")}</div>
          <p>${escapeHtml(site.bio || "")}</p>
          <div class="about-links">
            ${links.github ? `<a class="btn" href="${escapeHtml(links.github)}" target="_blank" rel="noopener">GitHub</a>` : ""}
            ${links.docs ? `<a class="btn" href="${escapeHtml(links.docs)}" target="_blank" rel="noopener">文档</a>` : ""}
          </div>
        </div>
      </div>`;
    document.title = "关于 — " + (site.title || "Docbit");
  }
})();
