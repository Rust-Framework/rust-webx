/* =========================================================================
   Docbit — Portfolio SPA Client
   ========================================================================= */

(function () {
  "use strict";

  const API = "";
  const THEME_KEY = "docbit-theme";

  const HLJS_THEMES = {
    dark: "https://cdn.jsdelivr.net/npm/highlight.js@11.9.0/styles/github-dark.min.css",
    light: "https://cdn.jsdelivr.net/npm/highlight.js@11.9.0/styles/github.min.css",
  };

  function loadTheme() {
    const theme = localStorage.getItem(THEME_KEY) || "light";
    document.documentElement.setAttribute("data-theme", theme);
    updateHljsTheme(theme);
  }

  function setTheme(mode) {
    document.documentElement.setAttribute("data-theme", mode);
    localStorage.setItem(THEME_KEY, mode);
    updateHljsTheme(mode);
  }

  function updateHljsTheme(theme) {
    const link = document.getElementById("hljs-theme");
    if (link) link.href = HLJS_THEMES[theme] || HLJS_THEMES.light;
  }

  function initTheme() {
    loadTheme();
    document.getElementById("theme-mode-btn")?.addEventListener("click", () => {
      const current = document.documentElement.getAttribute("data-theme");
      setTheme(current === "dark" ? "light" : "dark");
    });
  }

  // ── Markdown (markdown-it + highlight.js + DOMPurify) ──

  let md = null;

  function initMarkdown() {
    if (typeof markdownit === "undefined") return null;

    md = markdownit({
      html: false,
      linkify: true,
      typographer: true,
      highlight: (str, lang) => {
        if (typeof hljs !== "undefined" && lang && hljs.getLanguage(lang)) {
          try {
            return hljs.highlight(str, { language: lang }).value;
          } catch (_) {}
        }
        if (typeof hljs !== "undefined") {
          try {
            return hljs.highlightAuto(str).value;
          } catch (_) {}
        }
        return escapeHtml(str);
      },
    });

    if (typeof markdownItAnchor !== "undefined") {
      const anchorOpts = { level: [1, 2, 3, 4] };
      try {
        if (markdownItAnchor.permalink?.ariaHidden) {
          anchorOpts.permalink = markdownItAnchor.permalink.ariaHidden({
            placement: "after",
            class: "header-anchor",
            symbol: "#",
          });
        }
        md.use(markdownItAnchor, anchorOpts);
      } catch (_) {}
    }

    return md;
  }

  function renderMarkdown(mdText) {
    if (!md) initMarkdown();
    if (!md) return `<pre>${escapeHtml(mdText)}</pre>`;

    const raw = md.render(mdText);
    if (typeof DOMPurify !== "undefined") {
      return DOMPurify.sanitize(raw, {
        ADD_ATTR: ["target", "rel"],
        ADD_TAGS: ["iframe"],
      });
    }
    return raw;
  }

  function enhanceMarkdown(container) {
    if (!container) return;

    container.querySelectorAll("pre code").forEach((block) => {
      const pre = block.parentElement;
      if (pre.querySelector(".code-copy-btn")) return;
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "code-copy-btn";
      btn.textContent = "复制";
      btn.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText(block.textContent);
          btn.textContent = "已复制";
          btn.classList.add("copied");
          setTimeout(() => {
            btn.textContent = "复制";
            btn.classList.remove("copied");
          }, 2000);
        } catch (_) {}
      });
      pre.appendChild(btn);
    });

    container.querySelectorAll("a[href^='/']").forEach((a) => {
      if (a.getAttribute("target") === "_blank") return;
      a.setAttribute("data-nav", "");
    });
  }

  function buildToc(container) {
    const headings = container.querySelectorAll("h2, h3");
    if (headings.length < 2) return "";

    const items = [];
    headings.forEach((h) => {
      const id = h.getAttribute("id");
      if (!id) return;
      const level = h.tagName === "H3" ? "toc-h3" : "";
      const text = h.textContent.replace(/#$/, "").trim();
      items.push(
        `<li><a href="#${escapeHtml(id)}" class="${level}">${escapeHtml(text)}</a></li>`
      );
    });

    if (!items.length) return "";
    return `<nav class="docs-toc" id="docs-toc"><h5>本页目录</h5><ul>${items.join("")}</ul></nav>`;
  }

  function initTocScrollSpy() {
    const toc = document.getElementById("docs-toc");
    if (!toc) return;

    const links = toc.querySelectorAll("a");
    const ids = Array.from(links).map((a) => a.getAttribute("href").slice(1));
    const headings = ids
      .map((id) => document.getElementById(id))
      .filter(Boolean);

    if (!headings.length) return;

    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            const id = entry.target.id;
            links.forEach((a) => {
              a.classList.toggle("active", a.getAttribute("href") === `#${id}`);
            });
          }
        });
      },
      { rootMargin: "-80px 0px -70% 0px", threshold: 0 }
    );

    headings.forEach((h) => observer.observe(h));
  }

  // ── API ──

  async function api(path) {
    const res = await fetch(API + path);
    if (!res.ok) {
      let msg = res.statusText;
      try {
        const data = await res.json();
        msg = data.detail || data.error || data.title || msg;
      } catch (_) {}
      throw new Error(msg || "Request failed");
    }
    return res.json();
  }

  function docContentUrl(docsSlug, docPath) {
    const apiPath = docPath.replace(/\//g, ":");
    return `/api/docs/${encodeURIComponent(docsSlug)}/content/${apiPath}`;
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

  function navigate(path, replace) {
    if (replace) {
      window.history.replaceState(null, "", path);
    } else {
      window.history.pushState(null, "", path);
    }
    render();
  }

  // ── Bootstrap ──

  document.addEventListener("DOMContentLoaded", () => {
    initTheme();
    initMarkdown();
    initBackToTop();
    initMobileNav();

    document.addEventListener("click", (e) => {
      const link = e.target.closest("a[data-nav], a[href^='/']");
      if (!link) return;
      const href = link.getAttribute("href");
      if (!href || !href.startsWith("/") || href.startsWith("//")) return;
      if (link.getAttribute("target") === "_blank") return;
      if (href.includes("#") && href.split("#")[0] === window.location.pathname) return;
      e.preventDefault();
      navigate(href.split("#")[0]);
      if (href.includes("#")) {
        requestAnimationFrame(() => {
          const el = document.getElementById(href.split("#")[1]);
          el?.scrollIntoView({ behavior: "smooth" });
        });
      }
    });

    window.addEventListener("popstate", render);
    loadSite().then(render);
  });

  function initBackToTop() {
    const btn = document.getElementById("back-to-top");
    if (!btn) return;
    window.addEventListener("scroll", () => {
      btn.hidden = window.scrollY < 400;
    }, { passive: true });
    btn.addEventListener("click", () => {
      window.scrollTo({ top: 0, behavior: "smooth" });
    });
  }

  function initMobileNav() {
    const toggle = document.getElementById("nav-toggle");
    const nav = document.getElementById("main-nav");
    toggle?.addEventListener("click", () => {
      const open = nav.classList.toggle("open");
      toggle.classList.toggle("open", open);
      toggle.setAttribute("aria-expanded", String(open));
    });
    nav?.querySelectorAll("a").forEach((a) => {
      a.addEventListener("click", () => {
        nav.classList.remove("open");
        toggle?.classList.remove("open");
        toggle?.setAttribute("aria-expanded", "false");
      });
    });
  }

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
      '<div class="loading-state"><div class="spinner"></div><span>加载中…</span></div>';
  }

  function showError(msg) {
    document.getElementById("app").innerHTML =
      `<div class="error-box"><strong>加载失败</strong><p>${escapeHtml(msg)}</p>
       <div class="btn-row" style="justify-content:center;margin-top:1rem">
         <a class="btn btn-primary" href="/" data-nav>返回首页</a>
       </div></div>`;
  }

  async function render() {
    const route = parseRoute(window.location.pathname);
    setActiveNav(route.page);
    showLoading();
    window.scrollTo(0, 0);

    const app = document.getElementById("app");
    app.classList.toggle("docs-page", route.page === "docs");

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
        ${(w.tags || []).length ? `<div class="work-card-footer">${tagList(w.tags)}</div>` : ""}
      </a>`;

    document.getElementById("app").innerHTML = `
      <div class="page">
        <section class="hero">
          <div class="hero-tag">${escapeHtml(site.tagline || "Start 的作品")}</div>
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
        ${works.length === 0 ? '<p style="color:var(--text-muted);text-align:center;padding:2rem">暂无作品，请通过 API 添加。</p>' : ""}
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
          <div class="detail-meta">${categoryBadge(work.category)}</div>
          <h1>${escapeHtml(work.title)}</h1>
          <div class="meta">${escapeHtml(work.subtitle)}</div>
          ${(work.tags || []).length ? `<div class="tag-row">${tagList(work.tags)}</div>` : ""}
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

    if (!docPath) {
      const preferred = "FOREWORD.md";
      const hasPreferred = index.items.some(
        (i) => i.path === preferred || findPathInItems(index.items, preferred)
      );
      const first = findFirstDoc(index.items);
      const target = hasPreferred ? preferred : first;
      if (target) {
        navigate(`/works/${workSlug}/docs/${target}`, true);
        return;
      }
    }

    let content = { content: "# 暂无文档\n\n请在 `docs/` 目录添加 Markdown 文件。" };
    if (docPath) {
      content = await api(docContentUrl(docsSlug, docPath));
    }

    const navHtml = renderDocNav(index.items, workSlug, docPath);

    document.getElementById("app").innerHTML = `
      <div class="docs-shell" id="docs-layout">
        <div class="sidebar-overlay" id="sidebar-overlay"></div>
        <aside class="docs-sidebar" id="docs-sidebar">
          <div class="docs-sidebar-header">
            <h4>${escapeHtml(index.title)}</h4>
            <button type="button" class="sidebar-close" id="sidebar-close" aria-label="关闭目录">×</button>
          </div>
          <nav class="doc-nav-wrap"><ul class="doc-nav">${navHtml}</ul></nav>
        </aside>
        <div class="docs-main">
          <div class="docs-toolbar">
            <button type="button" class="btn btn-sm" id="sidebar-open">☰ 目录</button>
            <div class="docs-breadcrumb">
              <a href="/" data-nav>首页</a>
              <span>/</span>
              <a href="/works/${escapeHtml(workSlug)}" data-nav>${escapeHtml(work.title)}</a>
              <span>/</span>
              <span>文档</span>
            </div>
          </div>
          <article class="markdown-body docs-article" id="doc-article">${renderMarkdown(content.content)}</article>
        </div>
        <aside class="docs-toc-panel" id="toc-slot"></aside>
      </div>`;

    const article = document.getElementById("doc-article");
    enhanceMarkdown(article);

    const tocHtml = buildToc(article);
    const tocSlot = document.getElementById("toc-slot");
    if (tocHtml && tocSlot) {
      tocSlot.innerHTML = tocHtml;
      document.getElementById("docs-layout").classList.add("has-toc");
      initTocScrollSpy();
    } else if (tocSlot) {
      tocSlot.remove();
    }

    initDocsSidebar();
    document.title = work.title + " — 文档";
  }

  function initDocsSidebar() {
    const sidebar = document.getElementById("docs-sidebar");
    const overlay = document.getElementById("sidebar-overlay");
    const openBtn = document.getElementById("sidebar-open");
    const closeBtn = document.getElementById("sidebar-close");

    function close() {
      sidebar?.classList.remove("open");
      overlay?.classList.remove("visible");
    }
    function open() {
      sidebar?.classList.add("open");
      overlay?.classList.add("visible");
    }

    openBtn?.addEventListener("click", open);
    closeBtn?.addEventListener("click", close);
    overlay?.addEventListener("click", close);
    sidebar?.querySelectorAll("a").forEach((a) => a.addEventListener("click", close));
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

  function renderDocNav(items, workSlug, activePath, depth = 0) {
    return (items || [])
      .map((item) => {
        if (item.path) {
          const active = item.path === activePath ? " active" : "";
          return `<li class="nav-leaf"><a href="/works/${escapeHtml(workSlug)}/docs/${escapeHtml(item.path)}" class="${active.trim()}" data-nav>${escapeHtml(item.title)}</a></li>`;
        }
        if (item.children) {
          const sectionClass = depth === 0 ? "nav-part" : depth === 1 ? "nav-chapter" : "nav-group";
          return `<li class="${sectionClass}">
            <span class="nav-section nav-depth-${depth}">${escapeHtml(item.title)}</span>
            <ul class="doc-nav children depth-${depth + 1}">${renderDocNav(item.children, workSlug, activePath, depth + 1)}</ul>
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
        <div class="docs-article-card">
          <article class="markdown-body" id="blog-article">${renderMarkdown(post.content)}</article>
        </div>
        <div class="btn-row">
          <a class="btn" href="/blog" data-nav>← 返回博客</a>
        </div>
      </div>`;
    enhanceMarkdown(document.getElementById("blog-article"));
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
