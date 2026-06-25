/* Docbit — SPA bootstrap */
(function () {
  "use strict";

  const { escapeHtml } = Docbit.Utils;

  let siteInfo = null;
  let prevRoute = null;
  let rendering = false;

  function isDocsSoftNav(route, prev) {
    return (
      route.page === "docs" &&
      prev?.page === "docs" &&
      route.slug === prev.slug &&
      route.docPath &&
      route.docPath !== prev.docPath
    );
  }

  function parseRoute(path) {
    const parts = path.replace(/\/+$/, "").split("/").filter(Boolean);
    if (parts.length === 0) return { page: "home", pageKey: "home" };
    if (parts[0] === "login") return { page: "login", pageKey: "auth" };
    if (parts[0] === "register") return { page: "register", pageKey: "auth" };
    if (parts[0] === "forgot-password") return { page: "forgot", pageKey: "auth" };
    if (parts[0] === "reset-password") {
      const token = new URLSearchParams(window.location.search).get("token") || "";
      return { page: "reset", pageKey: "auth", token };
    }
    if (parts[0] === "blog") {
      if (parts[1] === "write") {
        const editSlug = parts[2] || null;
        return editSlug && editSlug !== "new"
          ? { page: "blog-edit", pageKey: "blog", slug: editSlug }
          : editSlug === "new"
            ? { page: "blog-edit", pageKey: "blog", slug: "new" }
            : { page: "blog-write", pageKey: "blog" };
      }
      return parts[1]
        ? { page: "blog-post", pageKey: "blog", slug: parts[1] }
        : { page: "blog", pageKey: "blog" };
    }
    if (parts[0] === "about") return { page: "about", pageKey: "about" };
    if (parts[0] === "works" && parts[1]) {
      if (parts[2] === "docs") {
        const docPath = parts.slice(3).join("/");
        return { page: "docs", pageKey: "docs", slug: parts[1], docPath: docPath || null };
      }
      return { page: "work", pageKey: "work", slug: parts[1] };
    }
    return { page: "home", pageKey: "home" };
  }

  function navigate(path, replace) {
    if (replace) {
      window.history.replaceState(null, "", path);
    } else {
      window.history.pushState(null, "", path);
    }
    render();
  }

  function setActiveNav(page) {
    document.querySelectorAll(".main-nav a").forEach((a) => {
      const href = a.getAttribute("href");
      const active =
        (page === "home" && href === "/") ||
        (page.startsWith("work") && href === "/") ||
        (page.startsWith("docs") && href === "/") ||
        (page.startsWith("blog") && href === "/blog") ||
        (page === "about" && href === "/about");
      a.classList.toggle("active", active);
    });
  }

  function showLoading() {
    document.getElementById("app").innerHTML =
      '<div class="loading-state"><div class="spinner"></div><span>加载中…</span></div>';
  }

  function showError(msg) {
    document.getElementById("app").innerHTML =
      `<div class="error-box"><strong>加载失败</strong><p>${escapeHtml(msg)}</p>
       <div class="btn-row" style="justify-content:center;margin-top:1rem">
         <a class="btn btn-primary" href="/" data-nav>返回首页</a>
       </div></div>`;
  }

  async function loadSite() {
    try {
      siteInfo = await Docbit.Api.get("/api/site");
      const brand = document.getElementById("brand-name");
      if (brand && siteInfo.title) brand.textContent = siteInfo.title;
    } catch (_) {
      siteInfo = { title: "Docbit", tagline: "", author: "", bio: "", links: {} };
    }
  }

  async function render() {
    if (rendering) return;
    rendering = true;
    try {
    const route = parseRoute(window.location.pathname);
    setActiveNav(route.page);
    const softDocs = isDocsSoftNav(route, prevRoute);

    if (!softDocs) {
      showLoading();
      window.scrollTo(0, 0);
    }

    const app = document.getElementById("app");
    const shellPages = ["docs", "blog", "blog-post", "blog-write", "blog-edit"];
    document.body.classList.toggle(
      "layout-shell",
      shellPages.includes(route.page) && route.page !== "blog-edit"
    );
    document.body.classList.toggle("layout-home", route.page === "home");
    document.body.classList.toggle("layout-blog-edit", route.page === "blog-edit");
    app.classList.remove("docs-page", "blog-page");

    try {
      await Docbit.Loader.ensurePage(route.pageKey);
      if (prevRoute?.page === "blog-edit" && route.page !== "blog-edit") {
        Docbit.Pages.blog?.destroyEditor?.();
      }

      switch (route.page) {
        case "home":
          await Docbit.Pages.home.render(siteInfo);
          break;
        case "work":
          await Docbit.Pages.work.render(route.slug);
          break;
        case "docs":
          if (softDocs && (await Docbit.Pages.docs.navigateTo(route.slug, route.docPath))) {
            break;
          }
          await Docbit.Pages.docs.render(route.slug, route.docPath, navigate);
          break;
        case "blog":
          await Docbit.Pages.blog.renderList(siteInfo);
          break;
        case "blog-post":
          await Docbit.Pages.blog.renderPost(route.slug, siteInfo);
          break;
        case "blog-write":
          await Docbit.Pages.blog.renderWriteList();
          break;
        case "blog-edit":
          await Docbit.Pages.blog.renderEditor(route.slug);
          break;
        case "about":
          Docbit.Pages.about.render(siteInfo);
          break;
        case "login":
          Docbit.Pages.auth.renderLogin(
            new URLSearchParams(window.location.search).get("redirect") || "/"
          );
          break;
        case "register":
          Docbit.Pages.auth.renderRegister();
          break;
        case "forgot":
          Docbit.Pages.auth.renderForgot();
          break;
        case "reset":
          Docbit.Pages.auth.renderReset(route.token);
          break;
        default:
          await Docbit.Pages.home.render(siteInfo);
      }
    } catch (err) {
      if (!softDocs) showError(err.message);
    }
    Docbit.Footer?.render?.(siteInfo);
    prevRoute = { ...route };
    } finally {
      rendering = false;
    }
  }

  function initBackToTop() {
    const btn = document.getElementById("back-to-top");
    if (!btn) return;
    window.addEventListener(
      "scroll",
      () => {
        btn.hidden = window.scrollY < 400;
      },
      { passive: true }
    );
    btn.addEventListener("click", () => {
      window.scrollTo({ top: 0, behavior: "smooth" });
    });
  }

  function initMobileNav() {
    const toggle = document.getElementById("nav-toggle");
    const nav = document.getElementById("main-nav");
    toggle?.addEventListener("click", () => {
      const open = nav.classList.toggle("open");
      toggle.classList.toggle("open", open);
      toggle.setAttribute("aria-expanded", String(open));
    });
    nav?.querySelectorAll("a").forEach((a) => {
      a.addEventListener("click", () => {
        nav.classList.remove("open");
        toggle?.classList.remove("open");
        toggle?.setAttribute("aria-expanded", "false");
      });
    });
  }

  function initNavClicks() {
    document.addEventListener("click", (e) => {
      const link = e.target.closest("a[data-nav], a[href^='/']");
      if (!link) return;
      const href = link.getAttribute("href");
      if (!href || !href.startsWith("/") || href.startsWith("//")) return;
      if (link.getAttribute("target") === "_blank") return;
      if (href.includes("#") && href.split("#")[0] === window.location.pathname) return;
      e.preventDefault();
      const path = href.split("#")[0];
      const search = href.includes("?") ? href.slice(href.indexOf("?")) : "";
      const target = path + search;
      if (target === window.location.pathname + window.location.search) return;
      navigate(target);
      if (href.includes("#")) {
        requestAnimationFrame(() => {
          const el = document.getElementById(href.split("#")[1]);
          el?.scrollIntoView({ behavior: "smooth" });
        });
      }
    });
    window.addEventListener("popstate", render);
  }

  document.addEventListener("DOMContentLoaded", async () => {
    Docbit.Theme.init();
    Docbit.Markdown.init();
    initBackToTop();
    initMobileNav();
    initNavClicks();
    await Docbit.Auth.init();
    await loadSite();
    await render();
  });

  window.Docbit = window.Docbit || {};
  Docbit.Router = { navigate, parseRoute, render };
})();
