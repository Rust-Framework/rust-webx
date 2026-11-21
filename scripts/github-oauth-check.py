#!/usr/bin/env python3
"""GitHub OAuth 2.0 authorization-code smoke check.

GitHub is OAuth 2.0, **not OpenID Connect**: this validates the real
authorize → callback → token → `/user` flow. It does not produce an id_token
and must not be wired into `OidcClient`.

Required environment:
  GITHUB_CLIENT_ID
  GITHUB_CLIENT_SECRET

Optional:
  GITHUB_REDIRECT_URI (default http://localhost:8765/auth/github/callback)

The callback must be registered on the OAuth App. A browser opens once for
GitHub login/consent; the script then prints the GitHub identity (no token).
"""
import http.server
import json
import os
import secrets
import threading
import urllib.parse
import urllib.request
import webbrowser

CLIENT_ID = os.environ["GITHUB_CLIENT_ID"]
CLIENT_SECRET = os.environ["GITHUB_CLIENT_SECRET"]
REDIRECT = os.environ.get(
    "GITHUB_REDIRECT_URI", "http://localhost:8765/auth/github/callback"
)
SCOPE = "read:user user:email"
STATE = secrets.token_urlsafe(24)
PORT = urllib.parse.urlparse(REDIRECT).port or 8765
RESULT = {"done": False}


def exchange(code: str) -> str:
    body = urllib.parse.urlencode(
        {
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "code": code,
            "redirect_uri": REDIRECT,
        }
    ).encode()
    request = urllib.request.Request(
        "https://github.com/login/oauth/access_token",
        data=body,
        headers={"Accept": "application/json", "User-Agent": "webx-github-oauth-check"},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        payload = json.load(response)
    if "access_token" not in payload:
        raise RuntimeError(payload.get("error_description", payload))
    return payload["access_token"]


def user(token: str) -> dict:
    request = urllib.request.Request(
        "https://api.github.com/user",
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "User-Agent": "webx-github-oauth-check",
        },
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        payload = json.load(response)
    return {
        "login": payload.get("login"),
        "id": payload.get("id"),
        "name": payload.get("name"),
        "email": payload.get("email"),
    }


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path != urllib.parse.urlparse(REDIRECT).path:
            self.send_response(404)
            self.end_headers()
            return
        query = urllib.parse.parse_qs(parsed.query)
        code = query.get("code", [None])[0]
        returned_state = query.get("state", [None])[0]
        if not code or returned_state != STATE:
            self.send_response(400)
            self.end_headers()
            self.wfile.write(b"missing code or state mismatch")
            return
        try:
            identity = user(exchange(code))
            RESULT.update(done=True, identity=identity)
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"GitHub OAuth complete. You can close this tab.")
        except Exception as error:  # noqa: BLE001 - CLI diagnostic
            RESULT.update(done=True, error=str(error))
            self.send_response(500)
            self.end_headers()
            self.wfile.write(str(error).encode())


def main() -> int:
    server = http.server.HTTPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    query = urllib.parse.urlencode(
        {
            "client_id": CLIENT_ID,
            "redirect_uri": REDIRECT,
            "scope": SCOPE,
            "state": STATE,
        }
    )
    webbrowser.open(f"https://github.com/login/oauth/authorize?{query}")
    try:
        server.timeout = 180
        server.handle_request()
    finally:
        server.server_close()
    if not RESULT.get("done"):
        print("TIMEOUT waiting for GitHub authorization")
        return 2
    if "error" in RESULT:
        print("ERROR " + RESULT["error"])
        return 3
    print("IDENTITY " + json.dumps(RESULT["identity"], ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
