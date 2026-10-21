"""Token storage -- the desktop counterpart of KeychainStore.swift.

Prefers the Secret Service (libsecret / gnome-keyring, KWallet), which is
what a native GNOME app should use. When no secret service is reachable --
a headless session, or a NixOS setup without `services.gnome.gnome-keyring`
enabled -- it falls back to a 0600 file under $XDG_CONFIG_HOME so the app
still works instead of silently failing to stay signed in. The fallback is
reported through `backend_name()` so Settings can tell the user which one
is in use rather than leaving the weaker storage invisible.
"""

from __future__ import annotations

import os
from pathlib import Path

from .config import APP_ID, config_dir

_SCHEMA_NAME = f"{APP_ID}.Token"
_ATTR = {"application": APP_ID, "kind": "session-token"}

_secret = None
_schema = None


def _service():
    """Resolve libsecret lazily; returns None when unavailable."""
    global _secret, _schema
    if _secret is None:
        try:
            import gi

            gi.require_version("Secret", "1")
            from gi.repository import Secret  # type: ignore

            _secret = Secret
            _schema = Secret.Schema.new(
                _SCHEMA_NAME,
                Secret.SchemaFlags.NONE,
                {
                    "application": Secret.SchemaAttributeType.STRING,
                    "kind": Secret.SchemaAttributeType.STRING,
                },
            )
        except Exception:
            _secret = False
    return _secret or None


def _fallback_path() -> Path:
    return config_dir() / "token"


def backend_name() -> str:
    return "Secret Service" if _service() else "local file"


def save_token(token: str) -> None:
    service = _service()
    if service:
        try:
            service.password_store_sync(
                _schema, _ATTR, service.COLLECTION_DEFAULT,
                "SwarmPanel session token", token, None,
            )
            return
        except Exception:
            pass
    path = _fallback_path()
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        # Create with 0600 from the outset -- writing then chmod'ing would
        # leave the token world-readable for the moment in between.
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(token)
    except OSError:
        pass


def load_token() -> str | None:
    service = _service()
    if service:
        try:
            found = service.password_lookup_sync(_schema, _ATTR, None)
            if found:
                return found
        except Exception:
            pass
    try:
        value = _fallback_path().read_text("utf-8").strip()
        return value or None
    except OSError:
        return None


def clear_token() -> None:
    service = _service()
    if service:
        try:
            service.password_clear_sync(_schema, _ATTR, None)
        except Exception:
            pass
    # Always clear the fallback too: a token written before the keyring came
    # online must not survive a sign-out.
    try:
        _fallback_path().unlink()
    except OSError:
        pass
