"""Stand-in SwarmPanel backend for the UI smoke test.

Returns plausibly-shaped payloads for every endpoint the client touches, so
the smoke test exercises the real render paths rather than empty states.
"""

import json
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# GET /api/session -- synthesises `authenticated`, per routes.lua.
SESSION = {
    "ok": True, "authenticated": True, "mode": "token", "token": "smoke-token",
    "username": "desmondc", "role": "owner", "guild_id": "777",
    "account_guild_id": "777", "site_owner": True, "admin_mode": True,
    "moderator": True, "image_gallery_owner": True, "expires_in": 86400,
    "pages_public_url": "https://swarmpanel.xenusanimations.studio",
}

# POST /api/session/login -- deliberately has NO `authenticated` key. This
# mirrors routes.lua exactly: the login route answers with `ok` + a token and
# leaves `authenticated` to GET /api/session. An earlier version of this fake
# wrongly included it, which hid a real bug where every screen was gated off
# after signing in. Keep these two payloads different.
LOGIN = {
    "ok": True, "token": "smoke-token",
    "username": "desmondc", "role": "owner", "guild_id": "777",
    "account_guild_id": "777", "site_owner": True, "admin_mode": True,
    "moderator": True, "image_gallery_owner": True, "expires_in": 86400,
    "pages_public_url": "https://swarmpanel.xenusanimations.studio",
}

SESSIONS = [{
    "guild_id": "777", "channel_id": "888", "guild_name": "Witch Knot",
    "channel_name": "music", "title": "Night Drive", "is_playing": True,
    "is_paused": False, "session_state": "playing",
    "session_state_label": "Playing", "queue_count": 12,
    "backup_queue_count": 3, "position_seconds": 134, "duration_seconds": 242,
    "video_url": "https://youtu.be/dQw4w9WgXcQ",
    "media_source_label": "YouTube", "cached": True,
    "thumbnail": "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg",
}]

DASHBOARD = {
    "bots": [
        {"key": "aria", "display_name": "Aria", "kind": "assistant",
         "status": "online", "heartbeat_status": "ok",
         "active_playing_count": 1, "known_guild_count": 4,
         "queue_depth": 12, "backup_queue_depth": 3, "sessions": SESSIONS},
        {"key": "node-7", "display_name": "Node 7", "kind": "music",
         "status": "offline", "heartbeat_status": "stale",
         "active_playing_count": 0, "known_guild_count": 2,
         "queue_depth": 0, "backup_queue_depth": 0, "sessions": []},
    ],
    "sessions": SESSIONS,
}

CONTROL_STATE = {"session": {
    "title": "Night Drive", "video_url": "https://youtu.be/dQw4w9WgXcQ",
    "is_playing": True, "is_paused": False, "session_state_label": "Playing",
    "queue_count": 12, "backup_queue_count": 3, "home_channel_id": "888",
    "channel_id": "888", "loop_mode": "queue", "filter_mode": "nightcore",
    "position_seconds": 134, "duration_seconds": 242, "volume": 80,
    "pending_direct_orders": 1,
    "latest_direct_order": {"command": "RECOVER", "data": "{}"},
    "queue_preview": [
        {"video_url": "https://youtu.be/a", "title": "Track A"},
        {"video_url": "https://youtu.be/b", "title": "Track B"},
    ],
}}

ROUTES = {
    "/api/session": SESSION,
    "/api/session/login": LOGIN,
    "/api/session/admin-mode": {"ok": True},
    "/api/dashboard": DASHBOARD,
    "/api/bots": {
        "bots": [{"key": "aria", "display_name": "Aria", "kind": "assistant"},
                 {"key": "node-7", "display_name": "Node 7", "kind": "music"}],
        "invite_bots": [{
            "key": "aria", "display_name": "Aria", "kind": "assistant",
            "capability_summary": "Playback + recommendations",
            "accent": "#8844ff", "token_configured": True,
            "connected_to_session_guild": True,
            "identity_name": "Aria#0001",
            "invite_url": "https://discord.com/oauth2/authorize?client_id=1",
            "permissions": ["Connect", "Speak", "Manage Channels"],
        }],
    },
    "/api/queues": {"ok": True, "queues": [
        {"id": 1, "name": "Late night", "items": [
            {"video_url": "https://youtu.be/a", "title": "Track A"}]}]},
    "/api/swarm-leaderboard": {
        "totals": {"plays": 48210, "listeners": 312},
        "top_tracks": [{"title": "Night Drive", "plays": 412, "listeners": 66}],
        "top_listeners": [{"display_name": "desmondc", "plays": 980, "minutes": 5120}],
    },
    "/api/music-intelligence": {
        "summary": {"tracked_tracks": 8120, "affinities": 240},
        "top_affinities": [{"track": "Night Drive", "score": 0.91}],
        "weights": {"recency": 0.4, "repeat": 0.2},
    },
    "/api/users/directory": {"users": [
        {"account_id": "2", "username": "nova", "display_name": "Nova",
         "bio": "Runs the night shift.", "online": True, "guild_name": "Witch Knot"},
        {"account_id": "3", "username": "quiet", "last_seen": "2026-10-01T10:00:00Z"},
    ]},
    # /api/me/friends is confirmed friends ONLY; requests come from
    # /api/friends/requests. Keep these separate -- conflating them is what
    # made the Friends screen miss pending requests.
    "/api/me/friends": {
        "ok": True,
        "friends": [{"account_id": "2", "username": "nova", "online": True,
                     "avatar_url": ""}],
    },
    # The thread LIST only -- a conversation lives at
    # /api/messages/:account_id, handled in PATTERNS below.
    "/api/messages/threads": {
        "ok": True,
        "threads": [{"account_id": "2", "username": "nova",
                     "last_message": "see you then", "unread": 2,
                     "avatar_url": ""}],
    },
    "/api/users/me": {
        "username": "desmondc", "display_name": "Desmond", "role": "owner",
        "bio": "Panel owner.", "avatar_url": "", "profile_color": "#8844ff",
        "visibility": "public", "email": "owner@example.com",
        "email_verified": True, "guild_id": "777", "guild_name": "Witch Knot",
        "created_at": "2025-01-04T12:00:00Z",
    },
    # Field names exactly as profiles.lua validates them.
    "/api/users/preferences": {"ok": True, "preferences": {
        "theme_mode": "dark", "accent_color": "#8844ff",
        "background_mode": "aurora", "background_color": "#0b0e18",
        "layout_mode": "standard", "density": "comfortable",
        "card_shape": "soft", "font_scale": "normal", "motion": "standard",
        "operator_layout": "command", "roster_layout": "cards",
        "tab_style": "rail", "surface_opacity": 0.92, "surface_blur": 18,
        "stream_card_style": "telemetry", "dashboard_density": "command"}},
    "/api/appearance/presets": {
        "panel": [
            {"id": "neon_arcade", "title": "Neon Arcade",
             "note": "Loud, glowy, and built for showing the panel off.",
             "patch": {"theme_mode": "dark", "accent_color": "#f472b6",
                       "background_mode": "aurora",
                       "stream_card_style": "cinematic", "surface_blur": 28}},
            {"id": "terminal_green", "title": "Terminal Green",
             "note": "Retro CRT read.",
             "patch": {"theme_mode": "dark", "accent_color": "#22c55e",
                       "roster_layout": "ledger"}},
        ],
        "profile": [
            {"id": "signal_card", "title": "Signal Card",
             "note": "Minimal, data-forward.",
             "patch": {"profile_card_style": "glass"}},
        ],
    },
    "/api/friends/requests": {
        "ok": True,
        "incoming": [{"request_id": 11, "account_id": "4",
                      "username": "newcomer", "avatar_url": ""}],
        "outgoing": [{"request_id": 12, "account_id": "5",
                      "username": "pending"}],
    },
    "/api/admin/overview": {
        "summary": {"accounts": 42, "open_reports": 3, "stale_nodes": 1},
        "recent_actions": [{"actor": "desmondc", "action": "TRUNCATE",
                            "created_at": "2026-10-03T20:00:00Z"}],
    },
    "/api/stability": {"status": "degraded",
                       "stale_nodes": [{"bot": "node-7", "stale_for": "3m"}],
                       "voice_failures": 2},
    "/api/metrics": {"queue_depth": 12, "heartbeat_drift_ms": 420},
    "/api/system-diagnostics": {"db": {"ok": True, "latency_ms": 4},
                                "redis": {"ok": True}},
    "/api/alert-rules": {"rules": [
        {"id": 1, "name": "Queue depth", "metric": "queue_depth",
         "comparison": ">", "threshold": 100, "enabled": True}]},
    "/api/exports": {"ok": True, "enabled": True, "snapshots": [
        {"date": "2026-10-03", "files": ["accounts.csv", "queues.csv"]}]},
    "/api/events": {"events": [
        {"kind": "voice_failure", "bot": "node-7",
         "created_at": "2026-10-03T21:00:00Z"}]},
    "/api/metrics/history": {"series": [
        {"metric": "queue_depth", "at": "2026-10-03T21:00:00Z", "value": 12}]},
    "/api/metrics/anomalies": {"anomalies": [
        {"metric": "heartbeat_drift_ms", "value": 9100, "severity": "warning"}]},
    "/api/audit-log": {"entries": [
        {"created_at": "2026-10-03T20:00:00Z", "actor": "desmondc",
         "action": "TRUNCATE", "target": "panel.sessions", "detail": "{}"}]},
    "/api/swarm-accounts/admin": {"accounts": [
        {"id": 2, "username": "nova", "email": "nova@example.com",
         "role": "user", "guild_id": "777", "email_verified": True,
         "moderator": False}]},
    # Table entries deliberately mix the three shapes the real backend has
    # used, so the tableName() fallback chain is exercised.
    "/api/databases": {"schemas": [
        {"schema": "panel", "tables": ["accounts", {"name": "sessions"},
                                       {"TABLE_NAME": "audit_log"}]},
        {"schema": "aria", "tables": [{"table_name": "telemetry"}]},
    ]},
    "/api/database/data": {"ok": True, "count": 2, "rows": [
        {"id": 1, "guild_id": "777", "panel_preferences": "{\"accent\":\"#8844ff\"}"},
        {"id": 2, "guild_id": "778", "panel_preferences": None},
    ]},
    "/api/image-gallery/admin": {
        "users": [{"id": 9, "username": "artist", "email": "a@example.com",
                   "age_verified": True, "email_verified": False}],
        "media": [{"id": 31, "title": "sunset.png", "owner_username": "artist"}],
        "reports": [{"id": 5, "reason": "spam", "status": "open",
                     "media_id": 31}],
        "comments": [{"id": 77, "body": "nice"}],
    },
    "/api/lumisound/admin": {
        "users": [{"id": 12, "username": "listener",
                   "email": "l@example.com", "active": True}],
        "uploads": [{"id": 4, "title": "loop.wav", "owner_username": "listener"}],
        "bug_reports": [{"id": 2, "title": "Crash on open",
                         "body": "Opens then dies", "status": "open"}],
    },
    "/api/notifications": {"notifications": [
        {"title": "Node stale", "body": "node-7 heartbeat stale",
         "created_at": "2026-10-03T21:30:00Z", "read": False}],
        "unread_count": 1},
    "/api/community/counts": {"counts": {"friend_requests": 1,
                                         "unread_messages": 2}},
}

PATTERNS = [
    (re.compile(r"^/api/bots/[^/]+/inventory$"), {"guilds": [
        {"id": "777", "name": "Witch Knot", "channels": [
            {"id": "888", "name": "music", "type": 2},
            {"id": "889", "name": "stage", "type": 13},
            {"id": "890", "name": "general", "type": 0}]}]}),
    (re.compile(r"^/api/bots/[^/]+/control-state$"), CONTROL_STATE),
    (re.compile(r"^/api/guilds/[^/]+/control-matrix$"), {"bots": [
        {"key": "aria", "display_name": "Aria", "session": {
            "title": "Night Drive", "is_playing": True, "queue_count": 12,
            "channel_name": "music", "session_state_label": "Playing"}},
        {"bot_key": "node-7", "bot_display": "Node 7",
         "error": "control state unavailable"}]}),
    (re.compile(r"^/api/messages/\d+$"), {"ok": True, "messages": [
        {"body": "hey", "mine": False, "username": "nova",
         "created_at": "2026-10-03T22:00:00Z"},
        {"body": "hi!", "mine": True, "created_at": "2026-10-03T22:01:00Z"}]}),
    (re.compile(r"^/api/users/\d+/profile$"), {"profile": {
        "account_id": "2", "username": "nova", "display_name": "Nova",
        "bio": "Runs the night shift.", "online": True,
        "visibility": "public", "created_at": "2025-06-01T12:00:00Z",
        "links": [{"label": "Site", "url": "https://example.com"}],
        "tags": ["night", "synth"]}}),
]

CALLS = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _payload(self):
        path = self.path.split("?", 1)[0]
        CALLS.append((self.command, path))
        if path in ROUTES:
            return ROUTES[path]
        for pattern, payload in PATTERNS:
            if pattern.match(path):
                return payload
        if path.endswith(".csv"):
            return None  # handled as raw bytes
        return {"ok": True}

    def _respond(self):
        path = self.path.split("?", 1)[0]
        if path.endswith(".csv"):
            data = b"a,b\n1,2\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/csv")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        payload = self._payload()
        data = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._respond()

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        self._respond()


def start():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}"
