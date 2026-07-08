/* About page — resume-style profile */
(function () {
  "use strict";

  const { escapeHtml, LOGOS, tagList } = Docbit.Utils;

  /* 默认技能栈 — 简历型展示 */
  const DEFAULT_SKILLS = [
    { name: "Rust", level: 88, note: "WebApi · Axum · Tokio" },
    { name: "TypeScript / JS", level: 92, note: "SPA · Vditor · Layui" },
    { name: "C# / .NET", level: 78, note: "ASP.NET Core 中介者模式" },
    { name: "HTML / CSS", level: 90, note: "响应式 · 主题切换" },
    { name: "SQLite / PostgreSQL", level: 75, note: "迁移 · WAL · 索引调优" },
  ];

  /* 默认经历时间线 */
  const DEFAULT_TIMELINE = [
    {
      period: "2024 — 至今",
      role: "独立全栈开发者",
      org: "Start World",
      desc: "基于 Rust 构建 WebApplication 框架，涵盖 WebApi、SPA、博客与文档系统，沉淀个人作品集。",
      tags: ["Rust", "WebApi", "SPA", "Layui"],
    },
    {
      period: "2023 — 2024",
      role: "技术探索 · 框架迁移",
      org: "Open Source",
      desc: "将 ASP.NET Core 的中介者模式与依赖注入体系迁移至 Rust，沉淀 LRWF 框架与 LDeep IDE 架构。",
      tags: ["LRWF", "IDE", "Architecture"],
    },
    {
      period: "2022 — 2023",
      role: "前端工程化实践",
      org: "Web 平台",
      desc: "推进多端小程序与 Web SPA 的工程化，沉淀 Taro 跨端方案与组件库。",
      tags: ["Taro", "Mini-App", "SPA"],
    },
  ];

  function taglineList(tagline) {
    return (tagline || "Rust · Web · Full Stack")
      .split(/[·|]/)
      .map((t) => t.trim())
      .filter(Boolean);
  }

  function skillBar(skill) {
    return `
      <div class="resume-skill">
        <div class="resume-skill-head">
          <span class="resume-skill-name">${escapeHtml(skill.name)}</span>
          <span class="resume-skill-pct">${Number(skill.level) || 0}%</span>
        </div>
        <div class="resume-skill-bar" aria-hidden="true">
          <span class="resume-skill-fill" style="width:${Math.min(100, Math.max(0, Number(skill.level) || 0))}%"></span>
        </div>
        ${skill.note ? `<p class="resume-skill-note">${escapeHtml(skill.note)}</p>` : ""}
      </div>`;
  }

  function timelineItem(item) {
    return `
      <li class="resume-tl-item">
        <div class="resume-tl-dot" aria-hidden="true"></div>
        <div class="resume-tl-body">
          <div class="resume-tl-period">${escapeHtml(item.period)}</div>
          <h3 class="resume-tl-role">${escapeHtml(item.role)}</h3>
          <div class="resume-tl-org">${escapeHtml(item.org)}</div>
          <p class="resume-tl-desc">${escapeHtml(item.desc)}</p>
          ${item.tags?.length ? `<div class="resume-tl-tags">${tagList(item.tags)}</div>` : ""}
        </div>
      </li>`;
  }

  function render(siteInfo) {
    const site = siteInfo || {};
    const stats = site.stats || {};
    const footer = site.footer || {};
    const links = site.links || {};

    const name = site.author || site.title || "Start";
    const tagline = site.tagline || "Rust · Web · Full Stack";
    const bio = site.bio || "个人开发者作品集";
    const heroSubtitle = site.hero_subtitle || "构建高性能、可靠、未来感的 Web 应用";
    const motto = footer.motto || "持续构建 · 不断探索 · 追求卓越";
    const years = footer.years || "2年+";
    const expCount = footer.experience || "100+";
    const stacks = footer.stacks || "5+";

    const skills = DEFAULT_SKILLS;
    const timeline = DEFAULT_TIMELINE;
    const pills = taglineList(tagline);

    document.getElementById("app").innerHTML = `
      <div class="page page-about">
        ${Docbit.Utils.pageDecoHtml("br")}

        <div class="resume">
          <!-- 头部：头像 + 基本信息 + 联系方式 -->
          <header class="resume-hero">
            <div class="resume-hero-glow" aria-hidden="true"></div>
            <div class="resume-hero-inner">
              <div class="resume-avatar" aria-hidden="true">
                <div class="resume-avatar-glow"></div>
                <img src="${LOGOS.heroBg}" alt="" class="resume-avatar-img" />
              </div>
              <div class="resume-hero-text">
                <h1 class="resume-name">${escapeHtml(name)}</h1>
                <div class="resume-pills">
                  ${pills.map((p) => `<span class="resume-pill">${escapeHtml(p)}</span>`).join("")}
                </div>
                <p class="resume-subtitle">${escapeHtml(heroSubtitle)}</p>
                <p class="resume-bio">${escapeHtml(bio)}</p>
                <p class="resume-motto">“${escapeHtml(motto)}”</p>
              </div>
            </div>
          </header>

          <!-- 关键数据 -->
          <section class="resume-stats">
            <div class="resume-stat">
              <strong>${escapeHtml(years)}</strong>
              <span>开发经验</span>
            </div>
            <div class="resume-stat">
              <strong>${escapeHtml(expCount)}</strong>
              <span>项目积累</span>
            </div>
            <div class="resume-stat">
              <strong>${escapeHtml(stacks)}</strong>
              <span>技术栈</span>
            </div>
            <div class="resume-stat">
              <strong>${escapeHtml(stats.commits || "2.1k+")}</strong>
              <span>代码提交</span>
            </div>
            <div class="resume-stat">
              <strong>${escapeHtml(stats.rating || "98%")}</strong>
              <span>好评率</span>
            </div>
          </section>

          <!-- 主体：左侧技能 + 右侧时间线 -->
          <div class="resume-body">
            <aside class="resume-side">
              <section class="resume-card">
                <h2 class="resume-card-title">
                  <i class="layui-icon layui-icon-component" aria-hidden="true"></i>
                  <span>技能栈</span>
                </h2>
                <div class="resume-skills">
                  ${skills.map(skillBar).join("")}
                </div>
              </section>

              <section class="resume-card">
                <h2 class="resume-card-title">
                  <i class="layui-icon layui-icon-link" aria-hidden="true"></i>
                  <span>联系方式</span>
                </h2>
                <ul class="resume-contact">
                  ${links.github ? `
                    <li>
                      <i class="layui-icon layui-icon-username" aria-hidden="true"></i>
                      <a href="${escapeHtml(links.github)}" target="_blank" rel="noopener">GitHub</a>
                    </li>` : ""}
                  ${links.docs ? `
                    <li>
                      <i class="layui-icon layui-icon-read" aria-hidden="true"></i>
                      <a href="${escapeHtml(links.docs)}" target="_blank" rel="noopener">在线文档</a>
                    </li>` : ""}
                  ${footer.site_url ? `
                    <li>
                      <i class="layui-icon layui-icon-website" aria-hidden="true"></i>
                      <a href="${footer.site_url.startsWith("http") ? escapeHtml(footer.site_url) : "https://" + escapeHtml(footer.site_url)}" target="_blank" rel="noopener">${escapeHtml(footer.site_url)}</a>
                    </li>` : ""}
                  <li>
                    <i class="layui-icon layui-icon-location" aria-hidden="true"></i>
                    <span>${escapeHtml(footer.site_label || "技术分享")}</span>
                  </li>
                </ul>
              </section>
            </aside>

            <main class="resume-main">
              <section class="resume-card">
                <h2 class="resume-card-title">
                  <i class="layui-icon layui-icon-time" aria-hidden="true"></i>
                  <span>经历时间线</span>
                </h2>
                <ul class="resume-timeline">
                  ${timeline.map(timelineItem).join("")}
                </ul>
              </section>

              <section class="resume-card">
                <h2 class="resume-card-title">
                  <i class="layui-icon layui-icon-about" aria-hidden="true"></i>
                  <span>关于本站</span>
                </h2>
                <div class="resume-about-text">
                  <p>本站由 <strong>rust-webx</strong> 框架驱动，采用 Rust 编写后端 WebApi，前端为原生 SPA 架构，集成 Layui 组件库与 Vditor 编辑器。</p>
                  <p>内容涵盖技术博客、开源文档与作品集，全部数据存储于 SQLite，支持博客评论、邮箱重置密码等完整功能。</p>
                  <div class="resume-about-actions">
                    <a class="layui-btn layui-btn-normal" href="/" data-nav>
                      <i class="layui-icon layui-icon-home" aria-hidden="true"></i>
                      浏览作品
                    </a>
                    <a class="layui-btn layui-btn-primary" href="/blog" data-nav>
                      <i class="layui-icon layui-icon-read" aria-hidden="true"></i>
                      阅读博客
                    </a>
                  </div>
                </div>
              </section>
            </main>
          </div>
        </div>
      </div>`;

    document.title = "关于 — " + (site.brand_name || site.title || "Start World");
  }

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.about = { render };
})();
