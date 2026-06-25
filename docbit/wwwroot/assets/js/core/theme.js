/* Docbit — theme switching */
(function () {
  "use strict";

  const THEME_KEY = "docbit-theme";
  const HLJS_THEMES = {
    dark: "https://cdn.jsdelivr.net/npm/highlight.js@11.9.0/styles/github-dark.min.css",
    light: "https://cdn.jsdelivr.net/npm/highlight.js@11.9.0/styles/github.min.css",
  };

  function updateHljsTheme(theme) {
    const link = document.getElementById("hljs-theme");
    if (link) link.href = HLJS_THEMES[theme] || HLJS_THEMES.light;
  }

  function loadTheme() {
    const theme = localStorage.getItem(THEME_KEY) || "light";
    document.documentElement.setAttribute("data-theme", theme);
    updateHljsTheme(theme);
  }

  function setTheme(mode) {
    document.documentElement.setAttribute("data-theme", mode);
    localStorage.setItem(THEME_KEY, mode);
    updateHljsTheme(mode);
  }

  function init() {
    loadTheme();
    document.getElementById("theme-mode-btn")?.addEventListener("click", () => {
      const current = document.documentElement.getAttribute("data-theme");
      setTheme(current === "dark" ? "light" : "dark");
    });
  }

  window.Docbit = window.Docbit || {};
  Docbit.Theme = { init, setTheme, loadTheme };
})();
