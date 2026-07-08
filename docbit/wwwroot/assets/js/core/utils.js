/* Docbit — shared utilities */
(function () {
  "use strict";

  function escapeHtml(str) {
    const div = document.createElement("div");
    div.textContent = str == null ? "" : String(str);
    return div.innerHTML;
  }

  function categoryBadge(cat) {
    const labels = { framework: "框架", product: "产品", article: "文章" };
    return `<span class="badge badge-${escapeHtml(cat)}">${labels[cat] || escapeHtml(cat)}</span>`;
  }

  function tagList(tags) {
    return (tags || [])
      .map((t) => `<span class="tag">${escapeHtml(t)}</span>`)
      .join("");
  }

  function blogCategoryLabel(id) {
    const labels = {
      rust: "Rust 生态",
      webapi: "Web 开发",
      tutorial: "教程实践",
      portfolio: "作品集",
      news: "动态资讯",
    };
    return labels[id] || id;
  }

  const LOGOS = {
    site: "/assets/logo-32.svg",
    hero: "/assets/logo-64.svg",
    heroAccent: "/assets/logo-64.svg",
    heroBg: "/assets/logo-128.svg",
    brand: "/assets/logo-32.svg",
    workRust: "/assets/works/rust-webx.svg",
    workDocbit: "/assets/works/docbit.svg",
  };

  function workLogoUrl(work) {
    if (!work) return null;
    return work.logo_url || null;
  }

  function docsLogoUrl(docsSlug, work) {
    if (work?.logo_url) return work.logo_url;
    if (docsSlug) return `/assets/works/${docsSlug}.svg`;
    return null;
  }

  function logoImg(src, className, size) {
    if (!src) return "";
    const s = size || 32;
    return `<img src="${src}" alt="" class="${className}" width="${s}" height="${s}" loading="lazy" />`;
  }

  /** Subtle logo watermark — no gradient overlays */
  function pageDecoHtml(placement) {
    const place = placement || "br";
    return `
      <div class="page-deco page-deco--${place}" aria-hidden="true">
        <img src="${LOGOS.heroBg}" alt="" class="page-deco-logo" width="320" height="320" loading="lazy" />
      </div>`;
  }

  window.Docbit = window.Docbit || {};
  Docbit.Utils = {
    escapeHtml,
    categoryBadge,
    tagList,
    blogCategoryLabel,
    LOGOS,
    workLogoUrl,
    docsLogoUrl,
    logoImg,
    pageDecoHtml,
  };
})();
