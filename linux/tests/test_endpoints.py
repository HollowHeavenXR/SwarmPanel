"""Every endpoint this client calls must exist on the server.

Guards the class of bug that made several screens silently fail: a path that
looks plausible but is not a route (`/api/users/{id}` instead of
`/api/users/{id}/profile`), or a route reached with the wrong method.

Two subtleties this check handles, because a naive regex match misses both:

* A *literal* client segment must not be matched by a server `:param`
  segment. `POST /api/messages/threads` otherwise "matches"
  `POST /api/messages/:account_id` -- which is exactly how sending a message
  ended up posting to an account literally named "threads".
* An f-string hole (`{account_id}`) is a parameter and may match either a
  `:param` or, if it happens to equal one, a literal.
"""

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import isolate  # noqa: F401

ROOT = Path(__file__).resolve().parents[1]
LUA = ROOT.parent / "lua" / "src"

ROUTE_RE = re.compile(r'httpd\.route\(\s*"(GET|POST|PUT|DELETE)"\s*,\s*"([^"]+)"')
CALL_RE = re.compile(
    r'api\.(get|post|download_raw|request)\(\s*(f?)"(/[^"]*)"')
METHOD = {"get": "GET", "post": "POST", "download_raw": "GET"}


def server_routes():
    routes = set()
    for path in sorted(LUA.glob("*.lua")):
        for method, route in ROUTE_RE.findall(path.read_text()):
            routes.add((method, route))
    return routes


def client_calls():
    calls = []
    for path in sorted((ROOT / "swarmpanel").rglob("*.py")):
        source = path.read_text()
        for match in CALL_RE.finditer(source):
            verb, is_fstring, route = match.groups()
            method = METHOD.get(verb)
            if method is None:
                continue
            line = source[: match.start()].count("\n") + 1
            calls.append((method, route, bool(is_fstring),
                          f"{path.relative_to(ROOT)}:{line}"))
    return calls


def matches(client_path: str, server_path: str) -> bool:
    """Segment-wise match where a literal never satisfies a :param."""
    client_segments = client_path.strip("/").split("/")
    server_segments = server_path.strip("/").split("/")
    if len(client_segments) != len(server_segments):
        return False
    for mine, theirs in zip(client_segments, server_segments):
        if theirs.startswith(":"):
            # Only a substituted value can fill a parameter slot.
            if not (mine.startswith("{") and mine.endswith("}")):
                return False
        elif mine != theirs:
            return False
    return True


routes = server_routes()
calls = client_calls()
print(f"{len(routes)} server routes, {len(calls)} client call sites\n")

problems = []
for method, route, _is_fstring, where in calls:
    if any(m == method and matches(route, r) for m, r in routes):
        continue
    other = sorted({m for m, r in routes if matches(route, r)})
    same_prefix = sorted({f"{m} {r}" for m, r in routes
                          if r.split("/")[:3] == route.split("/")[:3]})[:5]
    problems.append((method, route, where, other, same_prefix))

for method, route, where, other, near in problems:
    print(f"  BAD {method} {route}\n      at {where}")
    if other:
        print(f"      exists but only as: {other}")
    elif near:
        print(f"      nearby routes: {near}")
    print()

if problems:
    print(f"FAIL: {len(problems)} call(s) do not match any server route")
    sys.exit(1)
print("ALL CLIENT ENDPOINTS MATCH A SERVER ROUTE")
