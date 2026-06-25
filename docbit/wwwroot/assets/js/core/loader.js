/* Docbit — dynamic page asset loader */
(function () {
  "use strict";

  const loadedCss = new Set(["/app.css"]);
  const loadedJs = new Set();

  const SHELL_CSS = "/pages/shell/shell.css";
  const SHELL_JS = "/pages/shell/shell.js";
  const SHELL_PAGES = new Set(["docs", "blog"]);

  function loadCSS(href) {
    if (loadedCss.has(href)) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const link = document.createElement("link");
      link.rel = "stylesheet";
      link.href = href;
      link.onload = () => {
        loadedCss.add(href);
        resolve();
      };
      link.onerror = () => reject(new Error(`Failed to load CSS: ${href}`));
      document.head.appendChild(link);
    });
  }

  function loadScript(src) {
    if (loadedJs.has(src)) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = src;
      script.onload = () => {
        loadedJs.add(src);
        resolve();
      };
      script.onerror = () => reject(new Error(`Failed to load script: ${src}`));
      document.body.appendChild(script);
    });
  }

  const PAGE_ASSETS = {
    home: { css: "/pages/home/home.css", js: "/pages/home/home.js" },
    work: { css: "/pages/work/work.css", js: "/pages/work/work.js" },
    docs: { css: "/pages/docs/docs.css", js: "/pages/docs/docs.js" },
    blog: { css: "/pages/blog/blog.css", js: "/pages/blog/blog.js" },
    about: { css: "/pages/about/about.css", js: "/pages/about/about.js" },
    auth: { css: "/pages/auth/auth.css", js: "/pages/auth/auth.js" },
  };

  async function ensurePage(pageKey) {
    const assets = PAGE_ASSETS[pageKey];
    if (!assets) return;
    const tasks = [];
    if (SHELL_PAGES.has(pageKey)) {
      tasks.push(loadCSS(SHELL_CSS));
      tasks.push(loadScript(SHELL_JS));
    }
    if (assets.css) tasks.push(loadCSS(assets.css));
    if (assets.js) tasks.push(loadScript(assets.js));
    await Promise.all(tasks);
  }

  window.Docbit = window.Docbit || {};
  Docbit.Loader = { loadCSS, loadScript, ensurePage, PAGE_ASSETS };
})();
