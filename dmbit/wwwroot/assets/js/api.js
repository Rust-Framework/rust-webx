(() => {
  const TOKEN_KEY = "dmbit-token";
  const USER_KEY = "dmbit-user";

  function authHeaders(extra) {
    const headers = Object.assign({}, extra || {});
    const token = localStorage.getItem(TOKEN_KEY);
    if (token) headers.Authorization = `Bearer ${token}`;
    return headers;
  }

  // RFC 6266 / RFC 5987: prefer the UTF-8 `filename*` form, fall back to `filename`.
  function filenameFromDisposition(disposition) {
    if (!disposition) return null;
    const utf8 = /filename\*=UTF-8''([^;]+)/i.exec(disposition);
    if (utf8) {
      try {
        return decodeURIComponent(utf8[1]);
      } catch {
        /* fall through to the ASCII form */
      }
    }
    const plain = /filename="([^"]*)"/i.exec(disposition);
    return plain ? plain[1] : null;
  }

  function toError(res, data, text) {
    const msg =
      (data && (data.message || data.error || data.title || data.detail)) ||
      text ||
      res.statusText;
    const err = new Error(typeof msg === "string" ? msg : JSON.stringify(msg));
    err.status = res.status;
    err.data = data;
    return err;
  }

  async function request(path, options = {}) {
    // A FormData body must keep the browser-generated multipart boundary, so
    // Content-Type is deliberately left unset in that case.
    const headers = authHeaders(options.headers);
    if (!(options.body instanceof FormData)) {
      headers["Content-Type"] = "application/json";
    }

    const res = await fetch(path, Object.assign({}, options, { headers }));
    const text = await res.text();
    let data = null;
    try {
      data = text ? JSON.parse(text) : null;
    } catch {
      data = text;
    }

    if (!res.ok) throw toError(res, data, text);
    return data;
  }

  // Downloads need the bearer token, so the response is fetched and turned into
  // a blob; the server still decides the media type and the filename.
  async function download(path) {
    const res = await fetch(path, { headers: authHeaders() });
    if (!res.ok) {
      const text = await res.text();
      let data = null;
      try {
        data = text ? JSON.parse(text) : null;
      } catch {
        data = text;
      }
      throw toError(res, data, text);
    }
    return {
      blob: await res.blob(),
      filename: filenameFromDisposition(res.headers.get("Content-Disposition")),
    };
  }

  window.DmbitApi = {
    get: (path) => request(path),
    post: (path, body) => request(path, { method: "POST", body: JSON.stringify(body || {}) }),
    put: (path, body) => request(path, { method: "PUT", body: JSON.stringify(body || {}) }),
    del: (path) => request(path, { method: "DELETE" }),
    // Multipart upload: the caller supplies a FormData built from a real File.
    postForm: (path, formData) => request(path, { method: "POST", body: formData }),
    download,
    filenameFromDisposition,
    tokenKey: TOKEN_KEY,
    userKey: USER_KEY,
    getToken: () => localStorage.getItem(TOKEN_KEY),
    getUser: () => {
      try {
        return JSON.parse(localStorage.getItem(USER_KEY) || "null");
      } catch {
        return null;
      }
    },
    setSession: (token, user) => {
      localStorage.setItem(TOKEN_KEY, token);
      localStorage.setItem(USER_KEY, JSON.stringify(user));
    },
    clearSession: () => {
      localStorage.removeItem(TOKEN_KEY);
      localStorage.removeItem(USER_KEY);
    },
  };
})();
