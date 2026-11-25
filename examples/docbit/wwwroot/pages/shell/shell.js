/* Shared shell layout — Layui + sidebar grid */
(function () {
  "use strict";

  function topbar(breadcrumbHtml, actionsHtml) {
    return `
      <div class="shell-topbar">
        <button type="button" class="layui-btn layui-btn-sm layui-btn-primary shell-menu-btn" id="sidebar-open" title="打开目录">
          <i class="layui-icon layui-icon-spread-left"></i>
        </button>
        <nav class="shell-breadcrumb" aria-label="面包屑">${breadcrumbHtml}</nav>
        ${actionsHtml ? `<div class="shell-topbar-actions">${actionsHtml}</div>` : ""}
      </div>`;
  }

  function layout(opts) {
    const {
      id = "",
      className = "",
      sidebarId = "",
      sidebarHeader = "",
      sidebarExtra = "",
      sidebarBody = "",
      breadcrumb = "",
      actions = "",
      content = "",
      withToc = false,
    } = opts;

    const idAttr = id ? ` id="${id}"` : "";
    const sideId = sidebarId ? ` id="${sidebarId}"` : "";
    const tocHtml = withToc
      ? `<aside class="content-toc-panel docs-toc-panel" id="toc-slot"></aside>`
      : "";

    return `
      <div class="content-shell ${className}"${idAttr}>
        <div class="sidebar-overlay" id="sidebar-overlay"></div>
        <aside class="content-sidebar"${sideId}>
          <div class="content-sidebar-header">
            ${sidebarHeader}
            <button type="button" class="layui-btn layui-btn-sm layui-btn-primary sidebar-close" id="sidebar-close" title="关闭">
              <i class="layui-icon layui-icon-close"></i>
            </button>
          </div>
          ${sidebarExtra}
          <nav class="content-sidebar-body shell-nav">${sidebarBody}</nav>
        </aside>
        <div class="content-main">
          ${typeof Docbit !== "undefined" && Docbit.Utils?.pageDecoHtml ? Docbit.Utils.pageDecoHtml("br") : ""}
          <div class="content-main-inner">
          ${topbar(breadcrumb, actions)}
          ${content}
          </div>
        </div>
        ${tocHtml}
      </div>`;
  }

  function mount(html) {
    document.getElementById("app").innerHTML = html;
    initShellSidebar();
  }

  function scrollMain(opts) {
    const main = document.querySelector(".content-main");
    if (!main) return false;
    const top = typeof opts === "number" ? opts : opts?.top ?? 0;
    const behavior = typeof opts === "object" && opts?.behavior ? opts.behavior : "auto";
    main.scrollTo({ top, behavior });
    return true;
  }

  function initShellSidebar() {
    const sidebar = document.querySelector(".content-sidebar");
    const overlay = document.getElementById("sidebar-overlay");
    const openBtn = document.getElementById("sidebar-open");
    const closeBtn = document.getElementById("sidebar-close");
    if (!sidebar) return;

    function close() {
      sidebar.classList.remove("open");
      overlay?.classList.remove("visible");
    }
    function open() {
      sidebar.classList.add("open");
      overlay?.classList.add("visible");
    }

    openBtn?.addEventListener("click", open);
    closeBtn?.addEventListener("click", close);
    overlay?.addEventListener("click", close);
    sidebar.querySelectorAll("a").forEach((a) => a.addEventListener("click", close));
  }

  window.Docbit = window.Docbit || {};
  Docbit.Shell = { topbar, layout, mount, scrollMain, initShellSidebar };
})();
