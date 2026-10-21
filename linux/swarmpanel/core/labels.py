"""Remembered display names for guilds and channels.

The two sources of dashboard data do not carry the same fields:

* `GET /api/dashboard` includes `guild_name`, `channel_name` and
  `home_channel_name`.
* The `/ws` dashboard snapshot -- which replaces that payload every couple of
  seconds -- carries only `guild_id`, `channel_id` and `home_channel_id`.

So a card rendered from the socket feed has no names to show, which is why
the dashboard settled on raw snowflake IDs a moment after loading. Rather
than re-fetching over REST just to resolve a name, every payload that *does*
carry names contributes them here, and lookups fall back to this cache.

IDs are stable and names change rarely, so a process-lifetime cache is the
right trade; a stale name for a renamed channel is corrected by the next
REST read or inventory fetch.
"""

from __future__ import annotations

import threading
from typing import Any

from .casing import field

_lock = threading.Lock()
_guilds: dict[str, str] = {}
_channels: dict[str, str] = {}


def _put(store: dict[str, str], key: Any, value: Any) -> None:
    if key is None or value is None:
        return
    key_text, value_text = str(key).strip(), str(value).strip()
    if key_text and value_text:
        store[key_text] = value_text


def remember_session(session: Any) -> None:
    """Harvest any names a session payload happens to carry."""
    if not session:
        return
    with _lock:
        _put(_guilds, field(session, "guildId"), field(session, "guildName"))
        _put(_channels, field(session, "channelId"), field(session, "channelName"))
        _put(_channels, field(session, "homeChannelId"),
             field(session, "homeChannelName"))


def remember_inventory(guilds: Any) -> None:
    """Harvest names from `GET /api/bots/{key}/inventory`.

    This is the richest source -- every guild the bot can see, with all of
    their channels -- so visiting Controls once populates names the
    dashboard can then use for guilds that have never appeared in a REST
    dashboard payload.
    """
    with _lock:
        for guild in (guilds or []):
            _put(_guilds, field(guild, "id"), field(guild, "name"))
            for channel in (field(guild, "channels", []) or []):
                _put(_channels, field(channel, "id"), field(channel, "name"))


def guild_label(session: Any, default: str | None = None) -> str | None:
    name = field(session, "guildName")
    if name:
        return str(name)
    guild_id = field(session, "guildId")
    if guild_id is None:
        return default
    with _lock:
        known = _guilds.get(str(guild_id))
    return known or (str(guild_id) if default is None else default)


def channel_label(session: Any, default: str | None = None) -> str | None:
    for name_key in ("channelName", "homeChannelName"):
        name = field(session, name_key)
        if name:
            return str(name)
    for id_key in ("channelId", "homeChannelId"):
        channel_id = field(session, id_key)
        if channel_id is None:
            continue
        with _lock:
            known = _channels.get(str(channel_id))
        if known:
            return known
        return str(channel_id) if default is None else default
    return default


def stats() -> tuple[int, int]:
    with _lock:
        return len(_guilds), len(_channels)
