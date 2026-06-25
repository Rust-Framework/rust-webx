/* Home page — portfolio mockup layout */
(function () {
  "use strict";

  const { escapeHtml, tagList, LOGOS, workLogoUrl } = Docbit.Utils;

  const FILTERS = [
    { id: "all", label: "全部", test: () => true },
    {
      id: "rust",
      label: "Rust",
      test: (w) => (w.tags || []).some((t) => String(t).toLowerCase().includes("rust")),
    },
    {
      id: "web",
      label: "Web",
      test: (w) =>
        (w.tags || []).some((t) =>
          ["webapi", "webapplication", "web"].includes(String(t).toLowerCase())
        ),
    },
    {
      id: "fullstack",
      label: "全栈",
      test: (w) => w.category === "product",
    },
    {
      id: "tools",
      label: "工具",
      test: (w) => w.category === "framework" || w.category === "tool",
    },
  ];

  function categoryLabel(cat) {
    const map = {
      framework: "框架",
      product: "Web",
      tool: "工具",
      service: "服务",
      article: "文章",
    };
    return map[cat] || cat;
  }

  function categoryClass(cat) {
    const map = {
      framework: "cat-framework",
      product: "cat-web",
      tool: "cat-tool",
      service: "cat-service",
    };
    return map[cat] || "cat-default";
  }

  function tagPills(tagline) {
    return (tagline || "Rust · Web · Full Stack")
      .split("·")
      .map((t) => t.trim())
      .filter(Boolean)
      .map((t) => `<span class="hero-pill">${escapeHtml(t)}</span>`)
      .join("");
  }

  function workCard(w) {
    const logo = workLogoUrl(w);
    const demo =
      w.demo_url ||
      (w.docs_slug || w.slug ? `/works/${encodeURIComponent(w.slug)}/docs` : null);
    const repo = w.repo_url || "";
    return `
    <article class="work-card-v2" data-category="${escapeHtml(w.category)}" data-slug="${escapeHtml(w.slug)}">
      <a href="/works/${escapeHtml(w.slug)}" class="work-card-v2-stretch" data-nav aria-label="${escapeHtml(w.title)}"></a>
      <div class="work-card-v2-head">
        <div class="work-card-v2-title-row">
          ${logo ? `<span class="work-card-v2-icon"><img src="${logo}" alt="" loading="lazy" /></span>` : ""}
          <h3><a href="/works/${escapeHtml(w.slug)}" data-nav>${escapeHtml(w.title)}</a></h3>
        </div>
        <span class="work-card-v2-cat ${categoryClass(w.category)}">${escapeHtml(categoryLabel(w.category))}</span>
      </div>
      <p class="work-card-v2-desc">${escapeHtml(w.description)}</p>
      ${(w.tags || []).length ? `<div class="work-card-v2-tags">${tagList(w.tags.slice(0, 4))}</div>` : ""}
      <div class="work-card-v2-links">
        ${demo ? `<a href="${escapeHtml(demo)}" class="work-chip-link"${demo.startsWith("http") ? ' target="_blank" rel="noopener"' : " data-nav"}>Live Demo</a>` : ""}
        ${repo ? `<a href="${escapeHtml(repo)}" class="work-chip-link" target="_blank" rel="noopener">GitHub</a>` : ""}
      </div>
    </article>`;
  }

  function bindWorkGridNavigation() {
    const grid = document.getElementById("works-grid");
    if (!grid || grid.dataset.navBound) return;
    grid.dataset.navBound = "1";
    grid.addEventListener("click", (e) => {
      const card = e.target.closest(".work-card-v2");
      if (!card) return;
      if (e.target.closest("a, button")) return;
      const slug = card.dataset.slug;
      if (slug) Docbit.Router.navigate(`/works/${encodeURIComponent(slug)}`);
    });
  }

  function bindFilters(works) {
    const grid = document.getElementById("works-grid");
    if (!grid) return;

    function applyFilter(id) {
      const filter = FILTERS.find((f) => f.id === id) || FILTERS[0];
      const filtered = works.filter((w) => filter.test(w));
      grid.innerHTML = filtered.length
        ? filtered.map(workCard).join("")
        : '<p class="empty-hint">该分类下暂无作品。</p>';
    }

    Docbit.UI.loadLayui().then((layui) => {
      layui.element.render("tab", "works-filter");
      layui.element.on("tab(works-filter)", (data) => {
        const li = document.querySelectorAll(".works-filter-tabs .layui-tab-title li")[data.index];
        if (li) applyFilter(li.dataset.filter);
      });
    });
  }

  async function render(siteInfo) {
    const works = await Docbit.Api.get("/api/works");
    const sorted = [...works].sort((a, b) => (a.sort_order || 0) - (b.sort_order || 0));
    const site = siteInfo || {};
    const stats = site.stats || {};

    document.getElementById("app").innerHTML = `
      <div class="page page-home">
        <section class="home-hero">
          <div class="home-hero-glow" aria-hidden="true"></div>
          <div class="home-hero-grid">
            <div class="home-hero-logo" aria-hidden="true">
              <div class="hero-visual-glow"></div>
              <img src="${LOGOS.heroBg}" alt="" class="hero-visual-logo" />
            </div>
            <div class="home-hero-text">
              <h1>${escapeHtml(site.title || "Start 的作品")}</h1>
              <div class="hero-pills">${tagPills(site.tagline)}</div>
              <p class="home-hero-bio">${escapeHtml(site.bio || "个人开发者作品集")}</p>
              <p class="home-hero-sub">${escapeHtml(site.hero_subtitle || "构建高性能、可靠、未来感的 Web 应用")}</p>
            </div>
            <div class="home-hero-stats">
              <div class="hero-stat-item"><strong>${sorted.length}</strong><span>项目数量</span></div>
              <div class="hero-stat-item"><strong>${escapeHtml(stats.stacks || "5")}</strong><span>技术栈</span></div>
              <div class="hero-stat-item"><strong>${escapeHtml(stats.commits || "2.1k+")}</strong><span>代码提交</span></div>
              <div class="hero-stat-item"><strong>${escapeHtml(stats.rating || "98%")}</strong><span>好评率</span></div>
            </div>
          </div>
        </section>

        <section class="home-works-section">
          <div class="home-works-header">
            <h2 class="home-works-title"><span class="title-icon" aria-hidden="true">📦</span> 精选作品</h2>
            <div class="layui-tab layui-tab-brief works-filter-tabs" lay-filter="works-filter" id="works-filter">
              <ul class="layui-tab-title">
                ${FILTERS.map(
                  (f, i) =>
                    `<li class="${i === 0 ? "layui-this" : ""}" data-filter="${f.id}">${escapeHtml(f.label)}</li>`
                ).join("")}
              </ul>
            </div>
          </div>
          <div class="works-grid-v2" id="works-grid">
            ${sorted.length ? sorted.map(workCard).join("") : '<p class="empty-hint">暂无作品，请在 docs/ 目录添加 INDEX.json。</p>'}
          </div>
        </section>
      </div>`;

    bindFilters(sorted);
    bindWorkGridNavigation();
    document.title = (site.title || "Start 的作品") + " — 作品集";
  }

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.home = { render };
})();
