/* Auth pages — login, register, forgot password, reset password */
(function () {
  "use strict";

  const { escapeHtml, LOGOS } = Docbit.Utils;

  const AUTH_ASIDE = {
    login: {
      title: "欢迎回来",
      desc: "登录账号，参与博客讨论与管理你的内容",
      pills: ["安全认证", "博客互动", "作品浏览"],
    },
    register: {
      title: "加入社区",
      desc: "创建账号，开启你的技术创作之旅",
      pills: ["免费注册", "即时可用", "社区交流"],
    },
    forgot: {
      title: "找回访问",
      desc: "我们将向注册邮箱发送安全重置指引",
      pills: ["邮件验证", "限时链接", "隐私保护"],
    },
    reset: {
      title: "重置凭证",
      desc: "设置一个强密码，保护你的账号安全",
      pills: ["一次有效", "安全加密", "即时生效"],
    },
  };

  const AUTH_FEATURES = [
    { icon: "layui-icon-read", text: "阅读技术博客与开源文档" },
    { icon: "layui-icon-dialogue", text: "参与讨论，分享你的见解" },
    { icon: "layui-icon-component", text: "探索 Rust 全栈作品与框架" },
  ];

  function brandName() {
    return document.getElementById("brand-name")?.textContent?.trim() || "Start World";
  }

  function authField(opts) {
    const attrs = [
      `type="${opts.type || "text"}"`,
      `id="${opts.id}"`,
      `name="${opts.name}"`,
      `class="layui-input auth-input"`,
      `lay-verify="${opts.verify || "required"}"`,
      opts.autocomplete ? `autocomplete="${opts.autocomplete}"` : "",
      opts.minlength ? `minlength="${opts.minlength}"` : "",
      opts.maxlength ? `maxlength="${opts.maxlength}"` : "",
      opts.placeholder ? `placeholder="${escapeHtml(opts.placeholder)}"` : "",
    ]
      .filter(Boolean)
      .join(" ");

    return `
      <div class="layui-form-item auth-field">
        <label class="auth-field-label" for="${opts.id}">${escapeHtml(opts.label)}</label>
        <div class="layui-input-block">
          <div class="auth-input-wrap">
            <i class="layui-icon ${opts.icon} auth-input-icon" aria-hidden="true"></i>
            <input ${attrs} />
          </div>
        </div>
      </div>`;
  }

  function authShell(variant, title, subtitle, body, footer) {
    const aside = AUTH_ASIDE[variant] || AUTH_ASIDE.login;
    const site = brandName();
    return `
      <div class="page page-auth">
        <div class="auth-layout">
          <aside class="auth-aside" aria-hidden="true">
            <div class="auth-aside-inner">
              <div class="auth-aside-hero">
                <a href="/" class="auth-aside-logo-link" data-nav aria-label="返回首页">
                  <img src="${LOGOS.heroBg}" alt="" class="auth-aside-logo" width="96" height="96" />
                </a>
                <div class="auth-aside-copy">
                  <h2 class="auth-aside-title">${escapeHtml(aside.title)}</h2>
                  <p class="auth-aside-desc">${escapeHtml(aside.desc)}</p>
                </div>
              </div>
              <ul class="auth-aside-pills">
                ${aside.pills
                  .map((p) => `<li><span class="auth-pill">${escapeHtml(p)}</span></li>`)
                  .join("")}
              </ul>
              <ul class="auth-aside-features">
                ${AUTH_FEATURES.map(
                  (f) =>
                    `<li><i class="layui-icon ${f.icon}" aria-hidden="true"></i><span>${escapeHtml(f.text)}</span></li>`
                ).join("")}
              </ul>
            </div>
          </aside>
          <section class="auth-main">
            <div class="auth-panel">
              <header class="auth-head">
                <a href="/" class="auth-mobile-brand" data-nav aria-label="返回首页">
                  <img src="${LOGOS.heroAccent}" alt="" width="40" height="40" />
                  <span>${escapeHtml(site)}</span>
                </a>
                <h1>${escapeHtml(title)}</h1>
                ${subtitle ? `<p class="auth-subtitle">${escapeHtml(subtitle)}</p>` : ""}
              </header>
              ${body}
              ${footer ? `<div class="auth-footer">${footer}</div>` : ""}
            </div>
          </section>
        </div>
      </div>`;
  }

  function bindForm(filter, onSubmit) {
    Docbit.UI.renderForm();
    Docbit.UI.onFormSubmit(filter, async (fields) => {
      const form = document.querySelector("form.auth-form");
      const errEl = document.getElementById("auth-error");
      const okEl = document.getElementById("auth-success");
      if (errEl) errEl.hidden = true;
      if (okEl) okEl.hidden = true;
      const btn = form?.querySelector("button[lay-submit]");
      if (btn) btn.disabled = true;
      try {
        await onSubmit(fields);
      } catch (err) {
        if (errEl) {
          errEl.querySelector(".auth-alert-text").textContent = err.message;
          errEl.hidden = false;
        }
        Docbit.UI.error(err.message);
      } finally {
        if (btn) btn.disabled = false;
      }
    });
  }

  function authAlerts() {
    return `
      <div class="auth-alert auth-alert-error" id="auth-error" hidden role="alert">
        <i class="layui-icon layui-icon-close-fill" aria-hidden="true"></i>
        <span class="auth-alert-text"></span>
      </div>
      <div class="auth-alert auth-alert-success" id="auth-success" hidden role="status">
        <i class="layui-icon layui-icon-ok-circle" aria-hidden="true"></i>
        <span class="auth-alert-text"></span>
      </div>`;
  }

  function renderLogin(redirectTo) {
    document.getElementById("app").innerHTML = authShell(
      "login",
      "登录",
      "使用邮箱与密码登录你的账号",
      `<form class="layui-form auth-form" id="login-form" lay-filter="login-form">
        ${authField({
          icon: "layui-icon-email",
          id: "email",
          name: "email",
          type: "email",
          label: "邮箱",
          verify: "required|email",
          autocomplete: "email",
          placeholder: "name@example.com",
        })}
        ${authField({
          icon: "layui-icon-password",
          id: "password",
          name: "password",
          type: "password",
          label: "密码",
          verify: "required",
          autocomplete: "current-password",
          minlength: 6,
          placeholder: "至少 6 位",
        })}
        <div class="auth-field-extra">
          <a href="/forgot-password" class="auth-link" data-nav>忘记密码？</a>
        </div>
        ${authAlerts()}
        <div class="layui-form-item auth-submit-row">
          <button type="submit" class="layui-btn layui-btn-fluid auth-submit-btn" lay-submit lay-filter="login-submit">
            <i class="layui-icon layui-icon-username" aria-hidden="true"></i> 登录
          </button>
        </div>
      </form>`,
      `<span>还没有账号？<a href="/register" class="auth-link" data-nav>立即注册</a></span>`
    );
    document.title = "登录 — Start World";
    bindForm("login-submit", async (fd) => {
      await Docbit.Auth.login(fd.email, fd.password);
      Docbit.UI.success("登录成功");
      Docbit.Router.navigate(redirectTo || "/");
    });
  }

  function renderRegister() {
    document.getElementById("app").innerHTML = authShell(
      "register",
      "注册",
      "填写信息，创建你的账号",
      `<form class="layui-form auth-form" id="register-form" lay-filter="register-form">
        ${authField({
          icon: "layui-icon-username",
          id: "name",
          name: "name",
          type: "text",
          label: "昵称",
          verify: "required",
          autocomplete: "name",
          maxlength: 80,
          placeholder: "你的显示名称",
        })}
        ${authField({
          icon: "layui-icon-email",
          id: "email",
          name: "email",
          type: "email",
          label: "邮箱",
          verify: "required|email",
          autocomplete: "email",
          placeholder: "name@example.com",
        })}
        ${authField({
          icon: "layui-icon-password",
          id: "password",
          name: "password",
          type: "password",
          label: "密码",
          verify: "required",
          autocomplete: "new-password",
          minlength: 6,
          placeholder: "至少 6 位",
        })}
        ${authField({
          icon: "layui-icon-password",
          id: "password2",
          name: "password2",
          type: "password",
          label: "确认密码",
          verify: "required",
          autocomplete: "new-password",
          minlength: 6,
          placeholder: "再次输入密码",
        })}
        ${authAlerts()}
        <div class="layui-form-item auth-submit-row">
          <button type="submit" class="layui-btn layui-btn-fluid auth-submit-btn" lay-submit lay-filter="register-submit">
            <i class="layui-icon layui-icon-add-circle" aria-hidden="true"></i> 创建账号
          </button>
        </div>
      </form>`,
      `<span>已有账号？<a href="/login" class="auth-link" data-nav>返回登录</a></span>`
    );
    document.title = "注册 — Start World";
    bindForm("register-submit", async (fd) => {
      if (fd.password !== fd.password2) throw new Error("两次输入的密码不一致");
      await Docbit.Auth.register(fd.name, fd.email, fd.password);
      Docbit.UI.success("注册成功");
      Docbit.Router.navigate("/");
    });
  }

  function renderForgot() {
    document.getElementById("app").innerHTML = authShell(
      "forgot",
      "找回密码",
      "输入注册邮箱，我们将发送重置指引",
      `<form class="layui-form auth-form" id="forgot-form" lay-filter="forgot-form">
        ${authField({
          icon: "layui-icon-email",
          id: "email",
          name: "email",
          type: "email",
          label: "注册邮箱",
          verify: "required|email",
          autocomplete: "email",
          placeholder: "name@example.com",
        })}
        ${authAlerts()}
        <div id="dev-token-box" class="dev-token-box" hidden></div>
        <div class="layui-form-item auth-submit-row">
          <button type="submit" class="layui-btn layui-btn-fluid auth-submit-btn" lay-submit lay-filter="forgot-submit">
            <i class="layui-icon layui-icon-release" aria-hidden="true"></i> 发送重置链接
          </button>
        </div>
      </form>`,
      `<a href="/login" class="auth-link auth-back-link" data-nav><i class="layui-icon layui-icon-left" aria-hidden="true"></i> 返回登录</a>`
    );
    document.title = "找回密码 — Start World";
    bindForm("forgot-submit", async (fd) => {
      const res = await Docbit.Auth.forgotPassword(fd.email);
      const okEl = document.getElementById("auth-success");
      if (okEl) {
        okEl.querySelector(".auth-alert-text").textContent = res.message;
        okEl.hidden = false;
      }
      Docbit.UI.success(res.message);
      if (res.reset_token) {
        const box = document.getElementById("dev-token-box");
        if (box) {
          box.hidden = false;
          box.innerHTML = `
            <p><strong>开发模式</strong>：邮件未配置，请使用下方链接重置密码</p>
            <a class="layui-btn layui-btn-sm layui-btn-primary dev-token-btn" href="/reset-password?token=${encodeURIComponent(res.reset_token)}" data-nav>
              <i class="layui-icon layui-icon-link" aria-hidden="true"></i> 前往重置密码
            </a>`;
        }
      }
    });
  }

  function renderReset(token) {
    if (!token) {
      document.getElementById("app").innerHTML = authShell(
        "reset",
        "重置密码",
        "",
        `<div class="auth-alert auth-alert-error" style="display:flex" role="alert">
          <i class="layui-icon layui-icon-close-fill" aria-hidden="true"></i>
          <span class="auth-alert-text">无效的重置链接，请重新申请。</span>
        </div>`,
        `<a href="/forgot-password" class="auth-link" data-nav>重新申请</a>`
      );
      return;
    }

    document.getElementById("app").innerHTML = authShell(
      "reset",
      "重置密码",
      "请设置一个安全的新密码",
      `<form class="layui-form auth-form" id="reset-form" lay-filter="reset-form">
        <input type="hidden" name="token" value="${escapeHtml(token)}" />
        ${authField({
          icon: "layui-icon-password",
          id: "password",
          name: "password",
          type: "password",
          label: "新密码",
          verify: "required",
          autocomplete: "new-password",
          minlength: 6,
          placeholder: "至少 6 位",
        })}
        ${authField({
          icon: "layui-icon-password",
          id: "password2",
          name: "password2",
          type: "password",
          label: "确认密码",
          verify: "required",
          autocomplete: "new-password",
          minlength: 6,
          placeholder: "再次输入密码",
        })}
        ${authAlerts()}
        <div class="layui-form-item auth-submit-row">
          <button type="submit" class="layui-btn layui-btn-fluid auth-submit-btn" lay-submit lay-filter="reset-submit">
            <i class="layui-icon layui-icon-ok" aria-hidden="true"></i> 更新密码
          </button>
        </div>
      </form>`,
      `<a href="/login" class="auth-link auth-back-link" data-nav><i class="layui-icon layui-icon-left" aria-hidden="true"></i> 返回登录</a>`
    );
    document.title = "重置密码 — Start World";
    bindForm("reset-submit", async (fd) => {
      if (fd.password !== fd.password2) throw new Error("两次输入的密码不一致");
      const res = await Docbit.Auth.resetPassword(fd.token, fd.password);
      const okEl = document.getElementById("auth-success");
      if (okEl) {
        okEl.querySelector(".auth-alert-text").textContent = res.message;
        okEl.hidden = false;
      }
      Docbit.UI.success(res.message);
      setTimeout(() => Docbit.Router.navigate("/login"), 1500);
    });
  }

  window.Docbit = window.Docbit || {};
  Docbit.Pages = Docbit.Pages || {};
  Docbit.Pages.auth = { renderLogin, renderRegister, renderForgot, renderReset };
})();
