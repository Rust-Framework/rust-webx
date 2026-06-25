/* About page */
(function () {
  "use strict";

  const { escapeHtml, LOGOS } = Docbit.Utils;

  function render(siteInfo) {
    const site = siteInfo || {};
    const links = site.links || {};
    document.getElementById("app").innerHTML = `
      <div class="page page-about">
        <div class="about-card">
          <img src="${LOGOS.heroAccent}" alt="" class="about-logo" width="56" height="56" />
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

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.about = { render };
})();
