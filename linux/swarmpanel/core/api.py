"""REST client -- port of APIClient.swift / APIError.swift.

Attaches a bearer token, snake_cases outgoing JSON bodies, surfaces
`{"detail": ...}` error payloads as ApiError, and reports 401 once through
`on_unauthorized` so the whole app can route to Login at the same moment.

Cookies are deliberately never sent or stored. This client authenticates
purely by bearer token, and a leftover Starlette session cookie riding along
on an otherwise-unauthenticated request (a fresh login POST, which carries no
Authorization header and -- being a native client -- no Origin header either)
trips the backend's "cookie without a trusted browser Origin" guard with a
403. urllib only attaches cookies if a cookie-aware handler is installed, so
the fix here is simply to build an opener without one. Same conclusion
APIClient.makeSession() reached with an ephemeral, cookie-disabled
URLSession.
"""

from __future__ import annotations

import json
import socket
import threading
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Callable, Mapping

from .casing import field, snakeize
from .config import settings
from . import secrets

USER_AGENT = "SwarmPanel-Linux/1.0 (GTK4)"
DEFAULT_TIMEOUT = 30.0


class ApiError(Exception):
    """Mirrors APIError's cases, flattened into one exception type.

    `kind` is one of `network`, `invalid_response`, `decoding`, `server`,
    `unauthorized`; `message` is already the user-facing string, matching
    APIError.errorDescription.
    """

    def __init__(self, kind: str, message: str, status_code: int | None = None) -> None:
        super().__init__(message)
        self.kind = kind
        self.message = message
        self.status_code = status_code

    @property
    def is_unauthorized(self) -> bool:
        return self.kind == "unauthorized"

    def __str__(self) -> str:  # what the error banners show
        return self.message


def _detail_message(body: bytes, status_code: int) -> str:
    """Extract a human line from FastAPI's `{"detail": ...}`.

    `detail` may be a string, a list of Pydantic validation objects, or some
    other object -- the same three shapes APIErrorPayload handles.
    """
    try:
        payload = json.loads(body.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
        return f"Request failed ({status_code})."
    detail = payload.get("detail") if isinstance(payload, dict) else None
    if isinstance(detail, str) and detail.strip():
        return detail
    if isinstance(detail, list):
        lines = [
            str(item.get("msg") or "Invalid request.") if isinstance(item, dict) else str(item)
            for item in detail
        ]
        lines = [line for line in lines if line]
        if lines:
            return "; ".join(lines)
    if isinstance(detail, dict):
        for key in ("message", "msg", "error"):
            value = detail.get(key)
            if isinstance(value, str) and value.strip():
                return value
    return f"Request failed ({status_code})."


class ApiClient:
    """Thread-safe: screens call these from worker threads, never the UI thread."""

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._token: str | None = secrets.load_token()
        self.on_unauthorized: Callable[[], None] | None = None
        # No HTTPCookieProcessor -- see the module docstring.
        self._opener = urllib.request.build_opener(
            urllib.request.HTTPSHandler(),
            urllib.request.HTTPHandler(),
        )

    # -- auth state --------------------------------------------------------

    @property
    def token(self) -> str | None:
        with self._lock:
            return self._token

    @token.setter
    def token(self, value: str | None) -> None:
        with self._lock:
            self._token = value or None
        if value:
            secrets.save_token(value)
        else:
            secrets.clear_token()

    @property
    def base_url(self) -> str:
        return settings.base_url

    def url_for(self, path: str) -> str:
        """Absolute URL for a backend path, for things GTK opens directly
        (avatar images, invite links, Lumisound downloads)."""
        if path.startswith(("http://", "https://")):
            return path
        return f"{self.base_url}/{path.lstrip('/')}"

    # -- requests ----------------------------------------------------------

    def get(self, path: str, query: Mapping[str, Any] | None = None, **kw) -> Any:
        return self.request("GET", path, query=query, **kw)

    def post(self, path: str, body: Any = None, **kw) -> Any:
        return self.request("POST", path, body=body, **kw)

    def request(
        self,
        method: str,
        path: str,
        query: Mapping[str, Any] | None = None,
        body: Any = None,
        timeout: float = DEFAULT_TIMEOUT,
        raw: bool = False,
    ) -> Any:
        url = self.url_for(path)
        if query:
            # Drop None values rather than sending "None" -- matches the
            # Swift compactMap over optional query values.
            pairs = [(k, str(v)) for k, v in query.items() if v is not None]
            if pairs:
                sep = "&" if urllib.parse.urlparse(url).query else "?"
                url = f"{url}{sep}{urllib.parse.urlencode(pairs)}"

        data = None
        headers = {"Accept": "application/json", "User-Agent": USER_AGENT}
        if body is not None:
            data = json.dumps(snakeize(body)).encode("utf-8")
            headers["Content-Type"] = "application/json"
        token = self.token
        if token:
            headers["Authorization"] = f"Bearer {token}"

        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with self._opener.open(request, timeout=timeout) as response:
                payload = response.read()
        except urllib.error.HTTPError as error:
            body_bytes = b""
            try:
                body_bytes = error.read()
            except Exception:
                pass
            if error.code == 401:
                self._fire_unauthorized()
                raise ApiError(
                    "unauthorized",
                    "Your session expired. Please log in again.",
                    401,
                ) from None
            raise ApiError(
                "server", _detail_message(body_bytes, error.code), error.code
            ) from None
        except urllib.error.URLError as error:
            raise ApiError("network", _network_message(error)) from None
        except (TimeoutError, socket.timeout):
            raise ApiError("network", "The request timed out.") from None
        except OSError as error:
            raise ApiError("network", str(error) or "Network error.") from None

        if raw:
            return payload
        if not payload.strip():
            # A 204/empty body on an action endpoint is a success, not a
            # decode failure -- the Swift side models these as OKResponse.
            return {}
        try:
            return json.loads(payload.decode("utf-8"))
        except (ValueError, UnicodeDecodeError):
            raise ApiError("decoding", "Couldn't read the server's response.") from None

    def download_raw(self, path: str, timeout: float = 120.0) -> bytes:
        """Non-JSON download -- CSV exports, mainly."""
        return self.request("GET", path, timeout=timeout, raw=True)

    def _fire_unauthorized(self) -> None:
        self.token = None
        callback = self.on_unauthorized
        if callback:
            callback()


def _network_message(error: urllib.error.URLError) -> str:
    reason = getattr(error, "reason", None)
    if isinstance(reason, (TimeoutError, socket.timeout)):
        return "The request timed out."
    text = str(reason or error) or "Network error."
    # urllib stringifies DNS/connection failures as bare errno tuples; those
    # read badly in a banner, so give the common ones a plain sentence.
    if "Name or service not known" in text or "nodename nor servname" in text:
        return "Can't reach the panel -- check the server address and your connection."
    if "Connection refused" in text:
        return "The panel refused the connection -- is the backend running?"
    if "certificate" in text.lower():
        return f"TLS problem reaching the panel: {text}"
    return text


api = ApiClient()


# -- session helpers (port of AppState's session plumbing) ------------------


def login(username: str, password: str, guild_id: str | None = None) -> dict:
    payload = api.post(
        "/api/session/login",
        {
            "username": username,
            "password": password,
            "guildId": guild_id or None,
            # Long-lived token that every GET /api/session renews, so closing
            # the app overnight doesn't mean signing in again.
            "rememberDevice": settings.remember_device,
            "client": "linux",
        },
    )
    _adopt_session(payload)
    return payload


def register(
    username: str,
    guild_id: str,
    password: str,
    email: str | None = None,
    verification_webhook_url: str | None = None,
    discord_user_id: str | None = None,
) -> dict:
    payload = api.post(
        "/api/session/register",
        {
            "username": username,
            "guildId": guild_id,
            "password": password,
            "email": email or None,
            "verificationWebhookUrl": verification_webhook_url or None,
            "discordUserId": discord_user_id or None,
            "rememberDevice": settings.remember_device,
            "client": "linux",
        },
    )
    _adopt_session(payload)
    return payload


def refresh_session() -> dict:
    """GET /api/session -- also renews a remember-device token."""
    payload = api.get("/api/session")
    _adopt_session(payload)
    return payload


def set_admin_mode(enabled: bool) -> dict:
    return api.post("/api/session/admin-mode", {"enabled": bool(enabled)})


def logout() -> None:
    api.token = None
    settings.last_known_guild_id = ""


def _adopt_session(payload: Any) -> None:
    """Store a rotated token and remember the guild for headless controls."""
    token = field(payload, "token")
    if isinstance(token, str) and token:
        api.token = token
    guild_id = field(payload, "guildId") or field(payload, "accountGuildId")
    if isinstance(guild_id, str) and guild_id:
        settings.last_known_guild_id = guild_id
