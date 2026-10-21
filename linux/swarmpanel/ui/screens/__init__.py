"""Screen registry -- maps a nav route to the Screen subclass that serves it.

Kept as a lazy lookup so starting the app doesn't import and construct all
nineteen screens; the window builds each one the first time it is visited.
"""

from __future__ import annotations

from typing import Any

from ...core import nav
from .base import Screen


def build_screen(route: str, window: Any, item: nav.NavItem) -> Screen:
    from . import account, admin, community, fleet, insights

    registry: dict[str, type[Screen]] = {
        "/": fleet.DashboardScreen,
        "/controls": fleet.ControlsScreen,
        "/invites": fleet.InvitesScreen,
        "/leaderboard": insights.LeaderboardScreen,
        "/learning": insights.LearningScreen,
        "/users": community.DirectoryScreen,
        "/friends": community.FriendsScreen,
        "/messages": community.MessagesScreen,
        "/profile": account.ProfileScreen,
        "/appearance": account.AppearanceScreen,
        "/other-projects": account.OtherProjectsScreen,
        "/admin": admin.OverviewScreen,
        "/diagnostics": admin.DiagnosticsScreen,
        "/intel": admin.IntelScreen,
        "/audit-log": admin.AuditLogScreen,
        "/accounts": admin.AccountsScreen,
        "/databases": admin.DatabasesScreen,
        "/gallery-admin": admin.GalleryScreen,
        "/lumisound-admin": admin.LumisoundScreen,
    }
    screen_class = registry.get(route)
    if screen_class is None:
        raise KeyError(f"no screen registered for {route}")
    return screen_class(window, title=item.label, subtitle=item.blurb)


__all__ = ["Screen", "build_screen"]
