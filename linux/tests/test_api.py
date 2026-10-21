"""Contract tests for core.api against a stand-in backend."""

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import isolate  # noqa: F401  -- must precede every swarmpanel import

from swarmpanel.core import api as api_module
from swarmpanel.core.api import ApiError
from swarmpanel.core.config import settings

SEEN = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _record(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        SEEN.append({
            "path": self.path,
            "method": self.command,
            "auth": self.headers.get("Authorization"),
            "cookie": self.headers.get("Cookie"),
            "content_type": self.headers.get("Content-Type"),
            "body": body.decode() if body else None,
        })
        return body

    def _send(self, code, payload, set_cookie=False):
        data = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        if set_cookie:
            self.send_header("Set-Cookie", "session=abc123; Path=/")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._record()
        if self.path.startswith("/api/session"):
            # Sets a cookie, like Starlette's SessionMiddleware does.
            self._send(200, {"ok": True, "authenticated": True,
                             "token": "rotated-token", "guild_id": "777",
                             "admin_mode": False}, set_cookie=True)
        elif self.path.startswith("/api/needs-auth"):
            self._send(401, {"detail": "nope"})
        elif self.path.startswith("/api/boom"):
            self._send(500, {"detail": [{"msg": "one"}, {"msg": "two"}]})
        elif self.path.startswith("/api/empty"):
            self.send_response(204); self.send_header("Content-Length", "0"); self.end_headers()
        elif self.path.startswith("/api/raw.csv"):
            data = b"a,b\n1,2\n"
            self.send_response(200); self.send_header("Content-Type", "text/csv")
            self.send_header("Content-Length", str(len(data))); self.end_headers()
            self.wfile.write(data)
        elif self.path.startswith("/api/query"):
            self._send(200, {"echo": self.path})
        else:
            self._send(200, {"ok": True})

    def do_POST(self):
        self._record()
        self._send(200, {"ok": True})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
settings.base_url = f"http://127.0.0.1:{server.server_address[1]}"

client = api_module.ApiClient()
client._token = None

# 1. bearer attachment + snake_cased body
client._token = "tok"
client.post("/api/bots/control", {"botKey": "aria", "guildId": "5",
                                  "action": "PAUSE", "payload": {"videoUrl": "u"}})
sent = SEEN[-1]
assert sent["auth"] == "Bearer tok", sent
assert json.loads(sent["body"]) == {"bot_key": "aria", "guild_id": "5",
                                    "action": "PAUSE",
                                    "payload": {"video_url": "u"}}, sent["body"]
assert sent["content_type"] == "application/json"
print("ok  bearer token + snake_case request body")

# 2. {"detail": [...]} -> joined message
try:
    client.get("/api/boom")
    raise SystemExit("expected ApiError")
except ApiError as e:
    assert e.kind == "server" and e.message == "one; two" and e.status_code == 500, vars(e)
print("ok  validation-list detail flattened")

# 3. 401 clears the token and fires on_unauthorized exactly once
fired = []
client.on_unauthorized = lambda: fired.append(1)
try:
    client.get("/api/needs-auth")
    raise SystemExit("expected ApiError")
except ApiError as e:
    assert e.is_unauthorized, vars(e)
assert fired == [1], fired
assert client.token is None, "401 must clear the stored token"
print("ok  401 -> unauthorized, token cleared, hook fired once")

# 4. cookies are never stored or replayed (the 403 regression guard)
client._token = "tok"
client.get("/api/session")          # response sets a cookie
client.get("/api/session")          # must NOT send it back
assert SEEN[-1]["cookie"] is None, f"cookie was replayed: {SEEN[-1]}"
print("ok  Set-Cookie ignored, never replayed")

# 5. empty body is success, not a decode error
assert client.get("/api/empty") == {}
print("ok  empty 204 body treated as success")

# 6. raw download
assert client.download_raw("/api/raw.csv") == b"a,b\n1,2\n"
print("ok  raw CSV download")

# 7. None query values dropped
client.get("/api/query", {"keep": "1", "drop": None})
assert "keep=1" in SEEN[-1]["path"] and "drop" not in SEEN[-1]["path"], SEEN[-1]["path"]
print("ok  None query params dropped")

# 8. session adoption stores rotated token + guild id
api_module.api = client
settings.set("lastKnownGuildId", None)
api_module._adopt_session(client.get("/api/session"))
assert client.token == "rotated-token", client.token
assert settings.last_known_guild_id == "777", settings.last_known_guild_id
print("ok  rotated token + guild id adopted from session payload")

# 9. unreachable host -> friendly network message
settings.base_url = "http://127.0.0.1:1"
try:
    client.get("/api/anything")
    raise SystemExit("expected ApiError")
except ApiError as e:
    assert e.kind == "network" and "refused" in e.message.lower(), vars(e)
print("ok  connection refused -> readable message")

print("\nALL API CONTRACT TESTS PASSED")
