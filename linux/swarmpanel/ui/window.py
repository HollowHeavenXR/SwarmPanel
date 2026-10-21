"""Main window -- the port of RootTabView/ContentView plus the web panel's
sidebar shell.

Where iOS uses a five-tab bar, the desktop uses an Adwaita navigation split
view: the sidebar lists every section from core.nav (the same definition the
Lua sidebar is built from), gated by the live session's permissions, and the
content pane holds one lazily-constructed Screen per route.
"""

from __future__ import annotations

from typing import Any

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gio, GLib, Gtk

from ..core import api as api_module
from ..core import nav, work
from ..core.casing import field, fields
from ..core.config import settings
from ..core.live import live
from . import theme
from . import widgets as w
from .auth import AuthView
from .screens import build_screen


class MainWindow(Adw.ApplicationWindow):
    def __init__(self, application: Adw.Application) -> None:
        super().__init__(application=application, title="SwarmPanel")
        self.set_default_size(1180, 780)
        self.set_size_request(420, 480)

        self.session: dict = {}
        self._screens: dict[str, Any] = {}
        self._current_route: str | None = None
        self._session_source: int | None = None

        self.toast_overlay = Adw.ToastOverlay()
        self.set_content(self.toast_overlay)

        self._root_stack = Gtk.Stack()
        self._root_stack.set_transition_type(Gtk.StackTransitionType.CROSSFADE)
        self.toast_overlay.set_child(self._root_stack)

        self._auth = AuthView(on_signed_in=self._on_signed_in)
        self._root_stack.add_named(self._auth, "auth")
        self._root_stack.add_named(self._build_shell(), "shell")
        self._root_stack.add_named(w.spinner_page("Restoring session…"), "restoring")

        api_module.api.on_unauthorized = self._on_unauthorized
        live.add_connection_listener(self._on_connection_changed)

        self._boot()

    # -- shell -------------------------------------------------------------

    def _build_shell(self) -> Gtk.Widget:
        self._split = Adw.NavigationSplitView()
        self._split.set_min_sidebar_width(230)
        self._split.set_max_sidebar_width(300)

        # Sidebar
        self._sidebar_list = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        self._sidebar_list.set_margin_top(6)
        self._sidebar_list.set_margin_bottom(12)
        self._sidebar_list.set_margin_start(8)
        self._sidebar_list.set_margin_end(8)

        sidebar_header = Adw.HeaderBar()
        sidebar_header.set_show_end_title_buttons(False)
        sidebar_title = Adw.WindowTitle(title="SwarmPanel", subtitle="")
        self._sidebar_title = sidebar_title
        sidebar_header.set_title_widget(sidebar_title)
        sidebar_header.pack_end(self._build_menu_button())

        sidebar_view = Adw.ToolbarView()
        sidebar_view.add_top_bar(sidebar_header)
        sidebar_view.set_content(w.scrolled(self._sidebar_list))
        self._split.set_sidebar(
            Adw.NavigationPage(child=sidebar_view, title="Sections")
        )

        # Content
        self._content_stack = Gtk.Stack()
        self._content_stack.set_transition_type(Gtk.StackTransitionType.CROSSFADE)
        self._content_stack.set_vexpand(True)

        self._content_title = Adw.WindowTitle(title="Dashboard", subtitle="Fleet")
        content_header = Adw.HeaderBar()
        content_header.set_title_widget(self._content_title)

        self._offline_pill = w.pill("Offline", css="swarm-warn")
        self._offline_pill.set_visible(False)
        self._offline_pill.set_tooltip_text(
            "The live feed is disconnected; screens are falling back to polling."
        )
        content_header.pack_start(self._offline_pill)

        self._refresh_button = Gtk.Button(icon_name="view-refresh-symbolic")
        self._refresh_button.set_tooltip_text("Refresh this screen")
        self._refresh_button.connect("clicked", lambda _b: self._refresh_current())
        content_header.pack_end(self._refresh_button)

        self._bell = _NotificationsBell(self)
        content_header.pack_end(self._bell)

        self._admin_toggle = Gtk.ToggleButton(icon_name="emblem-system-symbolic")
        self._admin_toggle.set_tooltip_text("Admin mode")
        self._admin_toggle.connect("toggled", self._on_admin_toggled)
        self._admin_toggle.set_visible(False)
        content_header.pack_end(self._admin_toggle)

        content_view = Adw.ToolbarView()
        content_view.add_top_bar(content_header)
        content_view.set_content(self._content_stack)
        self._split.set_content(
            Adw.NavigationPage(child=content_view, title="SwarmPanel")
        )
        return self._split

    def _build_menu_button(self) -> Gtk.MenuButton:
        menu = Gio.Menu()
        menu.append("Refresh session", "win.refresh-session")
        menu.append("Server address", "win.server-address")
        menu.append("Sign out", "win.sign-out")
        menu.append("About SwarmPanel", "win.about")

        for name, handler in (
            ("refresh-session", lambda *_a: self._refresh_session()),
            ("server-address", lambda *_a: self._auth._edit_server()),
            ("sign-out", lambda *_a: self._sign_out()),
            ("about", lambda *_a: self._show_about()),
        ):
            action = Gio.SimpleAction.new(name, None)
            action.connect("activate", handler)
            self.add_action(action)

        button = Gtk.MenuButton(icon_name="open-menu-symbolic")
        button.set_menu_model(menu)
        button.set_tooltip_text("Menu")
        return button

    # -- session -----------------------------------------------------------

    def _boot(self) -> None:
        if not api_module.api.token:
            self._root_stack.set_visible_child_name("auth")
            return
        # A stored token may be stale; validate it before showing the shell
        # so the user never sees a panel full of 401s.
        self._root_stack.set_visible_child_name("restoring")
        work.run(
            api_module.refresh_session,
            on_success=self._on_signed_in,
            on_error=self._on_restore_failed,
        )

    def _on_restore_failed(self, message: str) -> None:
        if api_module.api.token:
            # Network trouble rather than a bad token: keep the session and
            # let the screens' own retries recover, instead of signing the
            # user out over a flaky connection.
            w.toast(self, message)
            # Minimal session: no gating fields, so only ungated screens
            # show until a refresh succeeds. Better than signing the user
            # out over a flaky connection.
            self._enter_shell({"authenticated": True,
                               "username": field(self.session, "username")})
            return
        self._root_stack.set_visible_child_name("auth")
        w.toast(self._auth, message)

    def _on_signed_in(self, payload: dict) -> None:
        self._enter_shell(payload if isinstance(payload, dict) else {})

    def _enter_shell(self, payload: dict) -> None:
        # Normalised, because a login/register payload and a GET /api/session
        # payload do not carry the same fields -- see nav.session_view.
        self.session = nav.session_view(payload)
        self._root_stack.set_visible_child_name("shell")
        self._apply_session()
        live.connect()
        # The `account` key carries the signed-in user's profile and their
        # panel appearance choices, and keeps them live across devices.
        live.watch("account", self._on_account_snapshot)
        live.watch("community_counts", self._on_counts_snapshot)
        self._bell.start()
        if self._current_route is None:
            self.navigate("/")
        # Renew a remember-device token periodically, the way every
        # GET /api/session does on the other clients.
        if self._session_source is None:
            self._session_source = GLib.timeout_add_seconds(900, self._tick_session)

    def _tick_session(self) -> bool:
        if not api_module.api.token:
            self._session_source = None
            return GLib.SOURCE_REMOVE
        work.run(api_module.refresh_session, on_success=self._on_session_payload,
                 on_error=lambda _m: None)
        return GLib.SOURCE_CONTINUE

    def _refresh_session(self) -> None:
        work.run(api_module.refresh_session, on_success=self._on_session_payload,
                 on_error=lambda message: w.toast(self, message))

    def _on_session_payload(self, payload: Any) -> None:
        if isinstance(payload, dict):
            self.session = nav.session_view(payload)
            self._apply_session()

    def _apply_session(self) -> None:
        """Rebuild the sidebar for this session's permissions and sync the
        admin-mode toggle."""
        username = field(self.session, "username", "")
        role = field(self.session, "role", "")
        self._sidebar_title.set_subtitle(" · ".join(x for x in (username, role) if x))

        is_staff = (
            field(self.session, "siteOwner", False) is True
            or field(self.session, "adminMode", False) is True
            or field(self.session, "moderator", False) is True
        )
        self._admin_toggle.set_visible(is_staff)
        self._admin_toggle.handler_block_by_func(self._on_admin_toggled)
        self._admin_toggle.set_active(field(self.session, "adminMode", False) is True)
        self._admin_toggle.handler_unblock_by_func(self._on_admin_toggled)

        self._rebuild_sidebar()

    def _on_admin_toggled(self, button: Gtk.ToggleButton) -> None:
        enabled = button.get_active()
        work.run(
            lambda: api_module.set_admin_mode(enabled),
            on_success=lambda _r: self._after_admin_mode(enabled),
            on_error=self._admin_mode_failed,
        )

    def _after_admin_mode(self, enabled: bool) -> None:
        # Re-derive rather than poking one key: image_gallery_owner is a
        # function of admin_mode, so flipping admin mode changes whether the
        # Gallery screen is visible too.
        updated = dict(self.session)
        updated["admin_mode"] = enabled
        self.session = nav.session_view(updated)
        self._apply_session()
        w.toast(self, f"Admin mode {'on' if enabled else 'off'}")
        # Admin screens may have just appeared or vanished; if the current
        # route is no longer visible to this session, fall back to Dashboard.
        located = nav.locate(self._current_route)
        if located and not nav.can_see(self.session, located[1]):
            self.navigate("/")

    def _admin_mode_failed(self, message: str) -> None:
        w.toast(self, message)
        self._apply_session()  # snap the toggle back to the server's truth

    def _on_unauthorized(self) -> None:
        GLib.idle_add(self._route_to_login, "Your session expired. Please log in again.")

    def _route_to_login(self, message: str) -> bool:
        self._teardown_session()
        self._root_stack.set_visible_child_name("auth")
        w.toast(self._auth, message)
        return GLib.SOURCE_REMOVE

    def _sign_out(self) -> None:
        w.confirm(
            self, "Sign out?", "You'll need your password to sign back in.",
            "Sign out", self._do_sign_out, destructive=False,
        )

    def _do_sign_out(self) -> None:
        api_module.logout()
        self._teardown_session()
        self._root_stack.set_visible_child_name("auth")

    def _teardown_session(self) -> None:
        self._bell.stop()
        live.disconnect()
        if self._session_source is not None:
            GLib.source_remove(self._session_source)
            self._session_source = None
        for screen in self._screens.values():
            screen.deactivate()
        # Drop every screen so the next sign-in starts clean rather than
        # showing the previous account's data for a frame.
        for route in list(self._screens):
            child = self._screens.pop(route)
            page = child.get_parent()
            if page is not None:
                self._content_stack.remove(child)
        self._current_route = None
        self.session = {}

    # -- navigation --------------------------------------------------------

    def _rebuild_sidebar(self) -> None:
        child = self._sidebar_list.get_first_child()
        while child is not None:
            nxt = child.get_next_sibling()
            self._sidebar_list.remove(child)
            child = nxt

        self._nav_buttons: dict[str, Gtk.Button] = {}
        self._badge_labels: dict[str, list[Gtk.Label]] = {}

        for section in nav.visible_sections(self.session):
            label = Gtk.Label(label=section.label.upper(), xalign=0)
            label.add_css_class("caption-heading")
            label.add_css_class("dim-label")
            label.set_margin_top(10)
            label.set_margin_start(6)
            header_box = Gtk.Box(spacing=6)
            header_box.append(label)
            if section.badge:
                badge = _badge_label()
                header_box.append(badge)
                self._badge_labels.setdefault(section.badge, []).append(badge)
            self._sidebar_list.append(header_box)

            group = Gtk.ListBox()
            group.set_selection_mode(Gtk.SelectionMode.NONE)
            group.add_css_class("navigation-sidebar")
            for item in section.items:
                group.append(self._nav_row(item))
            self._sidebar_list.append(group)

        self._highlight_route()

    def _nav_row(self, item: nav.NavItem) -> Gtk.Widget:
        box = Gtk.Box(spacing=10)
        box.append(Gtk.Image(icon_name=item.icon))
        label = Gtk.Label(label=item.label, xalign=0)
        label.set_hexpand(True)
        box.append(label)
        if item.badge:
            badge = _badge_label()
            box.append(badge)
            self._badge_labels.setdefault(item.badge, []).append(badge)

        button = Gtk.Button()
        button.set_child(box)
        button.add_css_class("flat")
        button.set_tooltip_text(item.blurb)
        button.connect("clicked", lambda _b, to=item.to: self.navigate(to))
        self._nav_buttons[item.to] = button
        row = Gtk.ListBoxRow()
        row.set_child(button)
        row.set_activatable(False)
        return row

    def navigate(self, route: str) -> None:
        located = nav.locate(route)
        if located is None:
            return
        section, item = located
        if not nav.can_see(self.session, item):
            w.toast(self, "That screen isn't available for this session.")
            return

        previous = self._screens.get(self._current_route) if self._current_route else None
        if previous is not None and self._current_route != route:
            previous.deactivate()

        screen = self._screens.get(route)
        if screen is None:
            screen = build_screen(route, self, item)
            self._screens[route] = screen
            self._content_stack.add_named(w.scrolled(screen) if screen.scrollable else screen, route)
        self._current_route = route
        self._content_stack.set_visible_child_name(route)
        self._content_title.set_title(item.label)
        self._content_title.set_subtitle(section.label)
        screen.activate()
        self._highlight_route()
        if self._split.get_collapsed():
            self._split.set_show_content(True)

    def _highlight_route(self) -> None:
        for route, button in getattr(self, "_nav_buttons", {}).items():
            if nav.is_active(self._current_route, route):
                button.add_css_class("swarm-nav-active")
            else:
                button.remove_css_class("swarm-nav-active")

    def _refresh_current(self) -> None:
        screen = self._screens.get(self._current_route or "")
        if screen is not None:
            screen.refresh(show_spinner=False)
            w.toast(self, "Refreshing…", timeout=1)

    # -- live feeds --------------------------------------------------------

    def _on_connection_changed(self, connected: bool) -> None:
        self._offline_pill.set_visible(not connected)

    def _on_account_snapshot(self, payload: Any) -> None:
        """The account feed carries profile + appearance; mirroring the
        appearance choice keeps the desktop app matching the web panel the
        way the tvOS app does."""
        account = field(payload, "account", payload)
        username = field(account, "displayName") or field(account, "username") or \
            field(self.session, "username", "")
        role = field(self.session, "role", "")
        self._sidebar_title.set_subtitle(" · ".join(x for x in (username, role) if x))
        self.account = account

        # Appearance is an account-level setting, so apply it whenever the
        # feed carries it -- the Appearance screen may never be opened, and
        # another device may have just changed it.
        preferences = field(account, "preferences") or field(account, "panelPreferences")
        if preferences:
            theme.apply_preferences(self, preferences)

        screen = self._screens.get("/appearance")
        if screen is not None:
            screen.on_account(account)

    def _on_counts_snapshot(self, payload: Any) -> None:
        counts = field(payload, "counts", payload)
        # The endpoint answers {unread_messages, pending_friend_requests};
        # the shorter spellings are kept as fallbacks.
        friends = _as_int(fields(counts, "pendingFriendRequests", "friendRequests",
                                 "friends", default=0))
        messages = _as_int(fields(counts, "unreadMessages", "messages", default=0))
        values = {
            "friends": friends,
            "messages": messages,
            "community": friends + messages,
        }
        for name, labels in getattr(self, "_badge_labels", {}).items():
            amount = values.get(name, 0)
            for label in labels:
                label.set_text(str(amount) if amount else "")
                label.set_visible(bool(amount))

    # -- about -------------------------------------------------------------

    def _show_about(self) -> None:
        about = Adw.AboutDialog(
            application_name="SwarmPanel",
            application_icon="studio.xenusanimations.SwarmPanel",
            developer_name="HollowHeavenXR",
            version="1.0.0",
            comments=(
                "Native Linux client for SwarmPanel — the command centre for "
                "Aria and the music bot fleet."
            ),
            website="https://swarmpanel.xenusanimations.studio",
            copyright="© HollowHeavenXR",
        )
        about.add_credit_section("Backend", ["Lua panel + FastAPI services"])
        about.present(self)


def _badge_label() -> Gtk.Label:
    label = Gtk.Label(label="")
    label.add_css_class("swarm-badge")
    label.add_css_class("caption-heading")
    label.set_visible(False)
    return label


def _as_int(value: Any) -> int:
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return 0


class _NotificationsBell(Gtk.MenuButton):
    """Port of NotificationsBell -- unread count plus a popover list."""

    def __init__(self, window: MainWindow) -> None:
        super().__init__(icon_name="preferences-system-notifications-symbolic")
        self.set_tooltip_text("Notifications")
        self._window = window
        self._popover = Gtk.Popover()
        self._popover.set_size_request(380, -1)
        self._list = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        self._list.set_margin_top(8)
        self._list.set_margin_bottom(8)
        self._list.set_margin_start(8)
        self._list.set_margin_end(8)

        header = Gtk.Box(spacing=8)
        header.append(w.heading("Notifications", level=4))
        spacer = Gtk.Box(hexpand=True)
        header.append(spacer)
        mark = Gtk.Button(label="Mark all read")
        mark.add_css_class("flat")
        mark.connect("clicked", lambda _b: self._mark_all_read())
        header.append(mark)

        column = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        column.append(header)
        column.append(w.scrolled(self._list))
        column.set_margin_top(8)
        column.set_margin_bottom(8)
        column.set_margin_start(8)
        column.set_margin_end(8)
        self._popover.set_child(column)
        self.set_popover(self._popover)

    def start(self) -> None:
        live.watch("notifications", self._on_snapshot)

    def stop(self) -> None:
        live.unwatch("notifications")

    def _on_snapshot(self, payload: Any) -> None:
        items = field(payload, "notifications", []) or []
        unread = _as_int(field(payload, "unreadCount", 0)) or sum(
            1 for item in items if not field(item, "read", False)
        )
        # There is no distinct "unread" bell icon in the theme, so the
        # unread state is carried by an accent class plus the tooltip.
        if unread:
            self.add_css_class("accent")
        else:
            self.remove_css_class("accent")
        self.set_tooltip_text(f"{unread} unread notification{'s' if unread != 1 else ''}"
                              if unread else "Notifications")

        child = self._list.get_first_child()
        while child is not None:
            nxt = child.get_next_sibling()
            self._list.remove(child)
            child = nxt

        if not items:
            self._list.append(w.dim("Nothing new."))
            return
        for item in items[:40]:
            row = Adw.ActionRow(
                title=str(field(item, "title", field(item, "kind", "Notification"))),
                subtitle=str(field(item, "body", field(item, "message", ""))),
            )
            row.set_subtitle_lines(3)
            created = field(item, "createdAt") or field(item, "created")
            if created:
                row.add_suffix(w.dim(w.format_relative(created), wrap=False))
            if not field(item, "read", False):
                row.add_prefix(Gtk.Image(icon_name="media-record-symbolic"))
                notification_id = field(item, "id")
                if notification_id is not None:
                    mark = Gtk.Button(icon_name="object-select-symbolic")
                    mark.add_css_class("flat")
                    mark.set_valign(Gtk.Align.CENTER)
                    mark.set_tooltip_text("Mark read")
                    mark.connect("clicked",
                                 lambda _b, n=notification_id: self._mark_read(n))
                    row.add_suffix(mark)
            self._list.append(row)

    def _mark_read(self, notification_id: Any) -> None:
        work.run(
            lambda: api_module.api.post(f"/api/notifications/{notification_id}/read"),
            # No refresh needed: the notifications feed pushes the new state.
            on_success=lambda _r: None,
            on_error=lambda message: w.toast(self._window, message),
        )

    def _mark_all_read(self) -> None:
        work.run(
            lambda: api_module.api.post("/api/notifications/read-all"),
            on_success=lambda _r: w.toast(self._window, "Notifications marked read"),
            on_error=lambda message: w.toast(self._window, message),
        )
