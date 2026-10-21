"""Regression test: signing in must not gate every screen off.

`POST /api/session/login` answers with `ok` + a token and *no*
`authenticated` key -- only `GET /api/session` synthesises that (see
lua/src/routes.lua). A client that tests `authenticated` alone therefore
sees a freshly-signed-in owner as anonymous, which empties the sidebar and
makes navigation fail with "That screen isn't available for this session."

These assertions use the real payload shapes verbatim, so they fail if the
normalisation in nav.session_view is ever dropped.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import isolate  # noqa: F401  -- must precede every swarmpanel import

from swarmpanel.core.nav import can_see, locate, session_view, visible_sections

# Verbatim from routes.lua's POST /api/session/login return.
LOGIN_OWNER = {
    "ok": True, "token": "t", "username": "desmondc", "role": "owner",
    "guild_id": "777", "account_guild_id": "777", "site_owner": True,
    "admin_mode": True, "moderator": True, "image_gallery_owner": True,
    "pages_public_url": "https://example", "expires_in": 86400,
}

# Verbatim from routes.lua's POST /api/session/register return.
REGISTER_ACCOUNT = {
    "ok": True, "token": "t", "username": "newbie", "role": "account",
    "guild_id": "778", "account_guild_id": "778", "verification_sent": True,
}

# GET /api/session, both states.
SESSION_OK = dict(LOGIN_OWNER, authenticated=True, mode="token")
SESSION_ANON = {"authenticated": False, "pages_public_url": "https://example"}


def check(name, condition):
    print(f"  {'ok ' if condition else 'BAD'} {name}")
    return condition


failures = 0

view = session_view(LOGIN_OWNER)
failures += not check("login payload is treated as authenticated",
                      view["authenticated"] is True)
sections = [s.key for s in visible_sections(view)]
failures += not check(
    f"owner sees all five sections after login ({sections})",
    sections == ["fleet", "insights", "community", "account", "admin"])
failures += not check(
    "every nav item is reachable right after login",
    all(can_see(view, item) for section in visible_sections(view)
        for item in section.items))
failures += not check(
    "Dashboard specifically is reachable (the route _enter_shell opens)",
    can_see(view, locate("/")[1]))

reg = session_view(REGISTER_ACCOUNT)
regs = [s.key for s in visible_sections(reg)]
failures += not check(
    f"a newly registered account gets its own sections, minus Admin ({regs})",
    regs == ["fleet", "insights", "community", "account"])

failures += not check("GET /api/session (authed) still works",
                      session_view(SESSION_OK)["authenticated"] is True)
failures += not check("GET /api/session (anon) stays locked out",
                      visible_sections(session_view(SESSION_ANON)) == [])
failures += not check("an empty/None payload stays locked out",
                      visible_sections(session_view(None)) == []
                      and visible_sections(session_view({})) == [])

# image_gallery_owner is always derived as admin_mode AND site_owner, the
# same formula the server uses -- never taken from the payload, so a stale
# value cannot keep a withdrawn screen visible.
failures += not check(
    "gallery gate derived (admin+owner -> visible)",
    session_view({"token": "t", "admin_mode": True,
                  "site_owner": True})["image_gallery_owner"] is True)
failures += not check(
    "gallery gate derived (admin only -> hidden)",
    session_view({"token": "t", "admin_mode": True,
                  "site_owner": False})["image_gallery_owner"] is False)
failures += not check(
    "a stale image_gallery_owner=true is ignored when admin mode is off",
    session_view({"token": "t", "admin_mode": False, "site_owner": True,
                  "image_gallery_owner": True})["image_gallery_owner"] is False)

# Turning admin mode off must also withdraw the gallery screen.
off = session_view(dict(LOGIN_OWNER, admin_mode=False))
offs = [item.to for section in visible_sections(off) if section.key == "admin"
        for item in section.items]
failures += not check(
    f"admin mode off leaves only moderator tools ({offs})",
    offs == ["/admin", "/audit-log", "/lumisound-admin"])

print()
if failures:
    print(f"FAIL: {failures} assertion(s) failed")
    sys.exit(1)
print("ALL SESSION GATING TESTS PASSED")
