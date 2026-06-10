/* =========================================================================
   LRWF Demo — SPA Client-side Router
   ========================================================================= */

(function () {
  'use strict';

  // ── State ──

  const routes = {
    '/':         { title: 'Home',            render: renderHome },
    '/users':    { title: 'Users',           render: renderUsers },
    '/products': { title: 'Products',        render: renderProducts },
    '/info':     { title: 'API Info',        render: renderInfo },
  };

  // ── Bootstrap ──

  document.addEventListener('DOMContentLoaded', () => {
    // Intercept all internal link clicks
    document.addEventListener('click', (e) => {
      const link = e.target.closest('a[href]');
      if (!link) return;
      const href = link.getAttribute('href');
      if (!href.startsWith('/') || href.startsWith('//')) return;
      e.preventDefault();
      navigate(href);
    });

    // Handle browser back/forward
    window.addEventListener('popstate', () => {
      renderRoute(window.location.pathname);
    });

    // Render the current route
    renderRoute(window.location.pathname);
  });

  // ── Navigation ──

  function navigate(path) {
    window.history.pushState(null, '', path);
    renderRoute(path);
  }

  function renderRoute(path) {
    const route = routes[path] || routes['/'];
    document.title = route.title + ' — LRWF Demo';
    document.getElementById('app').innerHTML =
      '<div class="page"><div class="spinner"></div></div>';
    route.render();
  }

  // ── Home page ──

  function renderHome() {
    const app = document.getElementById('app');
    app.innerHTML = `
      <div class="page">
        <h2>Welcome</h2>
        <p>This is a modern SPA powered by <strong>LRWF</strong> —
        a Rust WebApi framework inspired by ASP.NET Core.</p>
        <div class="links">
          <a class="link" href="/users">
            <span>View Users</span>
            <span class="icon">→</span>
          </a>
          <a class="link" href="/products">
            <span>View Products</span>
            <span class="icon">→</span>
          </a>
          <a class="link" href="/info">
            <span>API Info</span>
            <span class="icon">→</span>
          </a>
        </div>
      </div>
    `;
  }

  // ── Users page ──

  async function renderUsers() {
    try {
      const res = await fetch('/api/users');
      if (!res.ok) throw new Error('Failed to fetch users');
      const users = await res.json();
      const app = document.getElementById('app');
      if (!Array.isArray(users) || users.length === 0) {
        app.innerHTML = '<div class="page"><h2>Users</h2><p>No users found.</p></div>';
        return;
      }
      app.innerHTML = `
        <div class="page">
          <h2>Users (${users.length})</h2>
          <table>
            <thead><tr><th>ID</th><th>Name</th><th>Email</th><th>Created</th></tr></thead>
            <tbody>
              ${users.map(u => `
                <tr>
                  <td><code>${escapeHtml(u.id)}</code></td>
                  <td>${escapeHtml(u.name)}</td>
                  <td>${escapeHtml(u.email)}</td>
                  <td>${escapeHtml(u.created_at)}</td>
                </tr>
              `).join('')}
            </tbody>
          </table>
        </div>
      `;
    } catch (err) {
      document.getElementById('app').innerHTML =
        `<div class="page"><h2>Users</h2><p style="color:#f85149">Error: ${escapeHtml(err.message)}</p></div>`;
    }
  }

  // ── Products page ──

  async function renderProducts() {
    try {
      const res = await fetch('/api/products');
      if (!res.ok) throw new Error('Failed to fetch products');
      const products = await res.json();
      const app = document.getElementById('app');
      if (!Array.isArray(products) || products.length === 0) {
        app.innerHTML = '<div class="page"><h2>Products</h2><p>No products found.</p></div>';
        return;
      }
      app.innerHTML = `
        <div class="page">
          <h2>Products (${products.length})</h2>
          <table>
            <thead><tr><th>ID</th><th>Name</th><th>Price</th><th>Created</th></tr></thead>
            <tbody>
              ${products.map(p => `
                <tr>
                  <td><code>${escapeHtml(p.id)}</code></td>
                  <td>${escapeHtml(p.name)}</td>
                  <td>$${Number(p.price).toFixed(2)}</td>
                  <td>${escapeHtml(p.created_at)}</td>
                </tr>
              `).join('')}
            </tbody>
          </table>
        </div>
      `;
    } catch (err) {
      document.getElementById('app').innerHTML =
        `<div class="page"><h2>Products</h2><p style="color:#f85149">Error: ${escapeHtml(err.message)}</p></div>`;
    }
  }

  // ── API Info page ──

  async function renderInfo() {
    try {
      const res = await fetch('/api/info');
      if (!res.ok) throw new Error('Failed to fetch info');
      const info = await res.json();
      const app = document.getElementById('app');
      app.innerHTML = `
        <div class="page">
          <h2>API Info</h2>
          <pre>${escapeHtml(JSON.stringify(info, null, 2))}</pre>
        </div>
      `;
    } catch (err) {
      document.getElementById('app').innerHTML =
        `<div class="page"><h2>API Info</h2><p style="color:#f85149">Error: ${escapeHtml(err.message)}</p></div>`;
    }
  }

  // ── Helpers ──

  function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }
})();
