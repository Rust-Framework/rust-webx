/* Docbit — HTTP client */
(function () {
  "use strict";

  const API = "";

  async function parseError(res) {
    let msg = res.statusText;
    try {
      const data = await res.json();
      msg = data.detail || data.error || data.title || data.message || msg;
    } catch (_) {}
    return msg || "Request failed";
  }

  async function request(method, path, body, auth) {
    const headers = { Accept: "application/json" };
    if (body != null) headers["Content-Type"] = "application/json";
    if (auth && Docbit.Auth?.getAuthHeader) {
      const h = Docbit.Auth.getAuthHeader();
      if (h) headers.Authorization = h;
    }
    const res = await fetch(API + path, {
      method,
      headers,
      body: body != null ? JSON.stringify(body) : undefined,
    });
    if (!res.ok) throw new Error(await parseError(res));
    if (res.status === 204) return null;
    const text = await res.text();
    if (!text) return null;
    try {
      return JSON.parse(text);
    } catch (e) {
      const looksHtml = /^\s*</.test(text);
      const preview = text.trim().slice(0, 80).replace(/\s+/g, " ");
      throw new Error(
        looksHtml
          ? `Expected JSON from ${path} but got HTML (${res.status}). Check API routing.`
          : `Invalid JSON from ${path}: ${e.message}. Preview: ${preview}`
      );
    }
  }

  function get(path, auth) {
    return request("GET", path, null, auth);
  }

  function post(path, body, auth) {
    return request("POST", path, body, auth);
  }

  function put(path, body, auth) {
    return request("PUT", path, body, auth);
  }

  function del(path, auth) {
    return request("DELETE", path, null, auth);
  }

  function docContentUrl(docsSlug, docPath) {
    const apiPath = docPath.replace(/\//g, ":");
    return `/api/docs/${encodeURIComponent(docsSlug)}/content/${apiPath}`;
  }

  window.Docbit = window.Docbit || {};
  Docbit.Api = { get, post, put, del, docContentUrl };
})();
