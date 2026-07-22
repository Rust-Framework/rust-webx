(() => {
  if (!DmbitApi.getToken()) {
    location.replace("/admin/login.html");
    return;
  }

  const state = {
    products: [],
    selectedProductId: null,
    search: "",
    statusFilter: "",
    dashboard: null,
  };

  const el = {
    userChip: document.getElementById("user-chip"),
    productList: document.getElementById("product-list"),
    goodsBody: document.getElementById("goods-body"),
    goodsTitle: document.getElementById("goods-title"),
    goodsSub: document.getElementById("goods-sub"),
    btnAddGoods: document.getElementById("btn-add-goods"),
    pageTitle: document.getElementById("page-title"),
    pageSub: document.getElementById("page-sub"),
    productDialog: document.getElementById("product-dialog"),
    goodsDialog: document.getElementById("goods-dialog"),
    detailDialog: document.getElementById("detail-dialog"),
    productForm: document.getElementById("product-form"),
    goodsForm: document.getElementById("goods-form"),
    overviewGrid: document.getElementById("overview-grid"),
  };

  const fmt = (n) => Number(n || 0).toLocaleString("zh-CN");

  function escapeHtml(s) {
    return String(s ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;");
  }

  function statusClass(status) {
    if (status === "运行中") return "running";
    if (status === "联调中") return "commissioning";
    if (status === "已交付") return "delivered";
    return "pending";
  }

  function selectedProduct() {
    return state.products.find((p) => p.id === state.selectedProductId) || null;
  }

  function matchGoods(g) {
    if (state.statusFilter && g.status !== state.statusFilter) return false;
    const q = state.search.trim().toLowerCase();
    if (!q) return true;
    const hay = [g.brand, g.asset_code, g.location, g.parameters, g.status]
      .join(" ")
      .toLowerCase();
    return hay.includes(q);
  }

  function renderUser() {
    const user = DmbitApi.getUser();
    el.userChip.textContent = user ? `${user.name || user.email}` : "Admin";
  }

  function renderOverview() {
    const d = state.dashboard;
    if (!d) {
      el.overviewGrid.innerHTML = `<div class="empty-cell">加载中…</div>`;
      return;
    }
    const s = d.stats;
    const devices = (d.products || []).flatMap((p) =>
      (p.goods || []).map((g) => ({ ...g, typeName: p.name }))
    );
    el.overviewGrid.innerHTML = `
      <div class="ov-card"><span>设备总量</span><strong>${fmt(s.total_quantity)}</strong></div>
      <div class="ov-card"><span>运行中</span><strong>${fmt(s.running_quantity)}</strong></div>
      <div class="ov-card"><span>联调中</span><strong>${fmt(s.commissioning_quantity)}</strong></div>
      <div class="ov-card"><span>待上架</span><strong>${fmt(s.pending_quantity)}</strong></div>
      <div class="ov-wide">
        <h3>设备规格一览</h3>
        <div class="ov-list">
          ${devices.map((g) => `
            <div class="ov-item">
              <b>${escapeHtml(g.brand)}</b>
              <p>${escapeHtml(g.typeName)} · ${escapeHtml(g.status)} · ${fmt(g.quantity)} ${escapeHtml(g.unit)} · ${escapeHtml(g.location || "未分配机位")}</p>
            </div>
          `).join("")}
        </div>
      </div>
    `;
  }

  function renderProducts() {
    if (!state.products.length) {
      el.productList.innerHTML = `<div class="empty-cell">暂无设备类型，请先新增</div>`;
      return;
    }
    el.productList.innerHTML = state.products.map((p) => {
      const qty = (p.goods || []).reduce((s, g) => s + (g.quantity || 0), 0);
      const active = p.id === state.selectedProductId ? "active" : "";
      return `
        <div class="product-item ${active}" data-id="${p.id}">
          <div class="name">${escapeHtml(p.name)}</div>
          <div class="meta">${escapeHtml(p.code)} · ${(p.goods || []).length} 规格 · ${fmt(qty)} 台</div>
          <div class="actions">
            <button class="btn sm" data-edit="${p.id}" type="button">编辑</button>
            <button class="btn sm danger" data-del="${p.id}" type="button">删除</button>
          </div>
        </div>
      `;
    }).join("");
  }

  function renderGoods() {
    const product = selectedProduct();
    if (!product) {
      el.goodsTitle.textContent = "设备规格";
      el.goodsSub.textContent = "请选择左侧设备类型";
      el.btnAddGoods.disabled = true;
      el.goodsBody.innerHTML = `<tr><td colspan="6" class="empty-cell">请选择左侧设备类型</td></tr>`;
      return;
    }

    el.goodsTitle.textContent = `${product.name} · 设备规格`;
    el.goodsSub.textContent = product.remark || product.code;
    el.btnAddGoods.disabled = false;

    const goods = (product.goods || []).filter(matchGoods);
    if (!goods.length) {
      el.goodsBody.innerHTML = `<tr><td colspan="6" class="empty-cell">无匹配设备，可调整筛选或新增</td></tr>`;
      return;
    }

    el.goodsBody.innerHTML = goods.map((g) => `
      <tr>
        <td>
          <strong>${escapeHtml(g.brand)}</strong>
          <div class="muted">${escapeHtml(g.asset_code || "—")}</div>
        </td>
        <td><span class="badge ${statusClass(g.status)}">${escapeHtml(g.status)}</span></td>
        <td>${escapeHtml(g.location || "—")}</td>
        <td>${fmt(g.quantity)} ${escapeHtml(g.unit)}</td>
        <td><pre class="params-preview">${escapeHtml(g.parameters)}</pre></td>
        <td class="row-actions">
          <button class="btn sm" data-view-goods="${g.id}" type="button">详情</button>
          <button class="btn sm" data-edit-goods="${g.id}" type="button">编辑</button>
          <button class="btn sm danger" data-del-goods="${g.id}" type="button">删除</button>
        </td>
      </tr>
    `).join("");
  }

  async function loadAll(keepSelection = true) {
    const [products, dashboard] = await Promise.all([
      DmbitApi.get("/api/products"),
      DmbitApi.get("/api/dashboard"),
    ]);
    state.products = products;
    state.dashboard = dashboard;
    if (!keepSelection || !state.products.some((p) => p.id === state.selectedProductId)) {
      state.selectedProductId = state.products[0]?.id || null;
    }
    renderProducts();
    renderGoods();
    renderOverview();
  }

  function openProductDialog(product) {
    document.getElementById("product-dialog-title").textContent = product ? "编辑设备类型" : "新增设备类型";
    el.productForm.id.value = product?.id || "";
    el.productForm.name.value = product?.name || "";
    el.productForm.code.value = product?.code || "";
    el.productForm.remark.value = product?.remark || "";
    el.productForm.sort_order.value = product?.sort_order ?? 0;
    el.productDialog.showModal();
  }

  function openGoodsDialog(goods) {
    const product = selectedProduct();
    if (!product) return;
    document.getElementById("goods-dialog-title").textContent = goods ? "编辑设备" : "新增设备";
    el.goodsForm.id.value = goods?.id || "";
    el.goodsForm.brand.value = goods?.brand || "";
    el.goodsForm.asset_code.value = goods?.asset_code || "";
    el.goodsForm.location.value = goods?.location || "";
    el.goodsForm.status.value = goods?.status || "待上架";
    el.goodsForm.parameters.value = goods?.parameters || "";
    el.goodsForm.unit.value = goods?.unit || "台";
    el.goodsForm.quantity.value = goods?.quantity ?? 0;
    el.goodsForm.sort_order.value = goods?.sort_order ?? 0;
    el.goodsDialog.showModal();
  }

  function openDetail(goods) {
    const product = selectedProduct();
    document.getElementById("detail-title").textContent = goods.brand;
    const params = (goods.parameters || "").split(/\r?\n/).map((s) => s.trim()).filter(Boolean);
    document.getElementById("detail-body").innerHTML = `
      <div class="block"><label>设备类型</label><b>${escapeHtml(product?.name || goods.product_name)}</b></div>
      <div class="block"><label>状态 / 数量</label><b>${escapeHtml(goods.status)} · ${fmt(goods.quantity)} ${escapeHtml(goods.unit)}</b></div>
      <div class="block"><label>机位 / 资产编码</label><b>${escapeHtml(goods.location || "—")} · ${escapeHtml(goods.asset_code || "—")}</b></div>
      <div class="block"><label>完整参数</label><ul>${params.map((p) => `<li>${escapeHtml(p)}</li>`).join("") || "<li>暂无</li>"}</ul></div>
    `;
    el.detailDialog.showModal();
  }

  function showPage(page) {
    document.querySelectorAll(".nav-item").forEach((b) => b.classList.toggle("active", b.dataset.page === page));
    document.getElementById("page-inventory").hidden = page !== "inventory";
    document.getElementById("page-overview").hidden = page !== "overview";
    document.getElementById("page-password").hidden = page !== "password";
    if (page === "inventory") {
      el.pageTitle.textContent = "设备台账";
      el.pageSub.textContent = "设备类型（主表）+ 设备规格（从表）完整参数管理";
    } else if (page === "overview") {
      el.pageTitle.textContent = "运行概览";
      el.pageSub.textContent = "与监控大屏同源的状态统计";
      renderOverview();
    } else {
      el.pageTitle.textContent = "修改密码";
      el.pageSub.textContent = "更新当前管理员账号密码";
    }
  }

  document.querySelectorAll(".nav-item").forEach((btn) => {
    btn.addEventListener("click", () => showPage(btn.dataset.page));
  });

  document.getElementById("logout-btn").addEventListener("click", () => {
    DmbitApi.clearSession();
    location.replace("/admin/login.html");
  });

  document.getElementById("search-input").addEventListener("input", (e) => {
    state.search = e.target.value;
    renderGoods();
  });
  document.getElementById("status-filter").addEventListener("change", (e) => {
    state.statusFilter = e.target.value;
    renderGoods();
  });

  document.getElementById("btn-add-product").addEventListener("click", () => openProductDialog(null));
  document.getElementById("btn-add-goods").addEventListener("click", () => openGoodsDialog(null));

  el.productList.addEventListener("click", async (e) => {
    const editId = e.target.getAttribute("data-edit");
    const delId = e.target.getAttribute("data-del");
    const item = e.target.closest(".product-item");
    if (editId) {
      openProductDialog(state.products.find((p) => p.id === editId));
      return;
    }
    if (delId) {
      if (!confirm("确认删除该设备类型及其下全部设备？")) return;
      await DmbitApi.del(`/api/products/${delId}`);
      await loadAll(false);
      return;
    }
    if (item) {
      state.selectedProductId = item.dataset.id;
      renderProducts();
      renderGoods();
    }
  });

  el.goodsBody.addEventListener("click", async (e) => {
    const product = selectedProduct();
    if (!product) return;
    const viewId = e.target.getAttribute("data-view-goods");
    const editId = e.target.getAttribute("data-edit-goods");
    const delId = e.target.getAttribute("data-del-goods");
    if (viewId) {
      openDetail((product.goods || []).find((g) => g.id === viewId));
      return;
    }
    if (editId) {
      openGoodsDialog((product.goods || []).find((g) => g.id === editId));
      return;
    }
    if (delId) {
      if (!confirm("确认删除该设备规格？")) return;
      await DmbitApi.del(`/api/goods/${delId}`);
      await loadAll(true);
    }
  });

  el.productForm.addEventListener("submit", async (e) => {
    if (e.submitter?.value === "cancel") return;
    e.preventDefault();
    const payload = {
      name: el.productForm.name.value.trim(),
      code: el.productForm.code.value.trim(),
      remark: el.productForm.remark.value.trim(),
      sort_order: Number(el.productForm.sort_order.value || 0),
    };
    const id = el.productForm.id.value;
    if (id) await DmbitApi.put(`/api/products/${id}`, payload);
    else await DmbitApi.post("/api/products", payload);
    el.productDialog.close();
    await loadAll(true);
  });

  el.goodsForm.addEventListener("submit", async (e) => {
    if (e.submitter?.value === "cancel") return;
    e.preventDefault();
    const product = selectedProduct();
    if (!product) return;
    const payload = {
      product_id: product.id,
      brand: el.goodsForm.brand.value.trim(),
      asset_code: el.goodsForm.asset_code.value.trim(),
      location: el.goodsForm.location.value.trim(),
      status: el.goodsForm.status.value,
      parameters: el.goodsForm.parameters.value,
      unit: el.goodsForm.unit.value.trim(),
      quantity: Number(el.goodsForm.quantity.value || 0),
      sort_order: Number(el.goodsForm.sort_order.value || 0),
    };
    const id = el.goodsForm.id.value;
    if (id) await DmbitApi.put(`/api/goods/${id}`, payload);
    else await DmbitApi.post("/api/goods", payload);
    el.goodsDialog.close();
    await loadAll(true);
  });

  document.getElementById("password-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const msg = document.getElementById("password-msg");
    msg.hidden = true;
    const fd = new FormData(e.target);
    if (fd.get("new_password") !== fd.get("confirm_password")) {
      msg.textContent = "两次输入的新密码不一致";
      msg.className = "form-msg err";
      msg.hidden = false;
      return;
    }
    try {
      const res = await DmbitApi.post("/api/auth/change-password", {
        old_password: fd.get("old_password"),
        new_password: fd.get("new_password"),
      });
      msg.textContent = res.message || "密码已更新";
      msg.className = "form-msg ok";
      msg.hidden = false;
      e.target.reset();
    } catch (ex) {
      msg.textContent = ex.message || "修改失败";
      msg.className = "form-msg err";
      msg.hidden = false;
    }
  });

  renderUser();
  loadAll().catch((err) => {
    if (err.status === 401 || err.status === 403) {
      DmbitApi.clearSession();
      location.replace("/admin/login.html");
      return;
    }
    el.productList.innerHTML = `<div class="empty-cell">加载失败：${escapeHtml(err.message)}</div>`;
  });
})();
