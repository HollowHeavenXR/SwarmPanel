"""Panel information architecture -- port of lua/src/nav.lua.

Every screen belongs to exactly one section, and the sidebar, the header
breadcrumb, and the Admin overview grid are all derived from this one table,
same as the web panel's sidebar/drawer/tab-strip are. Adding a screen means
adding one entry here.

`badge` names a live count fed by the `community_counts` watch key:
"messages", "friends", or "community" for their sum on a section heading.

`when` gates visibility on the session shape:
  None      -> any signed-in session
  "admin"   -> admin mode on
  "mod"     -> admin mode on, or a moderator
  "gallery" -> image-gallery owner (admin mode + site owner)
  "staff"   -> any of the above (the Admin section's overview)

Glyphs are the icon *names* a desktop app wants rather than nav.lua's text
glyphs: these are symbolic icons from the GNOME icon theme, which is what
looks native in a GTK sidebar. The labels, order, blurbs, routes and
permission gates are kept identical to the Lua.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

from .casing import field as payload_field


@dataclass(frozen=True)
class NavItem:
    to: str
    label: str
    icon: str
    blurb: str
    when: str | None = None
    badge: str | None = None


@dataclass(frozen=True)
class NavSection:
    key: str
    label: str
    icon: str
    blurb: str
    items: tuple[NavItem, ...]
    badge: str | None = None


SECTIONS: tuple[NavSection, ...] = (
    NavSection(
        key="fleet", label="Fleet", icon="view-grid-symbolic",
        blurb="Live bot status and direct playback control.",
        items=(
            NavItem("/", "Dashboard", "view-grid-symbolic",
                    "Live status across the swarm."),
            NavItem("/controls", "Controls", "media-playback-start-symbolic",
                    "Send orders to any bot in any guild."),
            NavItem("/invites", "Invites", "insert-link-symbolic",
                    "Invite links for every bot."),
        ),
    ),
    NavSection(
        key="insights", label="Insights", icon="org.gnome.Settings-search-symbolic",
        blurb="What the swarm is playing and learning.",
        items=(
            NavItem("/leaderboard", "Leaderboard", "starred-symbolic",
                    "Top tracks across the swarm."),
            NavItem("/learning", "Learning", "emoji-symbols-symbolic",
                    "The recommendation engine's memory."),
        ),
    ),
    NavSection(
        key="community", label="Community", icon="system-users-symbolic",
        badge="community",
        blurb="Other operators, friends, and direct messages.",
        items=(
            NavItem("/users", "Directory", "system-users-symbolic",
                    "Find other operators."),
            NavItem("/friends", "Friends", "contact-new-symbolic",
                    "Requests and confirmed friends.", badge="friends"),
            NavItem("/messages", "Messages", "mail-unread-symbolic",
                    "Direct messages.", badge="messages"),
        ),
    ),
    NavSection(
        key="account", label="Account", icon="avatar-default-symbolic",
        blurb="Your profile and how the panel looks to you.",
        items=(
            NavItem("/profile", "Profile", "avatar-default-symbolic",
                    "How you appear across the panel."),
            NavItem("/appearance", "Appearance", "applications-graphics-symbolic",
                    "Theme, layout, and motion."),
            NavItem("/other-projects", "Other Projects", "application-x-addon-symbolic",
                    "Other things I've built."),
        ),
    ),
    NavSection(
        key="admin", label="Admin", icon="emblem-system-symbolic",
        blurb="Owner and moderator tooling.",
        items=(
            NavItem("/admin", "Overview", "emblem-system-symbolic",
                    "Every admin tool in one place.", when="staff"),
            NavItem("/diagnostics", "Diagnostics", "system-run-symbolic",
                    "Stability, metrics, alert rules, and exports.", when="admin"),
            NavItem("/intel", "Intel", "dialog-warning-symbolic",
                    "Trends, anomalies, and raw events.", when="admin"),
            NavItem("/audit-log", "Audit Log", "view-list-symbolic",
                    "Every recorded admin action.", when="mod"),
            NavItem("/accounts", "Accounts", "system-users-symbolic",
                    "Recover and manage swarm accounts.", when="admin"),
            NavItem("/databases", "Databases", "drive-harddisk-symbolic",
                    "Browse raw schema tables.", when="admin"),
            NavItem("/gallery-admin", "Gallery", "image-x-generic-symbolic",
                    "Image Gallery users, media, and reports.", when="gallery"),
            NavItem("/lumisound-admin", "Lumisound", "audio-x-generic-symbolic",
                    "Lumisound iOS app accounts and library.", when="mod"),
        ),
    ),
)

# Paths that belong to a section without being a nav entry of their own; the
# breadcrumb and active-state logic still need to place them.
ALIASES = {"/dashboard": "/"}


def session_view(payload: Any) -> dict[str, Any]:
    """Normalise a session payload into the shape the gates expect.

    Port of page_kit.session_view(). This exists because the panel's gating
    fields are *derived server-side for the HTML pages*, and the REST
    endpoints do not all return the same shape:

    * `GET /api/session` returns `authenticated` explicitly (true, or false
      when the token is no good).
    * `POST /api/session/login` and `/api/session/register` do **not**. They
      signal success with `ok = true` plus a token, so a client that checks
      `authenticated` alone sees a freshly-signed-in session as anonymous --
      which gated every screen off and left the sidebar empty.
    * `image_gallery_owner` is always *derived* here as
      `admin_mode and site_owner`, which is precisely how the server
      computes it -- never read from the payload. Trusting the field instead
      left a stale `true` in place when admin mode was switched off, so the
      Gallery screen stayed in the sidebar after the authority for it was
      withdrawn.

    Returning one canonical dict means `can_see` has a single shape to
    reason about, whichever endpoint the payload came from.
    """
    if not payload:
        return {"authenticated": False}

    authenticated = payload_field(payload, "authenticated", None)
    if authenticated is None:
        # No explicit flag: this is a login/register response, where a token
        # (or `ok`) is what says the session is real.
        authenticated = bool(
            payload_field(payload, "token")
            or payload_field(payload, "ok", False)
            or payload_field(payload, "username")
        )
    if authenticated is not True and not authenticated:
        return {"authenticated": False}

    admin_mode = payload_field(payload, "adminMode", False) is True
    site_owner = payload_field(payload, "siteOwner", False) is True
    gallery_owner = admin_mode and site_owner

    return {
        "authenticated": True,
        "username": payload_field(payload, "username"),
        "role": payload_field(payload, "role"),
        "site_owner": site_owner,
        "admin_mode": admin_mode,
        "moderator": payload_field(payload, "moderator", False) is True,
        "image_gallery_owner": gallery_owner,
        "guild_id": payload_field(payload, "guildId")
        or payload_field(payload, "accountGuildId"),
    }


def can_see(session: Any, item: NavItem) -> bool:
    """Port of nav.lua's M.can_see -- same gate names, same semantics."""
    if not session or not payload_field(session, "authenticated", False):
        return False
    gate = item.when
    if gate is None:
        return True
    admin = payload_field(session, "adminMode", False) is True
    mod = payload_field(session, "moderator", False) is True
    gallery = payload_field(session, "imageGalleryOwner", False) is True
    if gate == "admin":
        return admin
    if gate == "mod":
        return admin or mod
    if gate == "gallery":
        return gallery
    if gate == "staff":
        return admin or mod or gallery
    return False


def visible_sections(session: Any) -> list[NavSection]:
    """Sections with only the items this session may see; sections left
    empty are dropped entirely, so a regular user never sees an "Admin"
    heading at all."""
    out: list[NavSection] = []
    for section in SECTIONS:
        items = tuple(item for item in section.items if can_see(session, item))
        if items:
            out.append(
                NavSection(
                    key=section.key, label=section.label, icon=section.icon,
                    blurb=section.blurb, badge=section.badge, items=items,
                )
            )
    return out


def is_active(pathname: str | None, to: str) -> bool:
    if not pathname:
        return False
    pathname = ALIASES.get(pathname, pathname)
    if to == "/":
        return pathname == "/"
    return pathname == to or pathname.startswith(to + "/")


def locate(pathname: str | None) -> tuple[NavSection, NavItem] | None:
    """Section + item for a path; longest matching prefix wins, so
    `/users/42` lands on Directory. None for paths outside the nav."""
    if not pathname:
        return None
    best: tuple[NavSection, NavItem] | None = None
    best_len = -1
    for section in SECTIONS:
        for item in section.items:
            if is_active(pathname, item.to) and len(item.to) > best_len:
                best, best_len = (section, item), len(item.to)
    return best


def all_items() -> list[NavItem]:
    return [item for section in SECTIONS for item in section.items]
