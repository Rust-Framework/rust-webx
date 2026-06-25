/* Blog — shell layout, SERP list, Vditor editor */
(function () {
  "use strict";

  const { escapeHtml, tagList, blogCategoryLabel } = Docbit.Utils;
  const { render: renderMd, enhance } = Docbit.Markdown;

  const BLOG_CATEGORIES_FALLBACK = [
    { id: "rust", name: "Rust 生态" },
    { id: "webapi", name: "Web 开发" },
    { id: "tutorial", name: "教程实践" },
    { id: "portfolio", name: "作品集" },
    { id: "news", name: "动态资讯" },
  ];

  let editorInstance = null;
  let editorResizeObserver = null;
  let editorFullscreenObserver = null;
  let editorWindowResize = null;
  let listSearchQuery = "";

  function getUrlParams() {
    const p = new URLSearchParams(window.location.search);
    return {
      category: p.get("category") || "",
      q: p.get("q") || "",
    };
  }

  function categoryLink(cat, q) {
    const params = new URLSearchParams();
    if (cat) params.set("category", cat);
    if (q) params.set("q", q);
    const qs = params.toString();
    return qs ? `/blog?${qs}` : "/blog";
  }

  function canManagePost(post) {
    const user = Docbit.Auth.getUser();
    if (!user) return false;
    return user.role === "admin" || post.author_id === user.id;
  }

  function categoryName(c) {
    return c.name || blogCategoryLabel(c.id);
  }

  function renderCategorySidebar(categories, activeCategory, totalCount, extraHtml = "", q = "") {
    const allActive = !activeCategory ? " active" : "";
    const items = [
      `<li><a href="${categoryLink("", q)}" class="shell-nav-link${allActive}" data-nav>全部 <span class="count">${totalCount}</span></a></li>`,
    ];
    const list = categories.length
      ? categories
      : BLOG_CATEGORIES_FALLBACK.map((c) => ({ id: c.id, name: c.name, count: 0 }));
    list.forEach((c) => {
      const active = c.id === activeCategory ? " active" : "";
      items.push(
        `<li><a href="${categoryLink(c.id, q)}" class="shell-nav-link${active}" data-nav>
          ${escapeHtml(categoryName(c))}
          <span class="count">${c.count || ""}</span>
        </a></li>`
      );
    });
    return `
      <aside class="content-sidebar">
        <div class="content-sidebar-header">
          <div class="shell-brand">
            <span class="shell-brand-icon">✎</span>
            <div>
              <span class="shell-brand-label">博客</span>
              <h4>技术分享</h4>
            </div>
          </div>
          <button type="button" class="sidebar-close" id="sidebar-close" aria-label="关闭">×</button>
        </div>
        ${extraHtml}
        <nav class="content-sidebar-body shell-nav"><ul class="blog-cat-list">${items.join("")}</ul></nav>
      </aside>`;
  }

  function shellWrap(sidebarHtml, mainHtml) {
    return `
      <div class="content-shell page-blog">
        <div class="sidebar-overlay" id="sidebar-overlay"></div>
        ${sidebarHtml}
        <div class="content-main">${mainHtml}</div>
      </div>`;
  }

  function topbar(breadcrumbHtml, actionsHtml = "") {
    return `
      <div class="shell-topbar">
        <button type="button" class="btn btn-sm shell-menu-btn" id="sidebar-open">☰</button>
        <nav class="shell-breadcrumb" aria-label="面包屑">${breadcrumbHtml}</nav>
        ${actionsHtml ? `<div class="shell-topbar-actions">${actionsHtml}</div>` : ""}
      </div>`;
  }

  function filterPosts(posts, category, q) {
    let list = category ? posts.filter((p) => p.category === category) : posts;
    const needle = (q || "").trim().toLowerCase();
    if (!needle) return list;
    return list.filter((p) => {
      const hay = [
        p.title,
        p.summary,
        p.slug,
        p.author_name,
        p.category,
        blogCategoryLabel(p.category),
        ...(p.tags || []),
      ]
        .join(" ")
        .toLowerCase();
      return hay.includes(needle);
    });
  }

  function highlightQuery(text, q) {
    const raw = text == null ? "" : String(text);
    const needle = (q || "").trim();
    if (!needle) return escapeHtml(raw);
    const lower = raw.toLowerCase();
    const nLower = needle.toLowerCase();
    let out = "";
    let i = 0;
    while (i < raw.length) {
      const idx = lower.indexOf(nLower, i);
      if (idx === -1) {
        out += escapeHtml(raw.slice(i));
        break;
      }
      out += escapeHtml(raw.slice(i, idx));
      out += `<mark class="serp-hit">${escapeHtml(raw.slice(idx, idx + needle.length))}</mark>`;
      i = idx + needle.length;
    }
    return out;
  }

  function renderSearchHub(q, resultCount, activeCategory) {
    return `
      <div class="blog-search-hub">
        <form class="blog-search-form" id="blog-search-form" role="search">
          <div class="blog-search-box">
            <span class="blog-search-icon" aria-hidden="true">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3-3"/></svg>
            </span>
            <input
              type="search"
              id="blog-search-input"
              class="blog-search-input"
              placeholder="搜索文章标题、摘要、标签…"
              value="${escapeHtml(q)}"
              autocomplete="off"
              enterkeyhint="search"
            />
            ${q ? `<button type="button" class="blog-search-clear" id="blog-search-clear" aria-label="清除">×</button>` : ""}
            <button type="submit" class="btn btn-primary blog-search-btn">搜索</button>
          </div>
        </form>
        <p class="blog-search-stats" id="blog-search-stats">
          ${q ? `找到约 <strong>${resultCount}</strong> 条与「${escapeHtml(q)}」相关的结果` : `共 <strong>${resultCount}</strong> 篇文章`}
          ${activeCategory ? ` · 分类 <strong>${escapeHtml(blogCategoryLabel(activeCategory))}</strong>` : ""}
        </p>
      </div>`;
  }

  function formatPostDate(value) {
    const n = Number(value);
    if (!Number.isNaN(n) && n > 1_000_000_000) {
      return new Date(n * 1000).toLocaleDateString("zh-CN", {
        year: "numeric",
        month: "short",
        day: "numeric",
      });
    }
    return value == null ? "" : String(value);
  }

  function renderBlogCardList(posts, q) {
    if (!posts.length) {
      return `<p class="empty-hint blog-list-empty">${q ? "未找到匹配的文章，试试其他关键词。" : "该分类下暂无文章。"}</p>`;
    }
    return `<div class="blog-card-list">
      ${posts
        .map(
          (p) => `
        <article class="blog-card">
          <a href="/blog/${escapeHtml(p.slug)}" class="blog-card-stretch" data-nav aria-label="${escapeHtml(p.title)}"></a>
          <header class="blog-card-head">
            ${p.category ? `<span class="blog-card-cat">${escapeHtml(blogCategoryLabel(p.category))}</span>` : '<span class="blog-card-cat blog-card-cat--muted">未分类</span>'}
            <time class="blog-card-date" datetime="${escapeHtml(p.published_at || "")}">${escapeHtml(formatPostDate(p.published_at))}</time>
          </header>
          <h3 class="blog-card-title">
            <a href="/blog/${escapeHtml(p.slug)}" data-nav>${highlightQuery(p.title, q)}</a>
          </h3>
          <p class="blog-card-summary">${highlightQuery(p.summary, q)}</p>
          ${(p.tags || []).length ? `<div class="blog-card-tags">${tagList(p.tags)}</div>` : ""}
          <footer class="blog-card-foot">
            <span class="blog-card-author">${escapeHtml(p.author_name || "匿名")}</span>
            <span class="blog-card-slug">${escapeHtml(p.slug)}</span>
          </footer>
        </article>`
        )
        .join("")}
    </div>`;
  }

  function bindSearchForm(activeCategory) {
    const form = document.getElementById("blog-search-form");
    const input = document.getElementById("blog-search-input");
    if (!form || !input) return;

    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const q = input.value.trim();
      const path = categoryLink(activeCategory, q);
      const current = window.location.pathname + window.location.search;
      if (path === current) return;
      Docbit.Router.navigate(path);
    });

    document.getElementById("blog-search-clear")?.addEventListener("click", () => {
      Docbit.Router.navigate(categoryLink(activeCategory, ""));
    });

    let debounce;
    input.addEventListener("input", () => {
      clearTimeout(debounce);
      debounce = setTimeout(() => {
        listSearchQuery = input.value.trim();
        const stats = document.getElementById("blog-search-stats");
        const results = document.querySelector(".blog-card-list, .blog-list-empty");
        if (!stats || !window.__blogPostsCache) return;
        const filtered = filterPosts(window.__blogPostsCache, activeCategory, listSearchQuery);
        stats.innerHTML = listSearchQuery
          ? `找到约 <strong>${filtered.length}</strong> 条与「${escapeHtml(listSearchQuery)}」相关的结果`
          : `共 <strong>${filtered.length}</strong> 篇文章`;
        const container = results?.parentElement;
        if (container) {
          const old = container.querySelector(".blog-card-list, .blog-list-empty");
          if (old) old.outerHTML = renderBlogCardList(filtered, listSearchQuery);
        }
      }, 280);
    });
  }

  function renderComments(comments) {
    if (!comments.length) {
      return '<p class="comments-empty">暂无评论，来发表第一条吧。</p>';
    }
    return `<ul class="comment-list">
      ${comments
        .map(
          (c) => `
        <li class="comment-item">
          <div class="comment-head">
            <strong>${escapeHtml(c.user_name)}</strong>
            <time>${escapeHtml(c.created_at)}</time>
          </div>
          <div class="comment-body">${escapeHtml(c.content)}</div>
        </li>`
        )
        .join("")}
    </ul>`;
  }

  function renderCommentForm(slug) {
    const redirect = encodeURIComponent(`/blog/${slug}`);
    if (!Docbit.Auth.isLoggedIn()) {
      return `<div class="comment-guest">
        <p>登录后即可参与讨论。</p>
        <a href="/login?redirect=${redirect}" class="btn btn-primary btn-sm" data-nav>登录</a>
        <a href="/register" class="btn btn-sm" data-nav>注册</a>
      </div>`;
    }
    return `<form class="comment-form" id="comment-form">
      <label for="comment-content">发表评论</label>
      <textarea id="comment-content" rows="4" placeholder="写下你的想法…" maxlength="4000" required></textarea>
      <div class="comment-form-actions">
        <button type="submit" class="btn btn-primary btn-sm">发布评论</button>
      </div>
      <p class="form-error" id="comment-error" hidden></p>
    </form>`;
  }

  function bindCommentForm(slug) {
    const form = document.getElementById("comment-form");
    if (!form) return;
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const errEl = document.getElementById("comment-error");
      const content = document.getElementById("comment-content")?.value?.trim();
      if (!content) return;
      errEl.hidden = true;
      try {
        await Docbit.Api.post(`/api/blog/${encodeURIComponent(slug)}/comments`, { slug, content }, true);
        await renderPost(slug);
      } catch (err) {
        errEl.textContent = err.message;
        errEl.hidden = false;
      }
    });
  }

  async function loadVditor() {
    if (window.Vditor) return;
    await Docbit.Loader.loadCSS("/assets/vendor/vditor-dist/index.css");
    await Docbit.Loader.loadScript("/assets/vendor/vditor-dist/index.min.js");
    if (!window.Vditor) {
      throw new Error("Vditor 编辑器加载失败");
    }
  }

  function destroyEditor() {
    editorResizeObserver?.disconnect();
    editorResizeObserver = null;
    editorFullscreenObserver?.disconnect();
    editorFullscreenObserver = null;
    if (editorWindowResize) {
      window.removeEventListener("resize", editorWindowResize);
      editorWindowResize = null;
    }
    document.body.classList.remove("vditor-fs-active");
    if (editorInstance) {
      try {
        editorInstance.destroy();
      } catch (_) {
        /* ignore */
      }
      editorInstance = null;
    }
  }

  function getEditorMarkdown() {
    return editorInstance?.getValue() || "";
  }

  function isDarkTheme() {
    return document.documentElement.getAttribute("data-theme") === "dark";
  }

  function syncEditorHeight() {
    const body = document.querySelector(".blog-edit-body");
    const host = document.getElementById("vditor");
    if (!body || !host || !editorInstance) return;
    const h = Math.max(280, body.clientHeight);
    host.style.height = `${h}px`;
    try {
      editorInstance.resize?.();
    } catch (_) {
      /* ignore */
    }
  }

  function watchEditorFullscreen() {
    const root = document.querySelector(".blog-edit-body .vditor");
    if (!root) return null;
    const sync = () => {
      document.body.classList.toggle("vditor-fs-active", root.classList.contains("vditor--fullscreen"));
    };
    sync();
    const mo = new MutationObserver(sync);
    mo.observe(root, { attributes: true, attributeFilter: ["class"] });
    return mo;
  }

  function buildCategoryOptions(categories, selectedId) {
    const list = categories.length ? categories : BLOG_CATEGORIES_FALLBACK.map((c) => ({ ...c, count: 0 }));
    const sel = selectedId || list[0]?.id || "rust";
    return list
      .map((c) => `<option value="${escapeHtml(c.id)}"${c.id === sel ? " selected" : ""}>${escapeHtml(categoryName(c))}</option>`)
      .join("");
  }

  async function renderList(siteInfo) {
    const [posts, categories] = await Promise.all([
      Docbit.Api.get("/api/blog"),
      Docbit.Api.get("/api/blog/categories"),
    ]);
    const { category: activeCategory, q } = getUrlParams();
    listSearchQuery = q;
    window.__blogPostsCache = posts;

    const filtered = filterPosts(posts, activeCategory, q);
    const loggedIn = Docbit.Auth.isLoggedIn();
    const sidebarExtra = loggedIn
      ? `<div class="shell-sidebar-actions"><a href="/blog/write" class="btn btn-primary btn-sm" data-nav>写文章</a></div>`
      : "";

    const sidebar = renderCategorySidebar(categories, activeCategory, posts.length, sidebarExtra, q);
    const main = `
      ${topbar(
        `<a href="/" data-nav>首页</a><span class="sep">/</span><span class="current">博客</span>`,
        loggedIn ? `<a href="/blog/write" class="btn btn-sm" data-nav>管理文章</a>` : ""
      )}
      <div class="shell-article blog-list-article">
        ${renderSearchHub(q, filtered.length, activeCategory)}
        ${renderBlogCardList(filtered, q)}
      </div>`;

    document.getElementById("app").innerHTML = shellWrap(sidebar, main);
    bindSearchForm(activeCategory);
    Docbit.Shell.initShellSidebar();
    document.title = (q ? `${q} — 搜索` : "博客") + " — " + (siteInfo?.title || "Docbit");
  }

  async function renderPost(slug, siteInfo) {
    const [post, comments, categories, allPosts] = await Promise.all([
      Docbit.Api.get(`/api/blog/${encodeURIComponent(slug)}`),
      Docbit.Api.get(`/api/blog/${encodeURIComponent(slug)}/comments`),
      Docbit.Api.get("/api/blog/categories"),
      Docbit.Api.get("/api/blog"),
    ]);

    const manage = canManagePost(post);
    const sidebarExtra = Docbit.Auth.isLoggedIn()
      ? `<div class="shell-sidebar-actions"><a href="/blog/write" class="btn btn-primary btn-sm" data-nav>写文章</a></div>`
      : "";

    const sidebar = renderCategorySidebar(categories, post.category, allPosts.length, sidebarExtra);
    const main = `
      ${topbar(
        `<a href="/" data-nav>首页</a><span class="sep">/</span><a href="/blog" data-nav>博客</a><span class="sep">/</span><span class="current">${escapeHtml(post.title)}</span>`,
        manage
          ? `<a href="/blog/write/${escapeHtml(post.slug)}" class="btn btn-sm" data-nav>编辑</a>`
          : ""
      )}
      <header class="blog-post-header">
        ${post.category ? `<span class="shell-cat-badge">${escapeHtml(blogCategoryLabel(post.category))}</span>` : ""}
        <h1>${escapeHtml(post.title)}</h1>
        <div class="meta">${escapeHtml(post.author_name)} · ${escapeHtml(post.published_at)}</div>
        ${(post.tags || []).length ? `<div class="tag-row" style="margin-top:0.5rem">${tagList(post.tags)}</div>` : ""}
      </header>
      <article class="markdown-body shell-article" id="blog-article">${renderMd(post.content)}</article>
      <section class="comments-block" id="comments-section">
        <h3 class="comments-title">讨论 · ${comments.length} 条评论</h3>
        ${renderCommentForm(slug)}
        ${renderComments(comments)}
      </section>`;

    document.getElementById("app").innerHTML = shellWrap(sidebar, main);
    enhance(document.getElementById("blog-article"));
    bindCommentForm(slug);
    Docbit.Shell.initShellSidebar();
    document.title = post.title + " — 博客";
  }

  async function renderWriteList() {
    if (!Docbit.Auth.isLoggedIn()) {
      Docbit.Router.navigate("/login?redirect=" + encodeURIComponent("/blog/write"));
      return;
    }
    const [myPosts, categories, allPosts] = await Promise.all([
      Docbit.Api.get("/api/blog/my", true),
      Docbit.Api.get("/api/blog/categories"),
      Docbit.Api.get("/api/blog"),
    ]);

    const list =
      myPosts.length === 0
        ? '<p class="empty-hint">你还没有发布文章，点击「新建文章」开始写作。</p>'
        : `<div class="shell-post-list">${myPosts
            .map(
              (p) => `
          <div class="shell-post-item" style="cursor:default">
            <div class="shell-post-top">
              <h3>${escapeHtml(p.title)}</h3>
              <a href="/blog/write/${escapeHtml(p.slug)}" class="btn btn-sm" data-nav>编辑</a>
            </div>
            <div class="summary">${escapeHtml(p.summary)}</div>
            <div class="shell-post-meta"><span>${escapeHtml(p.published_at)}</span></div>
          </div>`
            )
            .join("")}</div>`;

    const sidebar = renderCategorySidebar(
      categories,
      "",
      allPosts.length,
      `<div class="shell-sidebar-actions"><a href="/blog/write/new" class="btn btn-primary btn-sm" data-nav>新建文章</a></div>`
    );
    const main = `
      ${topbar(
        `<a href="/blog" data-nav>博客</a><span class="sep">/</span><span class="current">我的文章</span>`,
        `<a href="/blog/write/new" class="btn btn-primary btn-sm" data-nav>新建</a>`
      )}
      <div class="shell-article">${list}</div>`;

    document.getElementById("app").innerHTML = shellWrap(sidebar, main);
    Docbit.Shell.initShellSidebar();
    document.title = "我的文章 — 博客";
  }

  function bindCategoryAdd(categories, selectEl) {
    const panel = document.getElementById("new-category-panel");
    const addBtn = document.getElementById("add-category-btn");
    const saveBtn = document.getElementById("save-category-btn");
    const errEl = document.getElementById("category-error");
    if (!panel || !addBtn || !saveBtn || !selectEl) return;

    addBtn.addEventListener("click", () => {
      panel.hidden = !panel.hidden;
      if (!panel.hidden) document.getElementById("new-cat-name")?.focus();
    });

    saveBtn.addEventListener("click", async () => {
      const id = document.getElementById("new-cat-id")?.value?.trim();
      const name = document.getElementById("new-cat-name")?.value?.trim();
      errEl.hidden = true;
      if (!id || !/^[a-zA-Z0-9_-]+$/.test(id)) {
        errEl.textContent = "分类标识仅支持字母、数字、下划线和连字符";
        errEl.hidden = false;
        return;
      }
      if (!name) {
        errEl.textContent = "请填写分类名称";
        errEl.hidden = false;
        return;
      }
      try {
        await Docbit.Api.post("/api/blog/categories", { id, name }, true);
        categories.push({ id, name, count: 0 });
        categories.sort((a, b) => a.id.localeCompare(b.id));
        selectEl.innerHTML = buildCategoryOptions(categories, id);
        panel.hidden = true;
        document.getElementById("new-cat-id").value = "";
        document.getElementById("new-cat-name").value = "";
      } catch (err) {
        errEl.textContent = err.message;
        errEl.hidden = false;
      }
    });
  }

  async function renderEditor(slug) {
    if (!Docbit.Auth.isLoggedIn()) {
      Docbit.Router.navigate("/login?redirect=" + encodeURIComponent("/blog/write/" + (slug || "new")));
      return;
    }

    const isNew = !slug || slug === "new";
    let post = null;
    if (!isNew) {
      post = await Docbit.Api.get(`/api/blog/${encodeURIComponent(slug)}`);
      if (!canManagePost(post)) {
        throw new Error("无权编辑此文章");
      }
    }

    const categories = await Docbit.Api.get("/api/blog/categories");
    await loadVditor();
    destroyEditor();

    const categoryOptions = buildCategoryOptions(categories, post?.category);
    const isNewLabel = isNew ? "新建文章" : "编辑文章";

    document.getElementById("app").innerHTML = `
      <div class="blog-edit-page">
        <header class="blog-edit-toolbar">
          <nav class="blog-edit-crumb" aria-label="面包屑">
            <a href="/blog/write" data-nav>我的文章</a>
            <span class="sep">/</span>
            <span class="current">${isNewLabel}</span>
          </nav>
          <div class="blog-edit-toolbar-actions">
            <a href="/blog/write" class="btn btn-sm" data-nav>取消</a>
            ${!isNew ? `<button type="button" class="btn btn-sm btn-danger" id="delete-post-btn">删除</button>` : ""}
            <button type="submit" form="blog-edit-form" class="btn btn-primary btn-sm">保存</button>
          </div>
        </header>

        <form class="blog-edit-form" id="blog-edit-form" novalidate>
          <details class="blog-edit-meta-panel" open>
            <summary class="blog-edit-meta-toggle">文章信息</summary>
            <div class="blog-edit-meta-grid">
              <div class="meta-field meta-title">
                <label for="post-title">标题</label>
                <input id="post-title" required maxlength="300" placeholder="文章标题" value="${escapeHtml(post?.title || "")}" />
              </div>
              <div class="meta-field meta-slug">
                <label for="post-slug">Slug</label>
                <input id="post-slug" required maxlength="120" placeholder="url-slug" ${isNew ? "" : "readonly"} value="${escapeHtml(post?.slug || "")}" />
              </div>
              <div class="meta-field meta-category">
                <label for="post-category">分类</label>
                <div class="category-picker">
                  <select id="post-category">${categoryOptions}</select>
                  <button type="button" class="btn btn-sm category-add-btn" id="add-category-btn" title="添加分类">+</button>
                </div>
                <div id="new-category-panel" class="new-category-panel" hidden>
                  <input id="new-cat-id" placeholder="标识 (英文)" maxlength="40" />
                  <input id="new-cat-name" placeholder="显示名称" maxlength="60" />
                  <button type="button" class="btn btn-sm btn-primary" id="save-category-btn">添加</button>
                </div>
                <p class="form-error form-error-inline" id="category-error" hidden></p>
              </div>
              <div class="meta-field meta-tags">
                <label for="post-tags">标签</label>
                <input id="post-tags" placeholder="逗号分隔，如 rust, web" value="${escapeHtml((post?.tags || []).join(", "))}" />
              </div>
              <div class="meta-field meta-summary">
                <label for="post-summary">摘要</label>
                <textarea id="post-summary" required maxlength="500" rows="2" placeholder="一句话摘要，用于列表展示">${escapeHtml(post?.summary || "")}</textarea>
              </div>
            </div>
          </details>

          <div class="blog-edit-body">
            <div id="vditor" class="vditor-host"></div>
          </div>
          <p class="form-error blog-edit-error" id="edit-error" hidden></p>
        </form>
      </div>`;

    const selectEl = document.getElementById("post-category");
    bindCategoryAdd(categories, selectEl);

    const dark = isDarkTheme();
    const initialContent = post?.content || "## 新文章\n\n开始写作…";
    const editBody = document.querySelector(".blog-edit-body");
    const initialHeight = Math.max(320, editBody?.clientHeight || 480);

    editorInstance = new Vditor("vditor", {
      cdn: "/assets/vendor/vditor-dist",
      height: initialHeight,
      mode: "ir",
      lang: "zh_CN",
      placeholder: "开始写作…支持 Markdown、代码高亮与公式",
      theme: dark ? "dark" : "classic",
      icon: "material",
      cache: { enable: false },
      counter: { enable: true, type: "text" },
      outline: { enable: false },
      typewriterMode: false,
      resize: { enable: false },
      preview: {
        markdown: { toc: true },
        theme: { current: dark ? "dark" : "light" },
      },
      toolbar: [
        "headings",
        "bold",
        "italic",
        "strike",
        "|",
        "line",
        "quote",
        "list",
        "ordered-list",
        "check",
        "|",
        "code",
        "inline-code",
        "link",
        "table",
        "|",
        "undo",
        "redo",
        "|",
        "edit-mode",
        "both",
        "preview",
        "fullscreen",
      ],
      value: initialContent,
      after: () => {
        syncEditorHeight();
        if (editBody && typeof ResizeObserver !== "undefined") {
          editorResizeObserver = new ResizeObserver(() => syncEditorHeight());
          editorResizeObserver.observe(editBody);
        }
        editorFullscreenObserver = watchEditorFullscreen();
        document.querySelector("#vditor .vditor-ir")?.focus();
      },
    });

    const onResize = () => syncEditorHeight();
    editorWindowResize = onResize;
    window.addEventListener("resize", onResize);

    const form = document.getElementById("blog-edit-form");
    const errEl = document.getElementById("edit-error");
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      errEl.hidden = true;
      const slug = document.getElementById("post-slug").value.trim();
      if (!slug || !/^[a-zA-Z0-9_-]+$/.test(slug)) {
        errEl.textContent = "Slug 仅支持字母、数字、下划线和连字符";
        errEl.hidden = false;
        return;
      }
      const payload = {
        slug,
        title: document.getElementById("post-title").value.trim(),
        summary: document.getElementById("post-summary").value.trim(),
        content: getEditorMarkdown(),
        category: document.getElementById("post-category").value,
        tags: document
          .getElementById("post-tags")
          .value.split(",")
          .map((t) => t.trim())
          .filter(Boolean),
        published_at: post?.published_at || String(Math.floor(Date.now() / 1000)),
      };
      try {
        if (isNew) {
          await Docbit.Api.post("/api/blog", payload, true);
        } else {
          await Docbit.Api.put(`/api/blog/${encodeURIComponent(payload.slug)}`, payload, true);
        }
        window.removeEventListener("resize", onResize);
        editorWindowResize = null;
        Docbit.Router.navigate(`/blog/${payload.slug}`);
      } catch (err) {
        errEl.textContent = err.message;
        errEl.hidden = false;
      }
    });

    document.getElementById("delete-post-btn")?.addEventListener("click", async () => {
      if (!confirm("确定删除这篇文章？")) return;
      try {
        await Docbit.Api.del(`/api/blog/${encodeURIComponent(post.slug)}`, true);
        window.removeEventListener("resize", onResize);
        editorWindowResize = null;
        Docbit.Router.navigate("/blog/write");
      } catch (err) {
        errEl.textContent = err.message;
        errEl.hidden = false;
      }
    });

    document.title = (isNew ? "新建文章" : "编辑文章") + " — 博客";
  }

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.blog = { renderList, renderPost, renderWriteList, renderEditor, destroyEditor };
})();
