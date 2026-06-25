/* Shared shell sidebar toggle */
(function () {
  "use strict";

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
  Docbit.Shell = { initShellSidebar };
})();
