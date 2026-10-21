"""Asynchronous image loading with an on-disk cache.

The panel is image-heavy -- track artwork on every now-playing card, bot
icons on the invite roster, avatars across Directory, Friends, Messages and
profiles -- and all of it is remote. Three things make that safe to do from
a GTK app:

* Fetches happen on a worker thread and the texture is handed back on the
  main thread, so the UI never blocks on the network.
* Results are cached in memory *and* under $XDG_CACHE_HOME, keyed by a hash
  of the URL. A live dashboard re-renders every couple of seconds; without a
  cache that would re-fetch the same artwork every time.
* A failed or non-image response is remembered as a failure, so a broken URL
  is attempted once rather than on every re-render.

Artwork URLs point at third-party hosts (i.ytimg.com for YouTube
thumbnails), so requests deliberately carry no Authorization header -- only
images served by the panel itself get the bearer token.
"""

from __future__ import annotations

import hashlib
import threading
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Callable

import gi

gi.require_version("Gdk", "4.0")
gi.require_version("GdkPixbuf", "2.0")

from gi.repository import Gdk, GdkPixbuf, GLib

from .api import USER_AGENT, api
from .config import cache_dir

MAX_BYTES = 8 * 1024 * 1024
TIMEOUT = 20.0

_memory: dict[str, Any] = {}        # url -> Gdk.Texture
_failed: set[str] = set()           # urls known not to resolve to an image
_in_flight: dict[str, list[Callable[[Any], None]]] = {}
_lock = threading.Lock()


def _cache_path(url: str) -> Path:
    digest = hashlib.sha256(url.encode("utf-8")).hexdigest()
    directory = cache_dir() / "images" / digest[:2]
    directory.mkdir(parents=True, exist_ok=True)
    return directory / digest[2:]


def _texture_from_bytes(data: bytes) -> Any:
    """Decode to a texture, or None when the bytes are not a usable image."""
    try:
        loader = GdkPixbuf.PixbufLoader()
        loader.write(data)
        loader.close()
        pixbuf = loader.get_pixbuf()
    except GLib.Error:
        return None
    if pixbuf is None:
        return None
    return Gdk.Texture.new_for_pixbuf(pixbuf)


def _fetch(url: str) -> bytes | None:
    headers = {"User-Agent": USER_AGENT, "Accept": "image/*"}
    # Only attach credentials to the panel's own host.
    if url.startswith(api.base_url):
        token = api.token
        if token:
            headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            kind = (response.headers.get("Content-Type") or "").lower()
            # Guard against an HTML error page being decoded as an image.
            if kind and not kind.startswith("image/"):
                return None
            return response.read(MAX_BYTES + 1)[:MAX_BYTES]
    except (urllib.error.URLError, OSError, ValueError):
        return None


def load(url: str | None, on_ready: Callable[[Any], None]) -> None:
    """Resolve `url` to a Gdk.Texture and call `on_ready` on the main thread.

    `on_ready` is never called for a URL that cannot be loaded -- callers
    show their placeholder until it fires, so there is nothing to undo.
    Already-cached images call back synchronously, which keeps a re-render
    from flickering through the placeholder.
    """
    if not url or not isinstance(url, str) or not url.startswith(("http://", "https://")):
        return

    with _lock:
        cached = _memory.get(url)
        if cached is not None:
            on_ready(cached)
            return
        if url in _failed:
            return
        waiting = _in_flight.get(url)
        if waiting is not None:
            # Another widget already asked for this; ride along rather than
            # issuing a second request.
            waiting.append(on_ready)
            return
        _in_flight[url] = [on_ready]

    def worker() -> None:
        path = _cache_path(url)
        data: bytes | None = None
        try:
            data = path.read_bytes()
        except OSError:
            data = _fetch(url)
            if data:
                try:
                    # Write via a temporary file so a crashed download cannot
                    # leave a truncated image in the cache forever.
                    tmp = path.with_suffix(".part")
                    tmp.write_bytes(data)
                    tmp.replace(path)
                except OSError:
                    pass
        GLib.idle_add(_deliver, url, data)

    threading.Thread(target=worker, name="swarmpanel-image", daemon=True).start()


def _deliver(url: str, data: bytes | None) -> bool:
    texture = _texture_from_bytes(data) if data else None
    with _lock:
        callbacks = _in_flight.pop(url, [])
        if texture is None:
            _failed.add(url)
        else:
            _memory[url] = texture
    if texture is not None:
        for callback in callbacks:
            try:
                callback(texture)
            except Exception:
                import traceback

                traceback.print_exc()
    return GLib.SOURCE_REMOVE


# -- URL helpers -----------------------------------------------------------


def youtube_thumbnail(video_url: Any) -> str | None:
    """Artwork URL for a YouTube link, or None for other sources.

    Port of ControlStateSession.derivedThumbnailURL. The backend only
    precomputes a `thumbnail` for /api/dashboard's session list, so
    control-state has to derive its own from the same video-id patterns the
    server uses.
    """
    if not isinstance(video_url, str) or not video_url:
        return None
    try:
        parts = urllib.parse.urlparse(video_url)
    except ValueError:
        return None
    host = (parts.hostname or "").lower()
    if "youtube.com" not in host and "youtu.be" not in host:
        return None

    video_id = None
    if "youtu.be" in host:
        video_id = parts.path.strip("/").split("/")[0] or None
    else:
        query = urllib.parse.parse_qs(parts.query)
        if query.get("v"):
            video_id = query["v"][0]
        else:
            segments = [p for p in parts.path.split("/") if p]
            for marker in ("embed", "shorts", "live"):
                if marker in segments:
                    index = segments.index(marker)
                    if len(segments) > index + 1:
                        video_id = segments[index + 1]
                    break
    if not video_id:
        return None
    return f"https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"


def avatar_url(value: Any) -> str | None:
    """Absolute URL for an avatar, which the backend may give relative."""
    if not isinstance(value, str) or not value.strip():
        return None
    value = value.strip()
    if value.startswith(("http://", "https://")):
        return value
    if value.startswith("/"):
        return api.url_for(value)
    return None
