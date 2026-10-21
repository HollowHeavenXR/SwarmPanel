"""Community section -- Directory, Friends, Messages.

Ported from Features/Discover and Features/Social. Friends and Messages are
driven by the `friends`, `threads` and `thread_messages` watch keys; the
`community_counts` key feeds the sidebar badges from the window, not here.
"""

from __future__ import annotations

from typing import Any

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gtk

from ...core import work
from ...core.api import api
from ...core.casing import field, fields
from ...core.live import live
from .. import widgets as w
from .base import Screen


def _person_name(person: Any) -> str:
    return str(fields(person, "displayName", "username", "name", "accountId",
                      default=w.DASH))


def _avatar_of(person: Any) -> Any:
    return fields(person, "avatarUrl", "avatar", "iconUrl",
                  "profileAvatarUrl", default=None)


def _presence(person: Any) -> tuple[str, str]:
    """(label, css) for the online/inactive pill the panel shows."""
    # The directory calls it `is_online`; `online` is kept as a fallback
    # for any payload that still uses the shorter name.
    if fields(person, "isOnline", "online", default=False) is True:
        return "online", "swarm-ok"
    last = fields(person, "lastSeen", "lastSeenAt", "lastActiveAt", default=None)
    if last:
        return w.format_relative(last), ""
    if field(person, "inactive") is True:
        return "inactive", "swarm-warn"
    return "", ""


class DirectoryScreen(Screen):
    """Profile discovery -- GET /api/users/directory."""

    fallback_interval = 0.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.query = ""
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        return api.get("/api/users/directory",
                       {"q": self.query or None, "limit": "100"})

    def render(self, payload: Any) -> Gtk.Widget:
        people = fields(payload, "users", "accounts", "profiles", default=[]) or []
        box = self.body()

        search = Gtk.SearchEntry()
        search.set_placeholder_text("Search operators…")
        search.set_text(self.query)
        search.connect("search-changed", self._on_search)
        search_group = Adw.PreferencesGroup()
        search_group.add(search)
        box.append(search_group)

        if not people:
            box.append(w.empty_state(
                "No operators found",
                "Try a different search." if self.query
                else "Nobody has a visible profile yet.",
                "system-users-symbolic",
            ))
            return w.clamp_of(box)

        group = w.section("Operators", f"{len(people)} visible profile(s)")
        for person in people:
            group.add(self._person_row(person))
        box.append(group)
        return w.clamp_of(box)

    def _person_row(self, person: Any) -> Gtk.Widget:
        row = Adw.ActionRow(title=_person_name(person))
        row.add_prefix(w.avatar(_avatar_of(person), _person_name(person)))
        bits = []
        bio = field(person, "bio")
        if bio:
            bits.append(str(bio))
        guild = fields(person, "guildName", "guildId", default=None)
        if guild:
            bits.append(f"guild {guild}")
        row.set_subtitle(" · ".join(bits))
        row.set_subtitle_lines(2)

        label, css = _presence(person)
        if label:
            row.add_suffix(w.pill(label, css=css))

        account_id = fields(person, "accountId", "id", default=None)
        if account_id:
            view = Gtk.Button(label="Profile")
            view.add_css_class("flat")
            view.set_valign(Gtk.Align.CENTER)
            view.connect("clicked", lambda _b, a=account_id: self._open_profile(a))
            row.add_suffix(view)

            add = Gtk.Button(icon_name="contact-new-symbolic")
            add.add_css_class("flat")
            add.set_valign(Gtk.Align.CENTER)
            add.set_tooltip_text("Send a friend request")
            add.connect("clicked", lambda _b, a=account_id: self._add_friend(a))
            row.add_suffix(add)
        return row

    def _on_search(self, entry: Gtk.SearchEntry) -> None:
        self.query = entry.get_text().strip()
        self.refresh(show_spinner=False)

    def _add_friend(self, account_id: Any) -> None:
        # The request is addressed by path, not by body -- and the response
        # reports the resulting state, which is "friends" straight away when
        # the other side had already asked.
        def done(result: Any) -> None:
            status = str(field(result, "status", ""))
            w.toast(self.window,
                    "You're now friends." if status == "friends"
                    else "Friend request sent.")

        self.act(
            lambda: api.post(f"/api/users/{account_id}/friend-request"),
            then=done,
        )

    def _open_profile(self, account_id: Any) -> None:
        PublicProfileDialog(self.window, account_id).present(self.get_root())


class PublicProfileDialog(Adw.Dialog):
    """Port of PublicProfileView -- a public profile page, as a dialog."""

    def __init__(self, window: Any, account_id: Any) -> None:
        super().__init__()
        self.set_title("Profile")
        self.set_content_width(560)
        self.set_content_height(620)
        self._window = window
        self._account_id = account_id

        self._stack = Gtk.Stack()
        self._stack.add_named(w.spinner_page(), "loading")
        self._body = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self._stack.add_named(w.scrolled(self._body), "content")
        self._stack.set_visible_child_name("loading")

        header = Adw.HeaderBar()
        view = Adw.ToolbarView()
        view.add_top_bar(header)
        view.set_content(self._stack)
        self.set_child(view)

        work.run(
            lambda: api.get(f"/api/users/{account_id}/profile"),
            on_success=self._render,
            on_error=self._failed,
        )

    def _failed(self, message: str) -> None:
        self._body.append(w.error_state(message))
        self._stack.set_visible_child_name("content")

    def _render(self, payload: Any) -> None:
        profile = fields(payload, "profile", "user", "account", default=payload)
        box = w.page_body()
        header = Gtk.Box(spacing=14)
        header.append(w.avatar(_avatar_of(profile), _person_name(profile), size=72))
        titles = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        titles.set_valign(Gtk.Align.CENTER)
        titles.append(w.heading(_person_name(profile), level=2))
        username = field(profile, "username")
        if username:
            titles.append(w.dim(f"@{username}", wrap=False))
        header.append(titles)
        box.append(header)

        label, css = _presence(profile)
        if label:
            pill_box = Gtk.Box()
            pill_box.append(w.pill(label, css=css))
            box.append(pill_box)

        bio = field(profile, "bio")
        if bio:
            bio_label = Gtk.Label(label=str(bio), xalign=0)
            bio_label.set_wrap(True)
            box.append(bio_label)

        group = w.section("Details")
        for title, key in (
            ("Username", "username"), ("Display name", "displayName"),
            ("Guild", "guildName"), ("Joined", "createdAt"),
            ("Visibility", "visibility"),
        ):
            value = field(profile, key)
            if value:
                group.add(w.key_value(
                    title,
                    w.format_absolute(value) if key == "createdAt" else value,
                ))
        box.append(group)

        links = fields(profile, "links", "profileLinks", default=[]) or []
        if links:
            links_group = w.section("Links")
            for link in links:
                url = fields(link, "url", "href", default=None) if isinstance(link, dict) else link
                if not url:
                    continue
                row = Adw.ActionRow(
                    title=str(fields(link, "label", "title", default=url)
                              if isinstance(link, dict) else url),
                    subtitle=str(url),
                )
                open_button = Gtk.Button(icon_name="adw-external-link-symbolic")
                open_button.add_css_class("flat")
                open_button.set_valign(Gtk.Align.CENTER)
                open_button.connect("clicked",
                                    lambda _b, u=str(url): Gtk.UriLauncher(uri=u).launch())
                row.add_suffix(open_button)
                links_group.add(row)
            box.append(links_group)

        actions = Gtk.Box(spacing=8)
        follow = Gtk.Button(label="Follow")
        follow.add_css_class("pill")
        follow.add_css_class("suggested-action")
        follow.connect("clicked", lambda _b: self._follow())
        actions.append(follow)
        friend = Gtk.Button(label="Add friend")
        friend.add_css_class("pill")
        friend.connect("clicked", lambda _b: self._friend_request())
        actions.append(friend)
        message = Gtk.Button(label="Message")
        message.add_css_class("pill")
        message.connect("clicked", lambda _b: self._message())
        actions.append(message)
        box.append(actions)

        tags = fields(profile, "tags", "profileTags", default=[]) or []
        if tags:
            tag_box = Gtk.Box(spacing=6)
            for tag in tags[:12]:
                tag_box.append(w.pill(str(tag)))
            box.append(tag_box)

        self._body.append(w.clamp_of(box))
        self._stack.set_visible_child_name("content")

    # -- actions on the viewed profile ------------------------------------

    def _act(self, work_fn, success: str) -> None:
        work.run(
            work_fn,
            on_success=lambda _r: w.toast(self._window, success),
            on_error=lambda message: w.toast(self._window, message),
        )

    def _follow(self) -> None:
        self._act(lambda: api.post(f"/api/users/{self._account_id}/follow"),
                  "Following.")

    def _friend_request(self) -> None:
        self._act(
            lambda: api.post(f"/api/users/{self._account_id}/friend-request"),
            "Friend request sent.")

    def _message(self) -> None:
        self.close()
        self._window.navigate("/messages")
        screen = self._window._screens.get("/messages")
        if screen is not None:
            screen.open_thread(str(self._account_id))


class FriendsScreen(Screen):
    """Requests and confirmed friends -- the `friends` watch key."""

    watch_key = "friends"
    fallback_interval = 30.0

    def load(self) -> Any:
        """Two endpoints: /api/me/friends is confirmed friends only, and the
        pending requests live on /api/friends/requests as `incoming` and
        `outgoing`."""
        friends = field(api.get("/api/me/friends"), "friends", []) or []
        requests = api.get("/api/friends/requests")
        return {
            "friends": friends,
            "incoming": field(requests, "incoming", []) or [],
            "outgoing": field(requests, "outgoing", []) or [],
        }

    def render(self, payload: Any) -> Gtk.Widget:
        incoming = field(payload, "incoming", []) or []
        outgoing = field(payload, "outgoing", []) or []
        friends = field(payload, "friends", []) or []
        box = self.body()

        if incoming:
            group = w.section("Requests", f"{len(incoming)} waiting on you")
            for person in incoming:
                group.add(self._request_row(person))
            box.append(group)

        if outgoing:
            group = w.section("Sent", f"{len(outgoing)} awaiting a reply")
            for person in outgoing:
                row = Adw.ActionRow(title=_person_name(person))
                row.set_subtitle("Request sent")
                cancel = Gtk.Button(label="Cancel")
                cancel.add_css_class("flat")
                cancel.set_valign(Gtk.Align.CENTER)
                cancel.connect(
                    "clicked",
                    lambda _b, p=person: self._respond(p, "cancel", "Request cancelled."),
                )
                row.add_suffix(cancel)
                group.add(row)
            box.append(group)

        if friends:
            group = w.section("Friends", f"{len(friends)} confirmed")
            for person in friends:
                row = Adw.ActionRow(title=_person_name(person))
                row.add_prefix(w.avatar(_avatar_of(person), _person_name(person)))
                label, css = _presence(person)
                if label:
                    row.add_suffix(w.pill(label, css=css))
                message = Gtk.Button(icon_name="mail-send-symbolic")
                message.add_css_class("flat")
                message.set_valign(Gtk.Align.CENTER)
                message.set_tooltip_text("Message")
                message.connect("clicked", lambda _b, p=person: self._message(p))
                row.add_suffix(message)
                # No "remove friend" button: the backend exposes no unfriend
                # route (social.lua only has accept/decline/cancel on a
                # pending request), so offering one would always fail.
                profile = Gtk.Button(icon_name="avatar-default-symbolic")
                profile.add_css_class("flat")
                profile.set_valign(Gtk.Align.CENTER)
                profile.set_tooltip_text("View profile")
                profile.connect(
                    "clicked",
                    lambda _b, p=person: PublicProfileDialog(
                        self.window, self._account_id(p)).present(self.get_root()),
                )
                row.add_suffix(profile)
                group.add(row)
            box.append(group)

        if not (incoming or outgoing or friends):
            box.append(w.empty_state(
                "No friends yet",
                "Find operators in the Directory and send a request.",
                "contact-new-symbolic",
            ))
        return w.clamp_of(box)

    def _request_row(self, person: Any) -> Gtk.Widget:
        row = Adw.ActionRow(title=_person_name(person))
        row.add_prefix(w.avatar(_avatar_of(person), _person_name(person)))
        row.set_subtitle("Wants to be friends")
        accept = Gtk.Button(label="Accept")
        accept.add_css_class("suggested-action")
        accept.add_css_class("pill")
        accept.set_valign(Gtk.Align.CENTER)
        accept.connect("clicked",
                       lambda _b, p=person: self._respond(p, "accept", "Friend added."))
        decline = Gtk.Button(label="Decline")
        decline.add_css_class("flat")
        decline.set_valign(Gtk.Align.CENTER)
        decline.connect("clicked",
                        lambda _b, p=person: self._respond(p, "decline", "Request declined."))
        row.add_suffix(decline)
        row.add_suffix(accept)
        return row

    def _account_id(self, person: Any) -> str:
        return str(fields(person, "accountId", "id", "requesterId",
                          "addresseeAccountId", default=""))

    @staticmethod
    def _request_id(person: Any) -> str:
        """Requests are answered by *request* id, not account id."""
        return str(fields(person, "requestId", "id", default=""))

    def _respond(self, person: Any, action: str, success: str) -> None:
        # social.lua accepts exactly accept / decline / cancel.
        request_id = self._request_id(person)
        if not request_id:
            w.toast(self.window, "That request is missing its id.")
            return
        self.act(
            lambda: api.post(f"/api/friends/requests/{request_id}",
                             {"action": action}),
            success=success,
        )

    def _message(self, person: Any) -> None:
        account_id = self._account_id(person)
        if not account_id:
            return
        self.window.navigate("/messages")
        screen = self.window._screens.get("/messages")
        if screen is not None:
            screen.open_thread(account_id, _person_name(person))


class MessagesScreen(Screen):
    """Direct messages -- `threads` for the list, `thread_messages` for the
    open conversation (scoped by an `account_id` param)."""

    watch_key = "threads"
    fallback_interval = 30.0
    scrollable = False

    def __init__(self, window: Any, **kwargs) -> None:
        self.threads: list[Any] = []
        self.active_account_id: str = ""
        self.active_name: str = ""
        self.messages: list[Any] = []
        super().__init__(window, **kwargs)

    # -- data

    def load(self) -> Any:
        return api.get("/api/messages/threads")

    def apply(self, payload: Any) -> None:
        self.threads = fields(payload, "threads", "conversations", default=[]) or []
        super().apply(payload)
        # Open the newest conversation on arrival; an empty right-hand pane
        # when there are threads to read is just a wasted click.
        if not self.active_account_id and self.threads:
            first = self.threads[0]
            account_id = str(fields(first, "accountId", "otherAccountId", "id",
                                    default=""))
            if account_id:
                self.open_thread(account_id, _person_name(first))

    def deactivate(self) -> None:
        # The conversation feed is a second watch key this screen owns, so it
        # has to be released alongside the thread list.
        live.unwatch("thread_messages")
        super().deactivate()

    def open_thread(self, account_id: str, name: str = "") -> None:
        self.active_account_id = str(account_id)
        self.active_name = name or self.active_name
        live.watch(
            "thread_messages",
            self._on_messages,
            params={"account_id": self.active_account_id},
            on_error=lambda message: w.toast(self.window, message),
        )
        work.run(
            # The conversation lives at /api/messages/:account_id;
            # /api/messages/threads is only the thread *list*.
            lambda: api.get(f"/api/messages/{self.active_account_id}",
                            {"limit": "80"}),
            on_success=self._on_messages,
            on_error=lambda message: w.toast(self.window, message),
        )
        self.apply(self.payload)

    def _on_messages(self, payload: Any) -> None:
        self.messages = fields(payload, "messages", "thread", default=[]) or []
        self._fill_transcript()

    # -- rendering

    def render(self, payload: Any) -> Gtk.Widget:
        split = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
        split.set_vexpand(True)

        # Thread list
        list_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        list_box.set_margin_top(8)
        list_box.set_margin_bottom(8)
        list_box.set_margin_start(8)
        list_box.set_margin_end(8)
        list_box.set_size_request(260, -1)
        if not self.threads:
            list_box.append(w.dim("No conversations yet."))
        for thread in self.threads:
            list_box.append(self._thread_row(thread))
        list_scroller = w.scrolled(list_box)
        list_scroller.set_size_request(290, -1)
        # Without this the list shares the width evenly with the
        # conversation, leaving a 290px list stretched across half the window.
        list_scroller.set_hexpand(False)
        split.append(list_scroller)
        split.append(Gtk.Separator(orientation=Gtk.Orientation.VERTICAL))

        # Conversation
        conversation = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        conversation.set_hexpand(True)
        if not self.active_account_id:
            conversation.append(w.empty_state(
                "Pick a conversation",
                "Choose someone on the left, or message a friend.",
                "mail-unread-symbolic",
            ))
            split.append(conversation)
            return split

        title = Gtk.Label(label=self.active_name or self.active_account_id, xalign=0)
        title.add_css_class("title-3")
        title.set_margin_top(10)
        title.set_margin_start(12)
        conversation.append(title)

        self._transcript = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        self._transcript.set_margin_start(12)
        self._transcript.set_margin_end(12)
        self._transcript.set_margin_bottom(8)
        self._transcript_scroller = w.scrolled(self._transcript)
        self._transcript_scroller.set_vexpand(True)
        conversation.append(self._transcript_scroller)

        composer = Gtk.Box(spacing=8)
        composer.set_margin_start(12)
        composer.set_margin_end(12)
        composer.set_margin_bottom(12)
        self._composer_entry = Gtk.Entry(placeholder_text="Message…")
        self._composer_entry.set_hexpand(True)
        self._composer_entry.connect("activate", lambda _e: self._send())
        send = Gtk.Button(icon_name="mail-send-symbolic")
        send.add_css_class("suggested-action")
        send.set_tooltip_text("Send")
        send.connect("clicked", lambda _b: self._send())
        composer.append(self._composer_entry)
        composer.append(send)
        conversation.append(composer)

        split.append(conversation)
        self._fill_transcript()
        return split

    def _thread_row(self, thread: Any) -> Gtk.Widget:
        account_id = str(fields(thread, "accountId", "otherAccountId", "id", default=""))
        name = _person_name(thread)
        unread = _as_int(fields(thread, "unread", "unreadCount", default=0))

        row = Gtk.Box(spacing=8)
        row.append(w.avatar(_avatar_of(thread), name, size=32))
        label_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        label_box.set_hexpand(True)
        name_label = Gtk.Label(label=name, xalign=0)
        name_label.add_css_class("heading")
        label_box.append(name_label)
        preview = fields(thread, "lastMessage", "preview", "body", default="")
        if preview:
            label_box.append(w.dim(w.cell_text(preview), wrap=False))
        row.append(label_box)
        if unread:
            badge = Gtk.Label(label=str(unread))
            badge.add_css_class("swarm-badge")
            row.append(badge)

        button = Gtk.Button()
        button.set_child(row)
        button.add_css_class("flat")
        if account_id and account_id == self.active_account_id:
            button.add_css_class("swarm-nav-active")
        button.connect("clicked",
                       lambda _b, a=account_id, n=name: self.open_thread(a, n))
        return button

    def _fill_transcript(self) -> None:
        transcript = getattr(self, "_transcript", None)
        if transcript is None:
            return
        child = transcript.get_first_child()
        while child is not None:
            nxt = child.get_next_sibling()
            transcript.remove(child)
            child = nxt

        if not self.messages:
            transcript.append(w.dim("No messages yet. Say hello."))
            return
        for message in self.messages:
            transcript.append(self._bubble(message))
        # Jump to the newest message, the way a chat view should open.
        adjustment = self._transcript_scroller.get_vadjustment()
        if adjustment is not None:
            from gi.repository import GLib

            GLib.idle_add(
                lambda: adjustment.set_value(
                    adjustment.get_upper() - adjustment.get_page_size()
                ) or GLib.SOURCE_REMOVE
            )

    def _bubble(self, message: Any) -> Gtk.Widget:
        mine = bool(fields(message, "mine", "fromMe", "isMine", default=False))
        body = str(fields(message, "body", "message", "text", default=""))
        sent = fields(message, "createdAt", "sentAt", default=None)

        bubble = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        bubble.add_css_class("card")
        bubble.set_margin_start(60 if mine else 0)
        bubble.set_margin_end(0 if mine else 60)
        text = Gtk.Label(label=body or w.DASH, xalign=0)
        text.set_wrap(True)
        text.set_selectable(True)
        text.set_margin_top(8)
        text.set_margin_bottom(4)
        text.set_margin_start(10)
        text.set_margin_end(10)
        bubble.append(text)
        meta = w.dim(
            " · ".join(x for x in (
                "You" if mine else _person_name(message),
                w.format_relative(sent) if sent else "",
            ) if x),
            wrap=False,
        )
        meta.set_margin_start(10)
        meta.set_margin_bottom(8)
        bubble.append(meta)
        return bubble

    def _send(self) -> None:
        entry = getattr(self, "_composer_entry", None)
        if entry is None or not self.active_account_id:
            return
        body = entry.get_text().strip()
        if not body:
            return
        entry.set_text("")

        def sent(result: Any) -> None:
            # Append straight away rather than waiting on the next
            # thread_messages broadcast, so sending feels immediate; the
            # feed replaces this list on its next tick anyway.
            message = field(result, "message")
            if message:
                self.messages = list(self.messages) + [message]
                self._fill_transcript()

        def failed(message: str) -> None:
            # Put the text back so a rejected message isn't simply lost.
            entry.set_text(body)
            w.toast(self.window, message)

        work.run(
            lambda: api.post(f"/api/messages/{self.active_account_id}",
                             {"body": body}),
            on_success=sent,
            on_error=failed,
        )


def _as_int(value: Any) -> int:
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return 0
