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

  /**
   * POST `multipart/form-data`.
   *
   * Always sends a `{ file }` part (the upload endpoints expect that field
   * name). `onProgress(percent)` is called while the browser reports progress.
   * Returns the parsed JSON body.
   */
  function upload(path, file, auth, onProgress, extraFields) {
    const form = new FormData();
    form.append("file", file, file.name);
    if (extraFields) {
      Object.keys(extraFields).forEach(function (k) {
        form.append(k, extraFields[k]);
      });
    }

    return new Promise(function (resolve, reject) {
      const xhr = new XMLHttpRequest();
      xhr.open("POST", API + path);
      xhr.setRequestHeader("Accept", "application/json");
      if (auth && Docbit.Auth?.getAuthHeader) {
        const h = Docbit.Auth.getAuthHeader();
        if (h) xhr.setRequestHeader("Authorization", h);
      }
      if (onProgress && xhr.upload) {
        xhr.upload.onprogress = function (e) {
          if (e.lengthComputable) onProgress(Math.round((e.loaded / e.total) * 100));
        };
      }
      xhr.onload = function () {
        let data = null;
        try {
          data = JSON.parse(xhr.responseText);
        } catch (_) {}
        if (xhr.status >= 200 && xhr.status < 300) {
          resolve(data);
        } else {
          const detail =
            data && (data.detail || data.error || data.title || data.message);
          reject(new Error(detail || xhr.statusText || "Upload failed"));
        }
      };
      xhr.onerror = function () {
        reject(new Error("Network error during upload"));
      };
      xhr.send(form);
    });
  }

  function docContentUrl(docsSlug, docPath) {
    // Encode nested paths as a single segment with `:` so both:
    // - legacy `/content/{path}` and
    // - new `/content/{*path}` (colon still decoded to `/`)
    // match correctly. Slash URLs 404 on servers that only have `{path}`.
    const apiPath = String(docPath || "")
      .replace(/\\/g, "/")
      .replace(/^\/+/, "")
      .replace(/\//g, ":");
    return `/api/docs/${encodeURIComponent(docsSlug)}/content/${apiPath}`;
  }

  /** Encode a docs path for use in SPA URLs: `/works/{slug}/docs/{path...}` */
  function encodeDocPath(docPath) {
    return String(docPath || "")
      .replace(/\\/g, "/")
      .split("/")
      .filter(Boolean)
      .map(encodeURIComponent)
      .join("/");
  }

  window.Docbit = window.Docbit || {};
  Docbit.Api = { get, post, put, del, upload, docContentUrl, encodeDocPath };
})();
