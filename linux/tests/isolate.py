"""Import this FIRST in every test, before any swarmpanel import.

Tests must never touch the developer's real settings or keyring. Two
separate leaks are possible and both are closed here:

* `$XDG_CONFIG_HOME` is redirected, which covers settings.json and the
  file-based token fallback.
* The Secret Service is *not* under XDG control -- a token stored there
  lands in the real login keyring regardless of XDG -- so secret storage is
  stubbed out in-process instead.
"""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

_sandbox = tempfile.mkdtemp(prefix="swarmpanel-test-")
os.environ["XDG_CONFIG_HOME"] = _sandbox
os.environ["XDG_CACHE_HOME"] = _sandbox

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from swarmpanel.core import secrets as _secrets  # noqa: E402

_store: dict[str, str] = {}


def _save(token: str) -> None:
    _store["token"] = token


def _load() -> str | None:
    return _store.get("token")


def _clear() -> None:
    _store.pop("token", None)


_secrets.save_token = _save
_secrets.load_token = _load
_secrets.clear_token = _clear
_secrets.backend_name = lambda: "test stub"

SANDBOX = _sandbox
