/* Docbit — authentication */
(function () {
  "use strict";

  const TOKEN_KEY = "docbit-token";
  const USER_KEY = "docbit-user";

  let currentUser = null;

  function loadStoredUser() {
    try {
      const raw = localStorage.getItem(USER_KEY);
      return raw ? JSON.parse(raw) : null;
    } catch (_) {
      return null;
    }
  }

  function getToken() {
    return localStorage.getItem(TOKEN_KEY);
  }

  function getAuthHeader() {
    const token = getToken();
    return token ? `Bearer ${token}` : null;
  }

  function isLoggedIn() {
    return !!getToken();
  }

  function getUser() {
    return currentUser || loadStoredUser();
  }

  function persistSession(token, user) {
    localStorage.setItem(TOKEN_KEY, token);
    localStorage.setItem(USER_KEY, JSON.stringify(user));
    currentUser = user;
    updateHeaderMenu();
  }

  function logout() {
    localStorage.removeItem(TOKEN_KEY);
    localStorage.removeItem(USER_KEY);
    currentUser = null;
    updateHeaderMenu();
  }

  async function login(email, password) {
    const data = await Docbit.Api.post("/api/auth/login", { email, password });
    persistSession(data.token, data.user);
    return data;
  }

  async function register(name, email, password) {
    const data = await Docbit.Api.post("/api/auth/register", {
      name,
      email,
      password,
    });
    persistSession(data.token, data.user);
    return data;
  }

  async function forgotPassword(email) {
    return Docbit.Api.post("/api/auth/forgot-password", { email });
  }

  async function resetPassword(token, password) {
    return Docbit.Api.post("/api/auth/reset-password", { token, password });
  }

  async function refreshMe() {
    if (!isLoggedIn()) {
      currentUser = null;
      updateHeaderMenu();
      return null;
    }
    try {
      const user = await Docbit.Api.get("/api/auth/me", true);
      currentUser = user;
      localStorage.setItem(USER_KEY, JSON.stringify(user));
      updateHeaderMenu();
      return user;
    } catch (_) {
      logout();
      return null;
    }
  }

  function updateHeaderMenu() {
    const slot = document.getElementById("user-menu");
    if (!slot) return;

    const user = getUser();
    if (user && isLoggedIn()) {
      slot.innerHTML = `
        <div class="user-menu logged-in">
          <button type="button" class="user-menu-trigger" id="user-menu-trigger" aria-expanded="false">
            <span class="user-avatar">${Docbit.Utils.escapeHtml(user.name.charAt(0).toUpperCase())}</span>
            <span class="user-name">${Docbit.Utils.escapeHtml(user.name)}</span>
          </button>
          <div class="user-dropdown" id="user-dropdown" hidden>
            <div class="user-dropdown-meta">${Docbit.Utils.escapeHtml(user.email)}</div>
            <button type="button" class="user-dropdown-item" id="logout-btn">退出登录</button>
          </div>
        </div>`;
      const trigger = document.getElementById("user-menu-trigger");
      const dropdown = document.getElementById("user-dropdown");
      trigger?.addEventListener("click", (e) => {
        e.stopPropagation();
        const open = dropdown.hidden;
        dropdown.hidden = !open;
        trigger.setAttribute("aria-expanded", String(open));
      });
      document.getElementById("logout-btn")?.addEventListener("click", () => {
        logout();
        Docbit.Router?.navigate("/");
      });
      if (!slot._clickBound) {
        slot._clickBound = true;
        document.addEventListener("click", () => {
          const dd = document.getElementById("user-dropdown");
          const tr = document.getElementById("user-menu-trigger");
          if (dd && !dd.hidden) {
            dd.hidden = true;
            tr?.setAttribute("aria-expanded", "false");
          }
        });
      }
    } else {
      slot.innerHTML = `
        <div class="user-menu guest">
          <a href="/login" class="layui-btn layui-btn-sm layui-btn-primary" data-nav>登录</a>
          <a href="/register" class="layui-btn layui-btn-sm layui-btn-normal" data-nav>注册</a>
        </div>`;
    }
  }

  async function init() {
    currentUser = loadStoredUser();
    updateHeaderMenu();
    if (isLoggedIn()) await refreshMe();
  }

  window.Docbit = window.Docbit || {};
  Docbit.Auth = {
    init,
    login,
    register,
    forgotPassword,
    resetPassword,
    logout,
    refreshMe,
    getToken,
    getAuthHeader,
    isLoggedIn,
    getUser,
    updateHeaderMenu,
  };
})();
