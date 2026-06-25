/* Auth pages — login, register, forgot password, reset password */
(function () {
  "use strict";

  const { escapeHtml, LOGOS } = Docbit.Utils;

  function authShell(title, subtitle, body, footer) {
    return `
      <div class="page page-auth">
        <div class="auth-card">
          <a href="/" class="auth-logo-link" data-nav aria-label="返回首页">
            <img src="${LOGOS.heroAccent}" alt="" class="auth-logo" width="40" height="40" />
          </a>
          <h1>${escapeHtml(title)}</h1>
          ${subtitle ? `<p class="auth-subtitle">${escapeHtml(subtitle)}</p>` : ""}
          ${body}
          ${footer ? `<div class="auth-footer">${footer}</div>` : ""}
        </div>
      </div>`;
  }

  function bindForm(formId, onSubmit) {
    const form = document.getElementById(formId);
    const errEl = document.getElementById("auth-error");
    const okEl = document.getElementById("auth-success");
    form?.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (errEl) errEl.hidden = true;
      if (okEl) okEl.hidden = true;
      const btn = form.querySelector('button[type="submit"]');
      if (btn) btn.disabled = true;
      try {
        await onSubmit(new FormData(form));
      } catch (err) {
        if (errEl) {
          errEl.textContent = err.message;
          errEl.hidden = false;
        }
      } finally {
        if (btn) btn.disabled = false;
      }
    });
  }

  function renderLogin(redirectTo) {
    document.getElementById("app").innerHTML = authShell(
      "登录",
      "欢迎回来，请登录你的账号",
      `<form class="auth-form" id="login-form">
        <div class="form-field">
          <label for="email">邮箱</label>
          <input type="email" id="email" name="email" required autocomplete="email" />
        </div>
        <div class="form-field">
          <label for="password">密码</label>
          <input type="password" id="password" name="password" required autocomplete="current-password" minlength="6" />
        </div>
        <p class="form-error" id="auth-error" hidden></p>
        <button type="submit" class="btn btn-primary auth-submit">登录</button>
      </form>`,
      `<a href="/forgot-password" data-nav>忘记密码？</a>
       <span>·</span>
       <span>还没有账号？<a href="/register" data-nav>注册</a></span>`
    );
    document.title = "登录 — Docbit";
    bindForm("login-form", async (fd) => {
      await Docbit.Auth.login(fd.get("email"), fd.get("password"));
      Docbit.Router.navigate(redirectTo || "/");
    });
  }

  function renderRegister() {
    document.getElementById("app").innerHTML = authShell(
      "注册",
      "创建账号，参与博客讨论",
      `<form class="auth-form" id="register-form">
        <div class="form-field">
          <label for="name">昵称</label>
          <input type="text" id="name" name="name" required autocomplete="name" maxlength="80" />
        </div>
        <div class="form-field">
          <label for="email">邮箱</label>
          <input type="email" id="email" name="email" required autocomplete="email" />
        </div>
        <div class="form-field">
          <label for="password">密码</label>
          <input type="password" id="password" name="password" required autocomplete="new-password" minlength="6" />
        </div>
        <div class="form-field">
          <label for="password2">确认密码</label>
          <input type="password" id="password2" name="password2" required autocomplete="new-password" minlength="6" />
        </div>
        <p class="form-error" id="auth-error" hidden></p>
        <button type="submit" class="btn btn-primary auth-submit">注册</button>
      </form>`,
      `<span>已有账号？<a href="/login" data-nav>登录</a></span>`
    );
    document.title = "注册 — Docbit";
    bindForm("register-form", async (fd) => {
      const p1 = fd.get("password");
      const p2 = fd.get("password2");
      if (p1 !== p2) throw new Error("两次输入的密码不一致");
      await Docbit.Auth.register(fd.get("name"), fd.get("email"), p1);
      Docbit.Router.navigate("/");
    });
  }

  function renderForgot() {
    document.getElementById("app").innerHTML = authShell(
      "找回密码",
      "输入注册邮箱，我们将发送重置指引",
      `<form class="auth-form" id="forgot-form">
        <div class="form-field">
          <label for="email">邮箱</label>
          <input type="email" id="email" name="email" required autocomplete="email" />
        </div>
        <p class="form-error" id="auth-error" hidden></p>
        <p class="form-success" id="auth-success" hidden></p>
        <div id="dev-token-box" class="dev-token-box" hidden></div>
        <button type="submit" class="btn btn-primary auth-submit">发送重置链接</button>
      </form>`,
      `<a href="/login" data-nav>← 返回登录</a>`
    );
    document.title = "找回密码 — Docbit";
    bindForm("forgot-form", async (fd) => {
      const res = await Docbit.Auth.forgotPassword(fd.get("email"));
      const okEl = document.getElementById("auth-success");
      if (okEl) {
        okEl.textContent = res.message;
        okEl.hidden = false;
      }
      if (res.reset_token) {
        const box = document.getElementById("dev-token-box");
        if (box) {
          box.hidden = false;
          box.innerHTML = `
            <p><strong>开发模式</strong>：邮件未配置，请使用下方链接重置密码：</p>
            <a class="btn btn-sm" href="/reset-password?token=${encodeURIComponent(res.reset_token)}" data-nav>前往重置密码</a>`;
        }
      }
    });
  }

  function renderReset(token) {
    if (!token) {
      document.getElementById("app").innerHTML = authShell(
        "重置密码",
        "",
        `<p class="form-error" style="display:block">无效的重置链接，请重新申请。</p>`,
        `<a href="/forgot-password" data-nav>重新申请</a>`
      );
      return;
    }

    document.getElementById("app").innerHTML = authShell(
      "重置密码",
      "请设置新密码",
      `<form class="auth-form" id="reset-form">
        <input type="hidden" name="token" value="${escapeHtml(token)}" />
        <div class="form-field">
          <label for="password">新密码</label>
          <input type="password" id="password" name="password" required autocomplete="new-password" minlength="6" />
        </div>
        <div class="form-field">
          <label for="password2">确认新密码</label>
          <input type="password" id="password2" name="password2" required autocomplete="new-password" minlength="6" />
        </div>
        <p class="form-error" id="auth-error" hidden></p>
        <p class="form-success" id="auth-success" hidden></p>
        <button type="submit" class="btn btn-primary auth-submit">更新密码</button>
      </form>`,
      `<a href="/login" data-nav>← 返回登录</a>`
    );
    document.title = "重置密码 — Docbit";
    bindForm("reset-form", async (fd) => {
      const p1 = fd.get("password");
      const p2 = fd.get("password2");
      if (p1 !== p2) throw new Error("两次输入的密码不一致");
      const res = await Docbit.Auth.resetPassword(fd.get("token"), p1);
      const okEl = document.getElementById("auth-success");
      if (okEl) {
        okEl.textContent = res.message;
        okEl.hidden = false;
      }
      setTimeout(() => Docbit.Router.navigate("/login"), 1500);
    });
  }

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.auth = { renderLogin, renderRegister, renderForgot, renderReset };
})();
