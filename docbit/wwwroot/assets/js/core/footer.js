/* Docbit — unified site footer (mockup-aligned) */
(function () {
  "use strict";

  const { escapeHtml, LOGOS } = Docbit.Utils;

  function beianHtml(f) {
    const icp = f.icp || "粤ICP备2023105607号-1";
    const label = f.site_label || "技术分享";
    const siteUrl = f.site_url || "lusida.net";
    const href = siteUrl.startsWith("http") ? siteUrl : `https://${siteUrl}`;
    return `<a href="https://beian.miit.gov.cn/" target="_blank" rel="noopener noreferrer">${escapeHtml(icp)}</a>
      <span class="footer-sep">·</span>
      <span>${escapeHtml(label)}</span>
      <span class="footer-sep">·</span>
      <a href="${escapeHtml(href)}" target="_blank" rel="noopener noreferrer">${escapeHtml(siteUrl)}</a>`;
  }

  function render(site) {
    const footer = document.getElementById("site-footer");
    if (!footer) return;

    const f = site?.footer || {};

    footer.className = "site-footer";
    footer.innerHTML = `
      <div class="footer-inner site-footer-inner">
        <div class="footer-brand">
          <span class="footer-rocket" aria-hidden="true">🚀</span>
          <div>
            <strong class="footer-motto">${escapeHtml(f.motto || "持续构建 · 不断探索 · 追求卓越")}</strong>
            <p class="footer-tagline">${escapeHtml(f.tagline || "用 Rust 构建更好的未来")}</p>
          </div>
        </div>
        <div class="footer-beian footer-beian--center">${beianHtml(f)}</div>
        <div class="footer-meta">
          <p class="footer-built">Built with <strong>rust-webapp</strong> · Rust</p>
          <p class="footer-copy-row">
            <span>${escapeHtml(f.copyright || "© 2024 Start. All rights reserved.")}</span>
            <img src="${LOGOS.brand}" alt="" class="footer-logo" width="22" height="22" />
          </p>
        </div>
      </div>`;
  }

  window.Docbit = window.Docbit || {};
  Docbit.Footer = { render };
})();
