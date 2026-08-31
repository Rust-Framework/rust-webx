/* Work detail page */
(function () {
  "use strict";

  const { escapeHtml, categoryBadge, tagList, workLogoUrl, pageDecoHtml } = Docbit.Utils;

  async function render(slug) {
    const work = await Docbit.Api.get(`/api/exhibitions/${encodeURIComponent(slug)}`);
    const hasDocs = work.docs_slug && work.docs_slug.length > 0;
    const logo = workLogoUrl(work);

    document.getElementById("app").innerHTML = `
      <div class="page page-work">
        ${pageDecoHtml("work")}
        <div class="page-work-inner">
        <div class="detail-header">
          ${logo ? `<div class="detail-logo-row"><img src="${logo}" alt="" class="logo-wide" height="36" loading="lazy" />${categoryBadge(work.category)}</div>` : `<div class="detail-meta">${categoryBadge(work.category)}</div>`}
          <h1>${escapeHtml(work.title)}</h1>
          <div class="meta">${escapeHtml(work.subtitle)}</div>
          ${(work.tags || []).length ? `<div class="tag-row">${tagList(work.tags)}</div>` : ""}
        </div>
        <div class="detail-body">
          <p>${escapeHtml(work.description)}</p>
        </div>
        <div class="btn-row">
          ${hasDocs ? `<a class="layui-btn layui-btn-normal" href="/works/${escapeHtml(work.slug)}/docs" data-nav>查看文档</a>` : ""}
          ${work.repo_url ? `<a class="layui-btn layui-btn-primary" href="${escapeHtml(work.repo_url)}" target="_blank" rel="noopener">源码仓库</a>` : ""}
          ${work.demo_url ? `<a class="layui-btn layui-btn-primary" href="${escapeHtml(work.demo_url)}" target="_blank" rel="noopener">在线演示</a>` : ""}
          <a class="layui-btn layui-btn-primary" href="/" data-nav>← 返回首页</a>
        </div>
        </div>
      </div>`;
    document.title = work.title + " — 作品";
  }

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.work = { render };
})();
