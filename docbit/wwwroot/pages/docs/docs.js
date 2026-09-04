/* Docs viewer page */
(function () {
  "use strict";

  const { escapeHtml, docsLogoUrl } = Docbit.Utils;
  const { render: renderMd, enhance, buildToc, initTocScrollSpy } = Docbit.Markdown;

  let cached = { workSlug: null, docsSlug: null, work: null, index: null };

  function brandHtml(docsSlug, title, work) {
    const logo = docsLogoUrl(docsSlug, work);
    if (!logo) {
      return `<div class="shell-brand"><h4>${escapeHtml(title)}</h4></div>`;
    }
    return `<div class="shell-brand">
      <span class="shell-brand-icon"><img src="${logo}" alt="" loading="lazy" /></span>
      <div>
        <span class="shell-brand-label">文档</span>
        <h4>${escapeHtml(title)}</h4>
      </div>
    </div>`;
  }

  function setActiveNav(docPath) {
    document.querySelectorAll(".doc-nav a").forEach((a) => {
      const href = a.getAttribute("href") || "";
      const path = href.includes("/docs/")
        ? decodeURIComponent(href.split("/docs/").slice(1).join("/docs/"))
        : "";
      a.classList.toggle("active", path === docPath);
    });
  }

  function updateToc(article) {
    const layout = document.getElementById("docs-layout");
    let tocSlot = document.getElementById("toc-slot");
    const tocHtml = buildToc(article);

    if (tocHtml) {
      if (!tocSlot) {
        tocSlot = document.createElement("aside");
        tocSlot.id = "toc-slot";
        tocSlot.className = "content-toc-panel docs-toc-panel";
        layout?.appendChild(tocSlot);
      }
      tocSlot.innerHTML = tocHtml;
      layout?.classList.add("has-toc");
      initTocScrollSpy();
    } else if (tocSlot) {
      tocSlot.remove();
      layout?.classList.remove("has-toc");
    }
  }

    async function loadArticle(docsSlug, docPath) {
    const article = document.getElementById("doc-article");
    if (!article) return;

    article.classList.add("is-swapping");
    try {
      let content = { content: "# 暂无文档\n\n请在 `docs/` 目录添加 Markdown 文件。" };
      if (docPath) {
        content = await Docbit.Api.get(Docbit.Api.docContentUrl(docsSlug, docPath));
      }
      article.innerHTML = renderMd(content.content);
      enhance(article, { workSlug: cached.workSlug, docPath });
      setActiveNav(docPath);
      updateToc(article);
      if (cached.work?.title) {
        document.title = cached.work.title + " — 文档";
      }
    } catch (err) {
      article.innerHTML = `<div class="error-box"><strong>无法加载文档</strong><p>${escapeHtml(
        err.message || String(err)
      )}</p><p class="meta">${escapeHtml(docPath || "")}</p></div>`;
      setActiveNav(docPath);
      updateToc(article);
    }
    article.classList.remove("is-swapping");
    Docbit.Shell?.scrollMain?.(0);
  }

  async function navigateTo(workSlug, docPath) {
    if (
      cached.workSlug !== workSlug ||
      !cached.index ||
      !cached.work ||
      !cached.docsSlug
    ) {
      return false;
    }
    await loadArticle(cached.docsSlug, docPath);
    return true;
  }

  function resolveDocPath(index, docPath) {
    if (docPath) return docPath;
    const preferred = "FOREWORD.md";
    const hasPreferred = index.items.some(
      (i) => i.path === preferred || findPathInItems(index.items, preferred)
    );
    const first = findFirstDoc(index.items);
    return hasPreferred ? preferred : first;
  }

  async function render(workSlug, docPath) {
    const work = await Docbit.Api.get(`/api/exhibitions/${encodeURIComponent(workSlug)}`);
    const docsSlug = work.docs_slug || workSlug;
    const index = await Docbit.Api.get(`/api/docs/${encodeURIComponent(docsSlug)}/index`);

    cached = { workSlug, docsSlug, work, index };

    const targetPath = resolveDocPath(index, docPath);
    if (!docPath && targetPath) {
      window.history.replaceState(
        null,
        "",
        `/works/${encodeURIComponent(workSlug)}/docs/${Docbit.Api.encodeDocPath(targetPath)}`
      );
      docPath = targetPath;
    }

    let content = { content: "# 暂无文档\n\n请在 `docs/` 目录添加 Markdown 文件。" };
    let contentError = null;
    if (docPath) {
      try {
        content = await Docbit.Api.get(Docbit.Api.docContentUrl(docsSlug, docPath));
      } catch (err) {
        contentError = err.message || String(err);
      }
    }

    const articleHtml = contentError
      ? `<div class="error-box"><strong>无法加载文档</strong><p>${escapeHtml(contentError)}</p><p class="meta">${escapeHtml(
          docPath || ""
        )}</p></div>`
      : renderMd(content.content);

    const navHtml = renderDocNav(index.items, workSlug, docPath);

    Docbit.Shell.mount(
      Docbit.Shell.layout({
        id: "docs-layout",
        className: "page-docs",
        sidebarId: "docs-sidebar",
        withToc: false,
        sidebarHeader: brandHtml(docsSlug, index.title, work),
        sidebarBody: `<ul class="doc-nav">${navHtml}</ul>`,
        breadcrumb: `
          <a href="/" data-nav>首页</a>
          <span class="sep">/</span>
          <a href="/works/${escapeHtml(workSlug)}" data-nav>${escapeHtml(work.title)}</a>
          <span class="sep">/</span>
          <span class="current">文档</span>`,
        content: `<article class="markdown-body shell-article" id="doc-article">${articleHtml}</article>`,
      })
    );

    const article = document.getElementById("doc-article");
    enhance(article, { workSlug, docPath });
    updateToc(article);
    Docbit.UI.renderForm();
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

  function renderDocNav(items, workSlug, activePath, depth = 0) {
    return (items || [])
      .map((item) => {
        if (item.path) {
          const active = item.path === activePath ? " active" : "";
          const href = `/works/${escapeHtml(workSlug)}/docs/${Docbit.Api.encodeDocPath(item.path)}`;
          return `<li class="nav-leaf"><a href="${href}" class="${active.trim()}" data-nav>${escapeHtml(item.title)}</a></li>`;
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

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.docs = { render, navigateTo };
})();
