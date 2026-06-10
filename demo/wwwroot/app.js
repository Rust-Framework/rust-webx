/* =========================================================================
   LRWF Demo — SPA Client-side Router
   ========================================================================= */

(function () {
  "use strict";

  const API_BASE = "";

  // ── Token management ──

  function getToken() {
    return localStorage.getItem("lrwf_token");
  }

  function setToken(token) {
    localStorage.setItem("lrwf_token", token);
  }

  function clearToken() {
    localStorage.removeItem("lrwf_token");
  }

  function isAuthenticated() {
    return !!getToken();
  }

  // ── Authenticated fetch ──

  async function apiFetch(path, options = {}) {
    const headers = { "Content-Type": "application/json", ...options.headers };
    const token = getToken();
    if (token) {
      headers["Authorization"] = "Bearer " + token;
    }
    const res = await fetch(API_BASE + path, { ...options, headers });
    if (!res.ok) {
      const data = await res.json().catch(() => ({ error: res.statusText }));
      throw new Error(data.error || "Request failed");
    }
    return res.json();
  }

  // ── Routes ──

  const routes = {
    "/": { title: "Home", render: renderHome },
    "/login": { title: "Login", render: renderLogin },
    "/register": { title: "Register", render: renderRegister },
    "/profile": { title: "Profile", render: renderProfile },
    "/users": { title: "Users", render: renderUsers },
    "/products": { title: "Products", render: renderProducts },
    "/info": { title: "API Info", render: renderInfo },
  };

  // ── Bootstrap ──

  document.addEventListener("DOMContentLoaded", () => {
    document.addEventListener("click", (e) => {
      const link = e.target.closest("a[href]");
      if (!link) return;
      const href = link.getAttribute("href");
      if (!href.startsWith("/") || href.startsWith("//")) return;
      // Don't intercept links that open in new tab
      if (link.getAttribute("target") === "_blank") return;
      e.preventDefault();
      navigate(href);
    });

    window.addEventListener("popstate", () => {
      renderRoute(window.location.pathname);
    });

    renderRoute(window.location.pathname);
  });

  // ── Navigation ──

  function navigate(path) {
    window.history.pushState(null, "", path);
    renderRoute(path);
  }

  function renderRoute(path) {
    const route = routes[path] || routes["/"];
    document.title = route.title + " — LRWF Demo";
    document.getElementById("app").innerHTML =
      '<div class="page" style="text-align:center"><div class="spinner"></div></div>';
    route.render();
    updateNav();
  }

  function updateNav() {
    const container = document.getElementById("nav-links");
    if (!container) return;
    const li = document.getElementById("auth-link");
    if (li) li.remove();
    const authItem = document.createElement("a");
    authItem.id = "auth-link";
    authItem.className = "link";
    if (isAuthenticated()) {
      authItem.innerHTML =
        '<span>Profile</span><span class="badge" id="logout-btn" style="cursor:pointer;background:#3a1a1a;color:#f85149">Logout</span>';
      authItem.href = "/profile";
    } else {
      authItem.innerHTML =
        '<span>Login / Register</span><span class="icon">→</span>';
      authItem.href = "/login";
    }
    container.appendChild(authItem);

    document.getElementById("logout-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      e.preventDefault();
      clearToken();
      navigate("/");
    });
  }

  // ── Helpers ──

  function escapeHtml(str) {
    const div = document.createElement("div");
    div.textContent = str;
    return div.innerHTML;
  }

  function showError(msg) {
    document.getElementById("app").innerHTML =
      `<div class="page"><h2>Error</h2><p style="color:#f85149">${escapeHtml(msg)}</p></div>`;
  }

  // ═════════════════════════════════════════════════════════════════════════
  //  Pages
  // ═════════════════════════════════════════════════════════════════════════

  // ── Home ──

  function renderHome() {
    const app = document.getElementById("app");
    const greeting = isAuthenticated()
      ? '<p>You are logged in. <a href="/profile">View your profile</a>.</p>'
      : '<p><a href="/login">Login</a> or <a href="/register">register</a> to get started.</p>';
    app.innerHTML = `
      <div class="page">
        <h2>Welcome</h2>
        <p>This is a modern SPA powered by <strong>LRWF</strong> —
        a Rust WebApi framework inspired by ASP.NET Core.</p>
        ${greeting}
        <div class="links">
          <a class="link" href="/users"><span>Users (admin)</span><span class="badge">Admin</span></a>
          <a class="link" href="/products"><span>Products</span><span class="icon">→</span></a>
          <a class="link" href="/info"><span>API Info</span><span class="icon">→</span></a>
        </div>
      </div>
    `;
  }

  // ── Login ──

  function renderLogin() {
    if (isAuthenticated()) {
      navigate("/profile");
      return;
    }
    const app = document.getElementById("app");
    app.innerHTML = `
      <div class="page">
        <h2>Login</h2>
        <form id="login-form" style="display:flex;flex-direction:column;gap:1rem;max-width:360px">
          <input type="email" id="login-email" placeholder="Email" required
                 style="padding:0.75rem;border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#c9d1d9">
          <input type="password" id="login-password" placeholder="Password" required
                 style="padding:0.75rem;border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#c9d1d9">
          <button type="submit"
                  style="padding:0.75rem;border-radius:8px;border:none;background:#238636;color:#fff;font-weight:600;cursor:pointer">
            Sign In
          </button>
          <p style="color:#8b949e;font-size:0.85rem">
            No account? <a href="/register">Register</a>
          </p>
          <p style="color:#8b949e;font-size:0.8rem;border-top:1px solid #30363d;padding-top:1rem">
            Demo admin: admin@lrwf.dev / admin123
          </p>
        </form>
        <div id="login-error" style="color:#f85149;margin-top:0.5rem"></div>
      </div>
    `;

    document
      .getElementById("login-form")
      .addEventListener("submit", async (e) => {
        e.preventDefault();
        const email = document.getElementById("login-email").value;
        const password = document.getElementById("login-password").value;
        try {
          const data = await apiFetch("/api/auth/login", {
            method: "POST",
            body: JSON.stringify({ email, password }),
          });
          setToken(data.token);
          navigate("/profile");
        } catch (err) {
          document.getElementById("login-error").textContent = err.message;
        }
      });
  }

  // ── Register ──

  function renderRegister() {
    const app = document.getElementById("app");
    app.innerHTML = `
      <div class="page">
        <h2>Register</h2>
        <form id="register-form" style="display:flex;flex-direction:column;gap:1rem;max-width:360px">
          <input type="text" id="reg-name" placeholder="Display Name" required
                 style="padding:0.75rem;border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#c9d1d9">
          <input type="email" id="reg-email" placeholder="Email" required
                 style="padding:0.75rem;border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#c9d1d9">
          <input type="password" id="reg-password" placeholder="Password" required minlength="6"
                 style="padding:0.75rem;border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#c9d1d9">
          <button type="submit"
                  style="padding:0.75rem;border-radius:8px;border:none;background:#1f6feb;color:#fff;font-weight:600;cursor:pointer">
            Create Account
          </button>
          <p style="color:#8b949e;font-size:0.85rem">
            Already have an account? <a href="/login">Sign in</a>
          </p>
        </form>
        <div id="register-error" style="color:#f85149;margin-top:0.5rem"></div>
      </div>
    `;

    document
      .getElementById("register-form")
      .addEventListener("submit", async (e) => {
        e.preventDefault();
        const name = document.getElementById("reg-name").value;
        const email = document.getElementById("reg-email").value;
        const password = document.getElementById("reg-password").value;
        try {
          const data = await apiFetch("/api/auth/register", {
            method: "POST",
            body: JSON.stringify({ name, email, password }),
          });
          setToken(data.token);
          navigate("/profile");
        } catch (err) {
          document.getElementById("register-error").textContent = err.message;
        }
      });
  }

  // ── Profile (authenticated) ──

  async function renderProfile() {
    if (!isAuthenticated()) {
      navigate("/login");
      return;
    }
    try {
      const user = await apiFetch("/api/auth/me");
      const app = document.getElementById("app");
      app.innerHTML = `
        <div class="page">
          <h2>Profile</h2>
          <table>
            <tr><th>ID</th><td><code>${escapeHtml(user.id)}</code></td></tr>
            <tr><th>Name</th><td>${escapeHtml(user.name)}</td></tr>
            <tr><th>Email</th><td>${escapeHtml(user.email)}</td></tr>
            <tr><th>Role</th><td><span class="badge">${escapeHtml(user.role)}</span></td></tr>
            <tr><th>Created</th><td>${escapeHtml(user.created_at)}</td></tr>
          </table>
          <div class="links" style="margin-top:1.5rem">
            <a class="link" href="/"><span>Back to Home</span><span class="icon">→</span></a>
          </div>
        </div>
      `;
    } catch (err) {
      clearToken();
      showError("Session expired. Please login again.");
    }
  }

  // ── Users (admin only) ──

  async function renderUsers() {
    try {
      const users = await apiFetch("/api/users");
      const app = document.getElementById("app");
      if (!Array.isArray(users) || users.length === 0) {
        app.innerHTML =
          '<div class="page"><h2>Users</h2><p>No users found.</p></div>';
        return;
      }
      app.innerHTML = `
        <div class="page">
          <h2>Users (${users.length})</h2>
          <table>
            <thead><tr><th>ID</th><th>Name</th><th>Email</th><th>Role</th><th>Created</th></tr></thead>
            <tbody>
              ${users
                .map(
                  (u) => `
                <tr>
                  <td><code>${escapeHtml(u.id)}</code></td>
                  <td>${escapeHtml(u.name)}</td>
                  <td>${escapeHtml(u.email)}</td>
                  <td><span class="badge">${escapeHtml(u.role)}</span></td>
                  <td>${escapeHtml(u.created_at)}</td>
                </tr>
              `,
                )
                .join("")}
            </tbody>
          </table>
        </div>
      `;
    } catch (err) {
      showError(err.message);
    }
  }

  // ── Products ──

  async function renderProducts() {
    try {
      const products = await apiFetch("/api/products");
      const app = document.getElementById("app");
      if (!Array.isArray(products) || products.length === 0) {
        app.innerHTML =
          '<div class="page"><h2>Products</h2><p>No products found.</p></div>';
        return;
      }
      app.innerHTML = `
        <div class="page">
          <h2>Products (${products.length})</h2>
          <table>
            <thead><tr><th>ID</th><th>Name</th><th>Price</th><th>Created</th></tr></thead>
            <tbody>
              ${products
                .map(
                  (p) => `
                <tr>
                  <td><code>${escapeHtml(p.id)}</code></td>
                  <td>${escapeHtml(p.name)}</td>
                  <td>$${Number(p.price).toFixed(2)}</td>
                  <td>${escapeHtml(p.created_at)}</td>
                </tr>
              `,
                )
                .join("")}
            </tbody>
          </table>
        </div>
      `;
    } catch (err) {
      showError(err.message);
    }
  }

  // ── API Info ──

  async function renderInfo() {
    try {
      const info = await apiFetch("/api/info");
      const app = document.getElementById("app");
      app.innerHTML = `
        <div class="page">
          <h2>API Info</h2>
          <pre>${escapeHtml(JSON.stringify(info, null, 2))}</pre>
        </div>
      `;
    } catch (err) {
      showError(err.message);
    }
  }
})();
