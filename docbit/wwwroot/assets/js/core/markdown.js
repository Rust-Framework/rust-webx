/* Docbit — markdown rendering */
(function () {
  "use strict";

  const { escapeHtml } = Docbit.Utils;
  let md = null;

  function init() {
    if (typeof markdownit === "undefined") return null;

    md = markdownit({
      html: false,
      linkify: true,
      typographer: true,
      highlight: (str, lang) => {
        if (typeof hljs !== "undefined" && lang && hljs.getLanguage(lang)) {
          try {
            return hljs.highlight(str, { language: lang }).value;
          } catch (_) {}
        }
        if (typeof hljs !== "undefined") {
          try {
            return hljs.highlightAuto(str).value;
          } catch (_) {}
        }
        return escapeHtml(str);
      },
    });

    if (typeof markdownItAnchor !== "undefined") {
      const anchorOpts = { level: [1, 2, 3, 4] };
      try {
        if (markdownItAnchor.permalink?.ariaHidden) {
          anchorOpts.permalink = markdownItAnchor.permalink.ariaHidden({
            placement: "after",
            class: "header-anchor",
            symbol: "#",
          });
        }
        md.use(markdownItAnchor, anchorOpts);
      } catch (_) {}
    }
    return md;
  }

  function render(mdText) {
    if (!md) init();
    if (!md) return `<pre>${escapeHtml(mdText)}</pre>`;
    const raw = md.render(mdText);
    if (typeof DOMPurify !== "undefined") {
      return DOMPurify.sanitize(raw, {
        ADD_ATTR: ["target", "rel"],
        ADD_TAGS: ["iframe"],
      });
    }
    return raw;
  }

  function enhance(container) {
    if (!container) return;
    container.querySelectorAll("pre code").forEach((block) => {
      const pre = block.parentElement;
      if (pre.querySelector(".code-copy-btn")) return;
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "code-copy-btn";
      btn.textContent = "复制";
      btn.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText(block.textContent);
          btn.textContent = "已复制";
          btn.classList.add("copied");
          setTimeout(() => {
            btn.textContent = "复制";
            btn.classList.remove("copied");
          }, 2000);
        } catch (_) {}
      });
      pre.appendChild(btn);
    });
    container.querySelectorAll("a[href^='/']").forEach((a) => {
      if (a.getAttribute("target") === "_blank") return;
      a.setAttribute("data-nav", "");
    });
  }

  function buildToc(container) {
    const headings = container.querySelectorAll("h2, h3");
    if (headings.length < 2) return "";
    const items = [];
    headings.forEach((h) => {
      const id = h.getAttribute("id");
      if (!id) return;
      const level = h.tagName === "H3" ? "toc-h3" : "";
      const text = h.textContent.replace(/#$/, "").trim();
      items.push(
        `<li><a href="#${escapeHtml(id)}" class="${level}">${escapeHtml(text)}</a></li>`
      );
    });
    if (!items.length) return "";
    return `<nav class="docs-toc" id="docs-toc"><h5>本页目录</h5><ul>${items.join("")}</ul></nav>`;
  }

  function initTocScrollSpy() {
    const toc = document.getElementById("docs-toc");
    if (!toc) return;
    const links = toc.querySelectorAll("a");
    const ids = Array.from(links).map((a) => a.getAttribute("href").slice(1));
    const headings = ids.map((id) => document.getElementById(id)).filter(Boolean);
    if (!headings.length) return;
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            const id = entry.target.id;
            links.forEach((a) => {
              a.classList.toggle("active", a.getAttribute("href") === `#${id}`);
            });
          }
        });
      },
      { rootMargin: "-80px 0px -70% 0px", threshold: 0 }
    );
    headings.forEach((h) => observer.observe(h));
  }

  window.Docbit = window.Docbit || {};
  Docbit.Markdown = { init, render, enhance, buildToc, initTocScrollSpy };
})();
