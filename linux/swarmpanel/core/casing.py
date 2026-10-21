"""Key-casing helpers for talking to the SwarmPanel backend.

The backend speaks snake_case. The iOS client leans on JSONEncoder's
`.convertToSnakeCase` / JSONDecoder's `.convertFromSnakeCase`, so its models
can be camelCase.

The two directions are deliberately *not* symmetric here:

* Outgoing bodies are rewritten wholesale (`snakeize`) -- exactly what
  `.convertToSnakeCase` does, and every request body in this app is a
  fixed-shape struct, so there are no data-valued keys to damage.

* Incoming payloads are **not** rewritten. Swift's decoder only converts the
  keys it can map onto a declared property, which leaves free-form payloads
  untouched -- and several endpoints return objects whose keys *are data*:
  `/api/database/data` rows are keyed by real column names, and
  SchemaEntry.tableNames checks `name` / `table_name` / `TABLE_NAME`
  verbatim. Blanket-camelizing those would rename the user's own columns.
  So responses keep their wire keys and are read through `field()`, which
  accepts either spelling.
"""

from __future__ import annotations

import re
from typing import Any, Mapping

_SNAKE_BOUNDARY = re.compile(r"_([a-z0-9])")
_CAMEL_BOUNDARY = re.compile(r"(?<!^)(?=[A-Z])")


def to_camel(name: str) -> str:
    """`bot_key` -> `botKey`. Leading underscores are preserved."""
    stripped = name.lstrip("_")
    prefix = name[: len(name) - len(stripped)]
    return prefix + _SNAKE_BOUNDARY.sub(lambda m: m.group(1).upper(), stripped)


def to_snake(name: str) -> str:
    """`botKey` -> `bot_key`. Already-snake names pass through unchanged."""
    return _CAMEL_BOUNDARY.sub("_", name).lower()


def snakeize(value: Any) -> Any:
    """Recursively snake_case every mapping key, for request bodies."""
    if isinstance(value, Mapping):
        return {to_snake(k): snakeize(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [snakeize(v) for v in value]
    return value


_MISSING = object()


def field(payload: Any, name: str, default: Any = None) -> Any:
    """Read `name` from a decoded response, accepting either key spelling.

    Lets ported screen code use the same camelCase field names as the Swift
    models it came from while the payload keeps its snake_case wire keys.
    Returns `default` for a missing key *and* for an explicit JSON null, so
    callers get one "absent" case to handle rather than two.
    """
    if not isinstance(payload, Mapping):
        return default
    for key in (name, to_snake(name), to_camel(name)):
        found = payload.get(key, _MISSING)
        if found is not _MISSING:
            return default if found is None else found
    return default


def fields(payload: Any, *names: str, default: Any = None) -> Any:
    """First present-and-non-null value among `names`.

    Mirrors the Swift models' fallback chains -- e.g. ControlMatrixBot's
    `key ?? bot_key ?? session.bot_key`, or SchemaEntry's `name` /
    `table_name` / `TABLE_NAME` probing.
    """
    for name in names:
        found = field(payload, name, _MISSING)
        if found is not _MISSING:
            return found
    return default


def unwrap(payload: Any) -> Any:
    """Return the `data` envelope's contents, if there is one.

    Several admin endpoints answer `{"ok": true, "data": {...}}` while others
    put their collections at the top level -- `/api/audit-log`,
    `/api/swarm-accounts/admin`, `/api/image-gallery/admin`,
    `/api/lumisound/admin` and `/api/music-intelligence` are enveloped;
    `/api/dashboard`, `/api/bots` and `/api/metrics` are not. Reading the
    wrong level is silent: the screen renders its empty state instead of an
    error, which is exactly how several admin screens came to look blank.
    """
    if isinstance(payload, Mapping):
        inner = payload.get("data")
        if isinstance(inner, (Mapping, list)):
            return inner
    return payload
