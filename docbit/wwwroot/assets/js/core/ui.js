/* Docbit — Layui UI bridge (global layui from index.html) */
(function () {
  "use strict";

  let ready = null;

  function initModules(resolve, reject) {
    if (!window.layui) {
      reject(new Error("Layui 未加载"));
      return;
    }
    window.layui.config({ dir: "/assets/vendor/layui/" });
    window.layui.use(["layer", "form", "element", "util", "dropdown", "table"], () => {
      resolve(window.layui);
    });
  }

  function loadLayui() {
    if (ready) return ready;
    ready = new Promise((resolve, reject) => {
      if (window.layui) {
        initModules(resolve, reject);
        return;
      }
      reject(new Error("Layui 未加载，请在 index.html 引入 layui.js"));
    });
    return ready;
  }

  function msg(text, opts) {
    return loadLayui().then((layui) => layui.layer.msg(text, Object.assign({ time: 2200 }, opts || {})));
  }

  function success(text) {
    return msg(text, { icon: 1 });
  }

  function error(text) {
    return msg(text, { icon: 2 });
  }

  function confirm(text, opts) {
    return loadLayui().then(
      (layui) =>
        new Promise((resolve) => {
          const options = Object.assign({ title: "确认", btn: ["确定", "取消"] }, opts || {});
          layui.layer.confirm(
            text,
            options,
            (index) => {
              layui.layer.close(index);
              resolve(true);
            },
            () => resolve(false)
          );
        })
    );
  }

  function open(options) {
    return loadLayui().then((layui) => layui.layer.open(options));
  }

  function renderForm(type, filter) {
    return loadLayui().then((layui) => layui.form.render(type || null, filter || null));
  }

  function onFormSubmit(filter, handler) {
    return loadLayui().then((layui) => {
      layui.form.on("submit(" + filter + ")", (data) => {
        handler(data.field, data);
        return false;
      });
    });
  }

  function renderTable(options) {
    return loadLayui().then((layui) => layui.table.render(options));
  }

  function tableApi() {
    return loadLayui().then((layui) => layui.table);
  }

  window.Docbit = window.Docbit || {};
  Docbit.UI = {
    loadLayui,
    msg,
    success,
    error,
    confirm,
    open,
    renderForm,
    onFormSubmit,
    renderTable,
    tableApi,
  };

  loadLayui().catch(() => {});
})();
