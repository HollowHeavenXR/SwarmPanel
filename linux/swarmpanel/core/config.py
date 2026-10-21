"""Persisted client settings.

The iOS client keeps these in UserDefaults; the desktop equivalent is a
small JSON file under $XDG_CONFIG_HOME. Same defaults and same keys as
APIClient.baseURLDefaultsKey / WidgetShared.lastKnownGuildIdKey so the two
ports stay conceptually aligned.
"""

from __future__ import annotations

import json
import os
import threading
from pathlib import Path
from typing import Any

APP_ID = "studio.xenusanimations.SwarmPanel"

# The production backend is a stable *named* Cloudflare tunnel hostname (not a
# rotating trycloudflare.com address), so it is safe to ship as the default --
# the same reasoning as APIClient.defaultBaseURL. Settings can override it to
# point at a local container.
DEFAULT_BASE_URL = "https://swarmpanel.xenusanimations.studio"


def config_dir() -> Path:
    base = os.environ.get("XDG_CONFIG_HOME") or (Path.home() / ".config")
    return Path(base) / "swarmpanel"


#: System-wide defaults, written by the NixOS module
#: (programs.swarmpanel.baseUrl). Read-only, and always loses to a value the
#: user has set in the app -- it supplies a default, never an override.
SYSTEM_CONFIG = Path("/etc/swarmpanel/settings.json")


def cache_dir() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or (Path.home() / ".cache")
    path = Path(base) / "swarmpanel"
    path.mkdir(parents=True, exist_ok=True)
    return path


class Settings:
    """Tiny write-through JSON store. Safe to touch from any thread."""

    def __init__(self, path: Path | None = None) -> None:
        self._path = path or (config_dir() / "settings.json")
        self._lock = threading.RLock()
        self._data: dict[str, Any] = {}
        self._load()

    def _load(self) -> None:
        self._defaults = _read_json(SYSTEM_CONFIG)
        # A missing file is the normal first-run case; a corrupt one is not
        # worth refusing to start over -- defaults are all recoverable.
        self._data = _read_json(self._path)

    def _flush(self) -> None:
        try:
            self._path.parent.mkdir(parents=True, exist_ok=True)
            tmp = self._path.with_suffix(".json.tmp")
            tmp.write_text(json.dumps(self._data, indent=2, sort_keys=True), "utf-8")
            os.replace(tmp, self._path)
        except OSError:
            pass

    def get(self, key: str, default: Any = None) -> Any:
        """User value, else the system-wide default, else `default`."""
        with self._lock:
            if key in self._data:
                value = self._data[key]
            else:
                value = self._defaults.get(key, default)
        return default if value is None else value

    def set(self, key: str, value: Any) -> None:
        with self._lock:
            if value is None:
                self._data.pop(key, None)
            else:
                self._data[key] = value
            self._flush()

    # -- Named settings the app actually reads -----------------------------

    @property
    def base_url(self) -> str:
        raw = str(self.get("baseURL", DEFAULT_BASE_URL)).strip()
        return (raw or DEFAULT_BASE_URL).rstrip("/")

    @base_url.setter
    def base_url(self, value: str) -> None:
        cleaned = (value or "").strip().rstrip("/")
        self.set("baseURL", cleaned or DEFAULT_BASE_URL)

    @property
    def last_known_guild_id(self) -> str:
        """Mirrors WidgetShared.lastKnownGuildIdKey -- lets control actions
        resolve "your guild's bot" the way FleetControlService does."""
        return str(self.get("lastKnownGuildId", "") or "")

    @last_known_guild_id.setter
    def last_known_guild_id(self, value: str) -> None:
        self.set("lastKnownGuildId", value or None)

    @property
    def color_scheme(self) -> str:
        """`follow` | `light` | `dark` -- the desktop analogue of the panel's
        Appearance theme choice, applied via Adw.StyleManager."""
        value = str(self.get("colorScheme", "follow"))
        return value if value in ("follow", "light", "dark") else "follow"

    @color_scheme.setter
    def color_scheme(self, value: str) -> None:
        self.set("colorScheme", value)

    @property
    def remember_device(self) -> bool:
        return bool(self.get("rememberDevice", True))

    @remember_device.setter
    def remember_device(self, value: bool) -> None:
        self.set("rememberDevice", bool(value))

    @property
    def pinned_bots(self) -> list[str]:
        """Port of PinnedBotsStore."""
        value = self.get("pinnedBots", [])
        return [str(v) for v in value] if isinstance(value, list) else []

    @pinned_bots.setter
    def pinned_bots(self, value: list[str]) -> None:
        self.set("pinnedBots", list(dict.fromkeys(value)))

    @property
    def recent_bots(self) -> list[str]:
        """Port of RecentBotsStore."""
        value = self.get("recentBots", [])
        return [str(v) for v in value] if isinstance(value, list) else []

    @recent_bots.setter
    def recent_bots(self, value: list[str]) -> None:
        self.set("recentBots", list(dict.fromkeys(value))[:12])


def _read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text("utf-8"))
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


settings = Settings()
