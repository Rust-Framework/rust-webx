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

  function categorySidebarParts(categories, activeCategory, totalCount, extraHtml = "", q = "") {
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
    return {
      header: `<div class="shell-brand">
            <span class="shell-brand-icon">✎</span>
            <div>
              <span class="shell-brand-label">博客</span>
              <h4>技术分享</h4>
            </div>
          </div>`,
      extra: extraHtml,
      body: `<ul class="blog-cat-list">${items.join("")}</ul>`,
    };
  }

  function mountBlogShell(sidebar, breadcrumb, actions, content) {
    Docbit.Shell.mount(
      Docbit.Shell.layout({
        className: "page-blog",
        sidebarHeader: sidebar.header,
        sidebarExtra: sidebar.extra,
        sidebarBody: sidebar.body,
        breadcrumb,
        actions,
        content,
      })
    );
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
            <button type="submit" class="blog-search-btn">搜索</button>
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
        <a href="/login?redirect=${redirect}" class="layui-btn layui-btn-sm layui-btn-normal" data-nav>登录</a>
        <a href="/register" class="layui-btn layui-btn-sm layui-btn-primary" data-nav>注册</a>
      </div>`;
    }
    return `<form class="layui-form comment-form" id="comment-form" lay-filter="comment-form">
      <div class="layui-form-item layui-form-text">
        <label class="layui-form-label">评论</label>
        <div class="layui-input-block">
          <textarea id="comment-content" class="layui-textarea" placeholder="写下你的想法…" maxlength="4000" required></textarea>
        </div>
      </div>
      <div class="layui-form-item">
        <div class="layui-input-block">
          <button type="submit" class="layui-btn layui-btn-sm layui-btn-normal">发布评论</button>
        </div>
      </div>
      <p class="form-error" id="comment-error" hidden></p>
    </form>`;
  }

  function bindCommentForm(slug) {
    const form = document.getElementById("comment-form");
    if (!form) return;
    Docbit.UI.renderForm();
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const errEl = document.getElementById("comment-error");
      const content = document.getElementById("comment-content")?.value?.trim();
      if (!content) return;
      errEl.hidden = true;
      try {
        await Docbit.Api.post(`/api/blog/${encodeURIComponent(slug)}/comments`, { slug, content }, true);
        Docbit.UI.success("评论已发布");
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
      ? `<div class="shell-sidebar-actions"><a href="/blog/write" class="layui-btn layui-btn-sm layui-btn-normal layui-btn-fluid" data-nav>写文章</a></div>`
      : "";

    const sidebar = categorySidebarParts(categories, activeCategory, posts.length, sidebarExtra, q);
    mountBlogShell(
      sidebar,
      `<a href="/" data-nav>首页</a><span class="sep">/</span><span class="current">博客</span>`,
      loggedIn ? `<a href="/blog/write" class="layui-btn layui-btn-sm layui-btn-primary" data-nav>管理文章</a>` : "",
      `<div class="shell-article blog-list-article">
        ${renderSearchHub(q, filtered.length, activeCategory)}
        ${renderBlogCardList(filtered, q)}
      </div>`
    );
    bindSearchForm(activeCategory);
    document.title = (q ? `${q} — 搜索` : "博客") + " — " + (siteInfo?.brand_name || siteInfo?.title || "Start World");
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
      ? `<div class="shell-sidebar-actions"><a href="/blog/write" class="layui-btn layui-btn-sm layui-btn-normal layui-btn-fluid" data-nav>写文章</a></div>`
      : "";

    const sidebar = categorySidebarParts(categories, post.category, allPosts.length, sidebarExtra);
    mountBlogShell(
      sidebar,
      `<a href="/" data-nav>首页</a><span class="sep">/</span><a href="/blog" data-nav>博客</a><span class="sep">/</span><span class="current">${escapeHtml(post.title)}</span>`,
      manage ? `<a href="/blog/write/${escapeHtml(post.slug)}" class="layui-btn layui-btn-sm layui-btn-primary" data-nav>编辑</a>` : "",
      `<header class="blog-post-header">
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
      </section>`
    );
    enhance(document.getElementById("blog-article"));
    bindCommentForm(slug);
    document.title = post.title + " — 博客";
  }

  let writeTableEventsBound = false;
  let writePostsCache = [];

  function formatTableDate(value) {
    const n = Number(value);
    if (!Number.isNaN(n) && n > 1_000_000_000) {
      return new Date(n * 1000).toLocaleString("zh-CN", {
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
      });
    }
    return value == null ? "" : String(value);
  }

  function normalizeWritePosts(posts) {
    return posts.map((p) => ({
      ...p,
      tags_text: (p.tags || []).join(", "),
      category_label: blogCategoryLabel(p.category),
      published_label: formatTableDate(p.published_at),
    }));
  }

  function bindWriteTableEvents(table) {
    if (writeTableEventsBound) return;
    writeTableEventsBound = true;

    table.on("toolbar(blog-manage-table)", (obj) => {
      if (obj.event === "create") {
        Docbit.Router.navigate("/blog/write/new");
        return;
      }
      if (obj.event === "refresh") {
        reloadWriteTable();
        return;
      }
      if (obj.event === "batchDel") {
        const checked = table.checkStatus("blogManageTable");
        if (!checked.data.length) {
          Docbit.UI.error("请先勾选要删除的文章");
          return;
        }
        Docbit.UI.confirm(`确定删除选中的 ${checked.data.length} 篇文章？`).then(async (ok) => {
          if (!ok) return;
          try {
            for (const row of checked.data) {
              await Docbit.Api.del(`/api/blog/${encodeURIComponent(row.slug)}`, true);
            }
            Docbit.UI.success("已删除选中文章");
            await reloadWriteTable();
          } catch (err) {
            Docbit.UI.error(err.message);
          }
        });
      }
    });

    table.on("tool(blog-manage-table)", (obj) => {
      const row = obj.data;
      if (obj.event === "view") {
        Docbit.Router.navigate(`/blog/${encodeURIComponent(row.slug)}`);
      } else if (obj.event === "edit") {
        Docbit.Router.navigate(`/blog/write/${encodeURIComponent(row.slug)}`);
      } else if (obj.event === "del") {
        Docbit.UI.confirm(`确定删除「${row.title}」？`).then(async (ok) => {
          if (!ok) return;
          try {
            await Docbit.Api.del(`/api/blog/${encodeURIComponent(row.slug)}`, true);
            obj.del();
            writePostsCache = writePostsCache.filter((p) => p.slug !== row.slug);
            Docbit.UI.success("文章已删除");
          } catch (err) {
            Docbit.UI.error(err.message);
          }
        });
      }
    });
  }

  function bindWriteSearch(table) {
    const root = document.querySelector(".page-blog-admin");
    if (!root || root.dataset.searchBound) return;
    root.dataset.searchBound = "1";
    let searchTimer;
    root.addEventListener("input", (e) => {
      if (e.target.id !== "blog-admin-search") return;
      clearTimeout(searchTimer);
      searchTimer = setTimeout(() => {
        const q = e.target.value.trim().toLowerCase();
        const filtered = q
          ? writePostsCache.filter((p) => {
              const hay = [p.title, p.slug, p.summary, p.category_label, p.tags_text]
                .join(" ")
                .toLowerCase();
              return hay.includes(q);
            })
          : writePostsCache;
        table.reload("blogManageTable", { data: filtered, page: { curr: 1 } });
      }, 280);
    });
  }

  async function reloadWriteTable() {
    const posts = await Docbit.Api.get("/api/blog/my", true);
    writePostsCache = normalizeWritePosts(posts);
    const table = await Docbit.UI.tableApi();
    const searchInput = document.getElementById("blog-admin-search");
    if (searchInput) searchInput.value = "";
    table.reload("blogManageTable", { data: writePostsCache, page: { curr: 1 } });
  }

  async function renderWriteList() {
    if (!Docbit.Auth.isLoggedIn()) {
      Docbit.Router.navigate("/login?redirect=" + encodeURIComponent("/blog/write"));
      return;
    }

    const myPosts = await Docbit.Api.get("/api/blog/my", true);
    writePostsCache = normalizeWritePosts(myPosts);

    document.getElementById("app").innerHTML = `
      <div class="page-blog-admin">
        <table id="blog-manage-table" lay-filter="blog-manage-table"></table>
      </div>
      <script type="text/html" id="blog-table-toolbar">
        <div class="blog-table-toolbar-inner">
          <div class="layui-btn-container">
            <button class="layui-btn layui-btn-sm" lay-event="create">
              <i class="layui-icon layui-icon-add-1"></i> 新建
            </button>
            <button class="layui-btn layui-btn-sm layui-btn-danger" lay-event="batchDel">
              <i class="layui-icon layui-icon-delete"></i> 删除
            </button>
            <button class="layui-btn layui-btn-sm layui-btn-primary" lay-event="refresh">
              <i class="layui-icon layui-icon-refresh"></i>
            </button>
          </div>
          <input type="search" class="layui-input" id="blog-admin-search" placeholder="搜索标题、Slug、摘要、标签" autocomplete="off" />
        </div>
      </script>
      <script type="text/html" id="blog-row-bar">
        <a class="layui-btn layui-btn-primary layui-btn-xs" lay-event="view">查看</a>
        <a class="layui-btn layui-btn-xs" lay-event="edit">编辑</a>
        <a class="layui-btn layui-btn-danger layui-btn-xs" lay-event="del">删除</a>
      </script>
      <script type="text/html" id="blog-title-tpl">
        <a href="/blog/{{ d.slug }}" data-nav>{{ d.title }}</a>
      </script>`;

    const table = await Docbit.UI.tableApi();
    bindWriteTableEvents(table);

    await Docbit.UI.renderTable({
      elem: "#blog-manage-table",
      id: "blogManageTable",
      data: writePostsCache,
      page: true,
      limit: 20,
      limits: [10, 20, 30, 50, 100],
      height: "full-105",
      cellMinWidth: 100,
      toolbar: "#blog-table-toolbar",
      defaultToolbar: ["filter", "exports", "print"],
      cols: [
        [
          { type: "checkbox", fixed: "left" },
          { field: "title", title: "标题", minWidth: 260, sort: true, templet: "#blog-title-tpl" },
          { field: "slug", title: "Slug", width: 160, sort: true },
          { field: "category_label", title: "分类", width: 120, sort: true },
          { field: "summary", title: "摘要", minWidth: 280 },
          { field: "tags_text", title: "标签", width: 180 },
          { field: "published_label", title: "发布时间", width: 180, sort: true },
          { fixed: "right", title: "操作", width: 200, align: "center", toolbar: "#blog-row-bar" },
        ],
      ],
      text: { none: "暂无文章" },
    });

    bindWriteSearch(table);
    document.title = "文章管理 — 博客";
  }

  function bindCategoryAdd(categories, selectEl) {
    const addBtn = document.getElementById("add-category-btn");
    if (!addBtn || !selectEl) return;

    addBtn.addEventListener("click", () => openCategoryDialog(categories, selectEl));
  }

  async function openCategoryDialog(categories, selectEl) {
    const layui = await Docbit.UI.loadLayui();

    function renderTableRows() {
      return categories
        .map(
          (c) =>
            `<tr>
              <td><code>${escapeHtml(c.id)}</code></td>
              <td>${escapeHtml(categoryName(c))}</td>
              <td>${c.count || 0}</td>
            </tr>`
        )
        .join("");
    }

    layui.layer.open({
      type: 1,
      title: "分类管理",
      area: ["500px", "auto"],
      shadeClose: true,
      content: `
        <div class="category-dialog" style="padding:16px 20px">
          <table class="layui-table" lay-size="sm">
            <colgroup><col width="130"><col><col width="60"></colgroup>
            <thead><tr><th>标识</th><th>名称</th><th>数量</th></tr></thead>
            <tbody id="cat-list-body">${renderTableRows()}</tbody>
          </table>
          <fieldset class="layui-elem-field" style="margin-top:16px">
            <legend>新建分类</legend>
            <div class="layui-field-box">
              <form class="layui-form" lay-filter="new-category-form">
                <div class="layui-form-item">
                  <label class="layui-form-label">标识</label>
                  <div class="layui-input-block">
                    <input type="text" id="new-cat-id" class="layui-input" placeholder="英文、数字、下划线" maxlength="40" />
                  </div>
                </div>
                <div class="layui-form-item">
                  <label class="layui-form-label">名称</label>
                  <div class="layui-input-block">
                    <input type="text" id="new-cat-name" class="layui-input" placeholder="显示名称" maxlength="60" />
                  </div>
                </div>
                <div class="layui-form-item">
                  <div class="layui-input-block">
                    <button type="button" class="layui-btn layui-btn-sm layui-btn-normal" id="save-category-btn">添加分类</button>
                  </div>
                </div>
              </form>
            </div>
          </fieldset>
          <p class="form-error form-error-inline" id="category-error" hidden></p>
        </div>`,
      success() {
        Docbit.UI.renderForm();
        const errEl = document.getElementById("category-error");
        document.getElementById("save-category-btn")?.addEventListener("click", async () => {
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
            Docbit.UI.renderForm("select");
            const tbody = document.getElementById("cat-list-body");
            if (tbody) tbody.innerHTML = renderTableRows();
            document.getElementById("new-cat-id").value = "";
            document.getElementById("new-cat-name").value = "";
            Docbit.UI.success("分类已添加");
          } catch (err) {
            errEl.textContent = err.message;
            errEl.hidden = false;
          }
        });
      },
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
        <div class="blog-edit-container">
          <header class="blog-edit-toolbar">
            <nav class="blog-edit-crumb" aria-label="面包屑">
              <a href="/blog/write" data-nav>文章管理</a>
              <span class="sep">/</span>
              <span class="current">${isNewLabel}</span>
            </nav>
            <div class="blog-edit-toolbar-actions">
              <a href="/blog/write" class="layui-btn layui-btn-sm layui-btn-primary" data-nav>取消</a>
              ${!isNew ? `<button type="button" class="layui-btn layui-btn-sm layui-btn-danger" id="delete-post-btn">删除</button>` : ""}
              <button type="submit" form="blog-edit-form" class="layui-btn layui-btn-sm layui-btn-normal">保存</button>
            </div>
          </header>

          <form class="layui-form blog-edit-form" id="blog-edit-form" lay-filter="blog-edit-form" novalidate>
            <div class="layui-form-item">
              <label class="layui-form-label" for="post-title">标题</label>
              <div class="layui-input-block">
                <input type="text" id="post-title" class="layui-input" required maxlength="300" placeholder="文章标题" value="${escapeHtml(post?.title || "")}" />
              </div>
            </div>
            <div class="layui-form-item">
              <label class="layui-form-label" for="post-slug">Slug</label>
              <div class="layui-input-block">
                <input type="text" id="post-slug" class="layui-input" required maxlength="120" placeholder="url-slug" ${isNew ? "" : "readonly"} value="${escapeHtml(post?.slug || "")}" />
              </div>
            </div>
            <div class="layui-form-item">
              <label class="layui-form-label" for="post-category">分类</label>
              <div class="layui-input-block category-picker">
                <select id="post-category" lay-filter="post-category">${categoryOptions}</select>
                <button type="button" class="layui-btn layui-btn-sm layui-btn-primary category-add-btn" id="add-category-btn" title="管理分类">
                  <i class="layui-icon layui-icon-add-1"></i>
                </button>
              </div>
            </div>
            <div class="layui-form-item">
              <label class="layui-form-label" for="post-tags">标签</label>
              <div class="layui-input-block">
                <input type="text" id="post-tags" class="layui-input" placeholder="逗号分隔，如 rust, web" value="${escapeHtml((post?.tags || []).join(", "))}" />
              </div>
            </div>
            <div class="layui-form-item layui-form-text">
              <label class="layui-form-label" for="post-summary">摘要</label>
              <div class="layui-input-block">
                <textarea id="post-summary" class="layui-textarea" required maxlength="500" placeholder="一句话摘要，用于列表展示">${escapeHtml(post?.summary || "")}</textarea>
              </div>
            </div>

            <div class="blog-edit-body">
              <div id="vditor" class="vditor-host"></div>
            </div>
            <p class="form-error blog-edit-error" id="edit-error" hidden></p>
          </form>
        </div>
      </div>`;

    const selectEl = document.getElementById("post-category");
    bindCategoryAdd(categories, selectEl);
    Docbit.UI.renderForm("select");

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
        Docbit.UI.success("文章已保存");
      } catch (err) {
        errEl.textContent = err.message;
        errEl.hidden = false;
      }
    });

    document.getElementById("delete-post-btn")?.addEventListener("click", async () => {
      const ok = await Docbit.UI.confirm("确定删除这篇文章？删除后不可恢复。");
      if (!ok) return;
      try {
        await Docbit.Api.del(`/api/blog/${encodeURIComponent(post.slug)}`, true);
        window.removeEventListener("resize", onResize);
        editorWindowResize = null;
        Docbit.Router.navigate("/blog/write");
        Docbit.UI.success("文章已删除");
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
