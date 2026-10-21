"""Fleet section -- Dashboard, Controls, Invites.

Ported from Features/Dashboard and Features/Controls. The live `dashboard`
and `control_state` watch keys drive these screens; REST reads of the same
endpoints are the first paint and the fallback while the socket is down.
"""

from __future__ import annotations

from typing import Any

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gtk, Pango

from ...core import images, labels
from ...core.api import api
from ...core.casing import field, fields
from ...core.config import settings
from ...core.live import live
from .. import widgets as w
from .base import Screen

# Control verbs, matching ControlAction in ControlsModels.swift. RESTART is
# intentionally excluded -- it is owner/admin-only on the web panel too.
ACTIONS = [
    "PLAY", "SMART_RECOMMEND", "PAUSE", "RESUME", "SKIP", "STOP", "CLEAR",
    "RESET_QUEUE", "SHUFFLE", "LOOP", "FILTER", "LEAVE", "SET_HOME", "RECOVER",
]
NEEDS_VOICE_CHANNEL = {"PLAY", "SET_HOME", "SMART_RECOMMEND"}
NEEDS_SOURCE_URL = {"PLAY"}
LOOP_MODES = ["off", "song", "queue"]
FILTER_MODES = [
    "none", "nightcore", "bassboost", "vaporwave", "8d", "karaoke",
    "tremolo", "vibrato", "lowpass", "lofi", "electronic", "party",
    "radio", "cinema",
]
# Discord channel type codes, matching ControlsPage.jsx's filters.
VOICE_CHANNEL_TYPES = {2, 13}


def action_label(action: str) -> str:
    return action.replace("_", " ").title()


# -- Dashboard --------------------------------------------------------------


class DashboardScreen(Screen):
    """Live fleet status.

    Presented as cards rather than a flat list, matching the web panel: each
    bot gets a header row with its health, and each active session a card
    with artwork, a progress bar and transport controls.

    Bot rows use `self.expander(...)` so an opened bot stays open: the
    dashboard snapshot arrives every couple of seconds (the track position
    moves), and a plain ExpanderRow would collapse on every one of them.
    """

    watch_key = "dashboard"
    fallback_interval = 10.0

    def load(self) -> Any:
        return api.get("/api/dashboard")

    def render(self, payload: Any) -> Gtk.Widget:
        bots = field(payload, "bots", []) or []
        sessions = field(payload, "sessions", []) or []
        # The top-level `sessions` list is the richest one in a REST payload;
        # harvest its names before anything renders from the leaner nested
        # copies the socket sends.
        for session in sessions:
            labels.remember_session(session)
        box = self.body()

        playing = sum(1 for x in sessions if field(x, "isPlaying"))
        paused = sum(1 for x in sessions if field(x, "isPaused"))
        queued = sum(_as_int(field(x, "queueCount", 0)) for x in sessions)
        unhealthy = [
            b for b in bots
            if w.status_css(field(b, "heartbeatStatus") or field(b, "status")) == "swarm-bad"
        ]
        box.append(w.stat_row([
            w.stat_tile("Bots", w.format_count(len(bots))),
            w.stat_tile("Playing", w.format_count(playing)),
            w.stat_tile("Paused", w.format_count(paused)),
            w.stat_tile("Queued", w.format_count(queued)),
            w.stat_tile("Unhealthy", w.format_count(len(unhealthy)),
                        "needs attention" if unhealthy else "all nominal"),
        ]))

        if not bots:
            box.append(w.empty_state(
                "No bots reporting",
                "The fleet hasn't sent any telemetry yet.",
                "network-offline-symbolic",
            ))
            return w.clamp_of(box)

        # Everything currently playing or paused is shown as a card up
        # front. Leaving these behind a collapsed row meant a fleet with a
        # dozen live sessions presented as a near-empty page -- the operator
        # had to expand a bot to discover anything was playing at all.
        own_guild = settings.last_known_guild_id
        live: list[tuple[Any, Any]] = []
        for bot in bots:
            for session in (field(bot, "sessions", []) or []):
                if field(session, "isPlaying") or field(session, "isPaused"):
                    live.append((bot, session))

        def live_order(pair):
            bot, session = pair
            own = str(field(session, "guildId", "")) != own_guild
            return (own, not field(session, "isPlaying"),
                    str(field(bot, "displayName") or field(bot, "key", "")).lower())

        live.sort(key=live_order)
        if live:
            shown = live[:12]
            group = w.section(
                "Now playing",
                f"{len(live)} live session(s)" +
                (f" — showing the first {len(shown)}" if len(live) > len(shown) else ""))
            for bot, session in shown:
                row = Adw.ActionRow()
                row.set_activatable(False)
                row.set_child(self._session_card(
                    str(field(bot, "key", "")), bot, session,
                    big=str(field(session, "guildId", "")) == own_guild))
                group.add(row)
            box.append(group)

        pinned = set(settings.pinned_bots)

        def sort_key(bot):
            key = str(field(bot, "key", ""))
            active = any(
                field(x, "isPlaying") or field(x, "isPaused")
                for x in (field(bot, "sessions", []) or [])
            )
            return (key not in pinned, not active,
                    str(field(bot, "displayName") or key).lower())

        idle = len(bots) - len({id(b) for b, _ in live})
        group = w.section(
            "Fleet",
            f"{len(bots)} node(s) reporting" + (f" · {idle} idle" if idle > 0 else ""))
        for bot in sorted(bots, key=sort_key):
            group.add(self._bot_row(bot, pinned))
        box.append(group)
        return w.clamp_of(box)

    # -- rows and cards ----------------------------------------------------

    def _bot_row(self, bot: Any, pinned: set[str]) -> Gtk.Widget:
        key = str(field(bot, "key", ""))
        name = str(field(bot, "displayName") or key)
        sessions = field(bot, "sessions", []) or []
        status = field(bot, "status")
        heartbeat = field(bot, "heartbeatStatus")

        bits = [
            f"{_as_int(field(bot, 'activePlayingCount', 0))} playing",
            f"{_as_int(field(bot, 'knownGuildCount', 0))} guild(s)",
            f"queue {_as_int(field(bot, 'queueDepth', 0))}",
        ]
        backup = _as_int(field(bot, "backupQueueDepth", 0))
        if backup:
            bits.append(f"backup {backup}")
        kind = field(bot, "kind")
        if kind:
            bits.insert(0, str(kind))

        # Keyed by bot, so the row the user opened stays open across the
        # next snapshot.
        row = self.expander(f"bot:{key}", name, " · ".join(bits))

        if status:
            row.add_suffix(w.pill(str(status), css=w.status_css(status)))
        if heartbeat and heartbeat != status:
            row.add_suffix(w.pill(f"♥ {heartbeat}", css=w.status_css(heartbeat)))

        pin = Gtk.ToggleButton(icon_name="view-pin-symbolic")
        pin.set_valign(Gtk.Align.CENTER)
        pin.add_css_class("flat")
        pin.set_active(key in pinned)
        pin.set_tooltip_text("Pin to the top of the dashboard")
        pin.connect("toggled", lambda button, k=key: self._toggle_pin(k, button.get_active()))
        row.add_suffix(pin)

        details = Gtk.Button(icon_name="go-next-symbolic")
        details.set_valign(Gtk.Align.CENTER)
        details.add_css_class("flat")
        details.set_tooltip_text("Bot details")
        details.connect("clicked", lambda _b, b=bot: self._open_detail(b))
        row.add_suffix(details)

        if not sessions:
            row.add_row(Adw.ActionRow(
                title="No active sessions",
                subtitle="This bot isn't in a voice channel."))
            return row
        for session in sessions:
            holder = Adw.ActionRow()
            holder.set_activatable(False)
            holder.set_child(self._session_card(key, bot, session))
            row.add_row(holder)
        return row

    def _session_card(self, bot_key: str, bot: Any, session: Any,
                      *, big: bool = False) -> Gtk.Widget:
        """Port of NowPlayingCard: artwork, title, progress, transport."""
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        card.set_margin_top(8)
        card.set_margin_bottom(8)
        card.set_margin_start(4)
        card.set_margin_end(4)
        if big:
            card.add_css_class("card")
            card.set_margin_top(4)
            for side in ("set_margin_start", "set_margin_end"):
                getattr(card, side)(12)
            card.set_margin_bottom(12)
            card.set_margin_top(12)

        top = Gtk.Box(spacing=12)
        # The dashboard payload precomputes `thumbnail`; fall back to
        # deriving it from the video URL the way control-state has to.
        art = field(session, "thumbnail") or images.youtube_thumbnail(
            field(session, "videoUrl"))
        top.append(w.artwork(art, size=72 if big else 56))

        text = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        text.set_hexpand(True)
        text.set_valign(Gtk.Align.CENTER)

        if big:
            who = Gtk.Label(label=str(field(bot, "displayName") or bot_key), xalign=0)
            who.add_css_class("caption")
            who.add_css_class("dim-label")
            text.append(who)

        title = Gtk.Label(label=w.text_or_dash(field(session, "title")), xalign=0)
        title.add_css_class("title-4" if big else "heading")
        title.set_ellipsize(Pango.EllipsizeMode.END)
        text.append(title)

        # Resolved through the name cache: the /ws dashboard snapshot sends
        # only IDs, so without this the card falls back to raw snowflakes a
        # couple of seconds after the REST payload paints.
        labels.remember_session(session)
        guild = labels.guild_label(session)
        channel = labels.channel_label(session)
        where = Gtk.Label(label=f"{guild or w.DASH} · #{channel or w.DASH}", xalign=0)
        where.add_css_class("caption")
        where.add_css_class("dim-label")
        where.set_ellipsize(Pango.EllipsizeMode.END)
        text.append(where)
        top.append(text)

        state = field(session, "sessionStateLabel") or field(session, "sessionState")
        if state:
            top.append(w.pill(str(state), css=w.status_css(
                "playing" if field(session, "isPlaying") else
                "paused" if field(session, "isPaused") else state)))
        card.append(top)

        position = field(session, "positionSeconds")
        duration = field(session, "durationSeconds")
        bar = w.progress(position, duration)
        if bar is not None:
            timeline = Gtk.Box(spacing=8)
            elapsed = Gtk.Label(label=w.format_duration(position))
            elapsed.add_css_class("numeric")
            elapsed.add_css_class("caption")
            total = Gtk.Label(label=w.format_duration(duration))
            total.add_css_class("numeric")
            total.add_css_class("caption")
            timeline.append(elapsed)
            timeline.append(bar)
            timeline.append(total)
            card.append(timeline)

        meta = []
        queue_count = _as_int(field(session, "queueCount", 0))
        if queue_count:
            meta.append(f"{queue_count} queued")
        backup = _as_int(field(session, "backupQueueCount", 0))
        if backup:
            meta.append(f"{backup} backup")
        source = field(session, "mediaSourceLabel")
        if source:
            meta.append(str(source))
        if field(session, "cached"):
            meta.append("cached")

        bottom = Gtk.Box(spacing=6)
        if meta:
            label = Gtk.Label(label=" · ".join(meta), xalign=0)
            label.add_css_class("caption")
            label.add_css_class("dim-label")
            label.set_hexpand(True)
            bottom.append(label)
        else:
            bottom.append(Gtk.Box(hexpand=True))

        guild_id = field(session, "guildId")
        if guild_id:
            for icon, action, tip in (
                ("media-playback-pause-symbolic", "PAUSE", "Pause"),
                ("media-playback-start-symbolic", "RESUME", "Resume"),
                ("media-skip-forward-symbolic", "SKIP", "Skip"),
            ):
                button = Gtk.Button(icon_name=icon)
                button.add_css_class("flat")
                button.set_tooltip_text(tip)
                button.connect(
                    "clicked",
                    lambda _b, a=action, g=str(guild_id): self._send(bot_key, g, a))
                bottom.append(button)

        url = field(session, "videoUrl")
        if url:
            link = Gtk.Button(icon_name="adw-external-link-symbolic")
            link.add_css_class("flat")
            link.set_tooltip_text(str(url))
            link.connect("clicked", lambda _b, u=str(url): Gtk.UriLauncher(uri=u).launch())
            bottom.append(link)
        card.append(bottom)
        return card

    # -- actions -----------------------------------------------------------

    def _open_detail(self, bot: Any) -> None:
        BotDetailDialog(self, bot).present(self.get_root())

    def _toggle_pin(self, key: str, pinned: bool) -> None:
        current = settings.pinned_bots
        if pinned and key not in current:
            current.append(key)
        elif not pinned and key in current:
            current.remove(key)
        settings.pinned_bots = current

    def _send(self, bot_key: str, guild_id: str, action: str) -> None:
        self.act(
            lambda: api.post("/api/bots/control", {
                "botKey": bot_key, "guildId": guild_id,
                "action": action, "payload": {},
            }),
            success=f"{action_label(action)} sent.",
            then=lambda _r: None,  # the live feed reflects it within a tick
        )


class BotDetailDialog(Adw.Dialog):
    """Port of BotDetailView -- everything the payload knows about one bot,
    plus its sessions, in a dialog rather than a pushed screen."""

    def __init__(self, screen: Screen, bot: Any) -> None:
        super().__init__()
        key = str(field(bot, "key", ""))
        name = str(field(bot, "displayName") or key)
        self.set_title(name)
        self.set_content_width(620)
        self.set_content_height(680)

        box = w.page_body()
        box.append(w.heading(name, level=2))

        pills = Gtk.Box(spacing=6)
        for value in (field(bot, "status"), field(bot, "heartbeatStatus")):
            if value:
                pills.append(w.pill(str(value), css=w.status_css(value)))
        if field(bot, "kind"):
            pills.append(w.pill(str(field(bot, "kind"))))
        box.append(pills)

        box.append(w.stat_row([
            w.stat_tile("Playing", w.format_count(field(bot, "activePlayingCount", 0))),
            w.stat_tile("Guilds", w.format_count(field(bot, "knownGuildCount", 0))),
            w.stat_tile("Queue", w.format_count(field(bot, "queueDepth", 0))),
            w.stat_tile("Backup", w.format_count(field(bot, "backupQueueDepth", 0))),
        ]))

        group = w.section("Details")
        group.add(w.key_value("Key", key, copyable=True))
        for title, name_key in (("Kind", "kind"), ("Status", "status"),
                                ("Heartbeat", "heartbeatStatus")):
            value = field(bot, name_key)
            if value:
                group.add(w.key_value(title, value))
        box.append(group)

        sessions = field(bot, "sessions", []) or []
        sessions_group = w.section(
            "Sessions", f"{len(sessions)} active" if sessions else "None active")
        if not sessions:
            sessions_group.add(Adw.ActionRow(
                title="No active sessions",
                subtitle="This bot isn't in a voice channel."))
        for session in sessions:
            row = Adw.ActionRow()
            row.set_activatable(False)
            row.set_child(screen._session_card(key, bot, session))
            sessions_group.add(row)
        box.append(sessions_group)

        header = Adw.HeaderBar()
        view = Adw.ToolbarView()
        view.add_top_bar(header)
        view.set_content(w.scrolled(w.clamp_of(box)))
        self.set_child(view)


# -- Controls ---------------------------------------------------------------


class ControlsScreen(Screen):
    """Full control surface for one bot in one guild, plus a guild-wide
    overview matrix and the voice/stage channel conversion tool."""

    watch_key = "control_state"
    fallback_interval = 8.0
    scrollable = True

    def __init__(self, window: Any, **kwargs) -> None:
        self.bots: list[Any] = []
        self.bot_key: str = ""
        self.guild_id: str = settings.last_known_guild_id
        self.voice_channel_id: str = ""
        self.loop_mode = "off"
        self.filter_mode = "none"
        self.control_state: Any = None
        self.saved_queues: list[Any] = []
        self.inventory: list[Any] = []
        self.overview: list[Any] = []
        super().__init__(window, **kwargs)

    # -- data

    def watch_params(self) -> dict[str, str] | None:
        if not (self.bot_key and self.guild_id):
            return None
        return {"bot_key": self.bot_key, "guild_id": self.guild_id}

    def load(self) -> Any:
        roster = api.get("/api/bots")
        bots = field(roster, "bots", []) or []
        bot_key = self.bot_key or (str(field(bots[0], "key", "")) if bots else "")
        guild_id = self.guild_id

        inventory: list[Any] = []
        if bot_key:
            try:
                inventory = field(
                    api.get(f"/api/bots/{bot_key}/inventory"), "guilds", []
                ) or []
                # Names for every guild and channel this bot can see -- the
                # dashboard reuses these when the socket feed sends only IDs.
                labels.remember_inventory(inventory)
            except Exception:
                # An inventory failure must not block the whole screen --
                # the operator can still type a guild ID by hand.
                inventory = []
        if not guild_id and inventory:
            guild_id = str(field(inventory[0], "id", ""))

        state = None
        queues: list[Any] = []
        overview: list[Any] = []
        if bot_key and guild_id:
            state = field(
                api.get(f"/api/bots/{bot_key}/control-state",
                        {"guild_id": guild_id}),
                "session",
            )
            try:
                queues = field(
                    api.get("/api/queues", {"guild_id": guild_id, "bot_key": bot_key}),
                    "queues", [],
                ) or []
            except Exception:
                queues = []
            try:
                overview = field(
                    api.get(f"/api/guilds/{guild_id}/control-matrix"), "bots", []
                ) or []
            except Exception:
                overview = []
        return {
            "bots": bots, "botKey": bot_key, "guildId": guild_id,
            "inventory": inventory, "session": state,
            "queues": queues, "overview": overview,
        }

    def apply(self, payload: Any) -> None:
        if isinstance(payload, dict) and "bots" in payload:
            self.bots = payload["bots"]
            self.bot_key = payload["botKey"]
            self.guild_id = payload["guildId"]
            self.inventory = payload["inventory"]
            self.control_state = payload["session"]
            self.saved_queues = payload["queues"]
            self.overview = payload["overview"]
            if self.guild_id:
                settings.last_known_guild_id = self.guild_id
            self._adopt_state_defaults()
            # Point the live watch at whatever bot/guild we just resolved.
            if self.watch_key and self.watch_params():
                live.resubscribe(self.watch_key, self.watch_params())
        super().apply(payload)

    def on_live_snapshot(self, payload: Any) -> None:
        """A control_state snapshot refreshes only the session half; the
        roster/inventory/queues come from REST and must not be dropped."""
        self.control_state = field(payload, "session", payload)
        self._adopt_state_defaults()
        super().apply(self.payload)

    def _adopt_state_defaults(self) -> None:
        state = self.control_state
        if not state:
            return
        # Fill-if-empty, matching the web panel: never clobber a channel the
        # operator is mid-picking for a PLAY/SET_HOME order.
        if not self.voice_channel_id:
            home = field(state, "homeChannelId") or field(state, "channelId")
            if home:
                self.voice_channel_id = str(home)
        loop = field(state, "loopMode")
        if loop:
            self.loop_mode = str(loop)
        filter_mode = field(state, "filterMode")
        if filter_mode:
            self.filter_mode = str(filter_mode)

    # -- rendering

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()
        box.append(self._target_group())
        if not (self.bot_key and self.guild_id):
            box.append(w.empty_state(
                "Pick a bot and guild",
                "Choose a bot above, then a guild it's in, to send orders.",
                "media-playback-start-symbolic",
            ))
            return w.clamp_of(box)
        box.append(self._now_playing_group())
        box.append(self._action_group())
        box.append(self._queue_group())
        box.append(self._overview_group())
        box.append(self._convert_group())
        return w.clamp_of(box)

    def _target_group(self) -> Gtk.Widget:
        group = w.section("Target", "Which bot, in which guild.")

        bot_row = Adw.ComboRow(title="Bot")
        model = Gtk.StringList()
        keys = []
        for bot in self.bots:
            key = str(field(bot, "key", ""))
            keys.append(key)
            model.append(str(field(bot, "displayName") or key))
        bot_row.set_model(model)
        if self.bot_key in keys:
            bot_row.set_selected(keys.index(self.bot_key))
        bot_row.connect("notify::selected", lambda row, _p: self._choose_bot(keys, row.get_selected()))
        group.add(bot_row)

        if self.inventory:
            guild_row = Adw.ComboRow(title="Guild")
            guild_model = Gtk.StringList()
            guild_ids = []
            for guild in self.inventory:
                gid = str(field(guild, "id", ""))
                guild_ids.append(gid)
                guild_model.append(f"{field(guild, 'name') or gid}")
            guild_row.set_model(guild_model)
            if self.guild_id in guild_ids:
                guild_row.set_selected(guild_ids.index(self.guild_id))
            guild_row.connect("notify::selected",
                              lambda row, _p: self._choose_guild(guild_ids, row.get_selected()))
            group.add(guild_row)
        else:
            manual = Adw.EntryRow(title="Guild ID")
            manual.set_text(self.guild_id)
            manual.connect("apply", lambda row: self._choose_guild_id(row.get_text().strip()))
            manual.set_show_apply_button(True)
            group.add(manual)

        channels = self._voice_channels()
        if channels:
            channel_row = Adw.ComboRow(title="Voice channel")
            channel_row.set_subtitle("Used by Play, Smart Recommend and Set Home.")
            channel_model = Gtk.StringList()
            channel_ids = []
            for channel in channels:
                cid = str(field(channel, "id", ""))
                channel_ids.append(cid)
                channel_model.append(str(field(channel, "name") or cid))
            channel_row.set_model(channel_model)
            if self.voice_channel_id in channel_ids:
                channel_row.set_selected(channel_ids.index(self.voice_channel_id))
            channel_row.connect(
                "notify::selected",
                lambda row, _p: setattr(
                    self, "voice_channel_id",
                    channel_ids[row.get_selected()] if 0 <= row.get_selected() < len(channel_ids) else "",
                ),
            )
            group.add(channel_row)
        else:
            channel_entry = Adw.EntryRow(title="Voice channel ID")
            channel_entry.set_text(self.voice_channel_id)
            channel_entry.set_show_apply_button(True)
            channel_entry.connect(
                "apply",
                lambda row: setattr(self, "voice_channel_id", row.get_text().strip()),
            )
            group.add(channel_entry)
        return group

    def _voice_channels(self) -> list[Any]:
        for guild in self.inventory:
            if str(field(guild, "id", "")) == self.guild_id:
                channels = field(guild, "channels", []) or []
                return [c for c in channels
                        if _as_int(field(c, "type", -1)) in VOICE_CHANNEL_TYPES]
        return []

    def _choose_bot(self, keys: list[str], index: int) -> None:
        if not (0 <= index < len(keys)) or keys[index] == self.bot_key:
            return
        self.bot_key = keys[index]
        self.voice_channel_id = ""
        recents = settings.recent_bots
        settings.recent_bots = [self.bot_key] + [k for k in recents if k != self.bot_key]
        self.refresh(show_spinner=True)

    def _choose_guild(self, guild_ids: list[str], index: int) -> None:
        if 0 <= index < len(guild_ids):
            self._choose_guild_id(guild_ids[index])

    def _choose_guild_id(self, guild_id: str) -> None:
        if not guild_id or guild_id == self.guild_id:
            return
        self.guild_id = guild_id
        self.voice_channel_id = ""
        settings.last_known_guild_id = guild_id
        self.refresh(show_spinner=True)

    def _now_playing_group(self) -> Gtk.Widget:
        state = self.control_state
        group = w.section("Now playing")
        if not state:
            group.add(Adw.ActionRow(
                title="No active session",
                subtitle="This bot isn't playing in this guild right now.",
            ))
            return group

        row = Adw.ActionRow(title=w.text_or_dash(field(state, "title")))
        # Prefer the server's own thumbnail when it sends one, and fall back
        # to deriving it from the video URL (which is all the iOS client had).
        row.add_prefix(w.artwork(
            field(state, "thumbnail") or images.youtube_thumbnail(field(state, "videoUrl")),
            size=48))
        bar = w.progress(field(state, "positionSeconds"),
                         field(state, "durationSeconds"))
        if bar is not None:
            bar.set_size_request(140, -1)
            row.add_suffix(bar)
        bits = [
            f"{w.format_duration(field(state, 'positionSeconds'))} / "
            f"{w.format_duration(field(state, 'durationSeconds'))}",
            f"queue {_as_int(field(state, 'queueCount', 0))}",
        ]
        backup = _as_int(field(state, "backupQueueCount", 0))
        if backup:
            bits.append(f"backup {backup}")
        volume = field(state, "volume")
        if volume is not None:
            bits.append(f"vol {volume}")
        bits.append(f"loop {field(state, 'loopMode') or 'off'}")
        bits.append(f"filter {field(state, 'filterMode') or 'none'}")
        row.set_subtitle(" · ".join(bits))
        row.set_subtitle_lines(2)
        label = field(state, "sessionStateLabel")
        if label:
            row.add_suffix(w.pill(str(label), css=w.status_css(
                "playing" if field(state, "isPlaying") else
                "paused" if field(state, "isPaused") else label
            )))
        group.add(row)

        # Surface queued "direct orders" -- otherwise "did my order actually
        # go through?" is invisible, since LEAVE/SEEK/RECOVER/RESTART are
        # picked up by the bot on a poll rather than applied instantly.
        pending = _as_int(field(state, "pendingDirectOrders", 0))
        latest = field(state, "latestDirectOrder")
        if pending or latest:
            order_row = Adw.ActionRow(title="Direct orders")
            detail = []
            if pending:
                detail.append(f"{pending} waiting to be picked up")
            if latest:
                command = field(latest, "command")
                if command:
                    detail.append(f"latest: {command}")
            order_row.set_subtitle(" · ".join(detail) or w.DASH)
            if pending:
                order_row.add_suffix(w.pill("pending", css="swarm-warn"))
            group.add(order_row)

        preview = field(state, "queuePreview", []) or []
        if preview:
            expander = Adw.ExpanderRow(title=f"Queue preview ({len(preview)})")
            for index, item in enumerate(preview[:25], start=1):
                expander.add_row(Adw.ActionRow(
                    title=f"{index}. {field(item, 'title') or field(item, 'videoUrl') or w.DASH}",
                    subtitle=str(field(item, "videoUrl", "")),
                ))
            group.add(expander)
        return group

    def _action_group(self) -> Gtk.Widget:
        group = w.section("Send an order")

        self._source_entry = Adw.EntryRow(title="Source URL or search")
        group.add(self._source_entry)

        loop_row = Adw.ComboRow(title="Loop mode")
        loop_model = Gtk.StringList()
        for mode in LOOP_MODES:
            loop_model.append(mode)
        loop_row.set_model(loop_model)
        loop_row.set_selected(LOOP_MODES.index(self.loop_mode)
                              if self.loop_mode in LOOP_MODES else 0)
        loop_row.connect("notify::selected",
                         lambda row, _p: setattr(self, "loop_mode", LOOP_MODES[row.get_selected()]))
        group.add(loop_row)

        filter_row = Adw.ComboRow(title="Filter")
        filter_model = Gtk.StringList()
        for mode in FILTER_MODES:
            filter_model.append(mode)
        filter_row.set_model(filter_model)
        filter_row.set_selected(FILTER_MODES.index(self.filter_mode)
                                if self.filter_mode in FILTER_MODES else 0)
        filter_row.connect("notify::selected",
                           lambda row, _p: setattr(self, "filter_mode", FILTER_MODES[row.get_selected()]))
        group.add(filter_row)

        flow = Gtk.FlowBox()
        flow.set_selection_mode(Gtk.SelectionMode.NONE)
        flow.set_column_spacing(8)
        flow.set_row_spacing(8)
        flow.set_min_children_per_line(2)
        flow.set_max_children_per_line(4)
        flow.set_margin_top(6)
        for action in ACTIONS:
            button = Gtk.Button(label=action_label(action))
            button.add_css_class("pill")
            if action in ("STOP", "CLEAR", "RESET_QUEUE", "LEAVE"):
                button.add_css_class("destructive-action")
            elif action in ("PLAY", "RESUME"):
                button.add_css_class("suggested-action")
            button.connect("clicked", lambda _b, a=action: self._send_action(a))
            flow.append(button)
        group.add(flow)
        return group

    def _send_action(self, action: str) -> None:
        # Payload keys are exactly what control.lua reads -- it errors with
        # "PLAY payload must be an object with source_url and
        # voice_channel_id", so these names are not interchangeable.
        payload: dict[str, str] = {}
        if action in NEEDS_SOURCE_URL:
            source = self._source_entry.get_text().strip()
            if not source:
                w.toast(self.window, "Enter a source URL or search text to play.")
                return
            payload["source_url"] = source
        if action == "SMART_RECOMMEND":
            # Optional free-text steer for the recommender; the action works
            # with just a voice channel.
            query = self._source_entry.get_text().strip()
            if query:
                payload["query"] = query
        if action in NEEDS_VOICE_CHANNEL:
            # PLAY, SMART_RECOMMEND and SET_HOME all require a channel
            # server-side, so catching a blank one here saves a round trip
            # to learn the same thing.
            if not self.voice_channel_id:
                w.toast(self.window,
                        f"Pick a voice channel before sending {action_label(action)}.")
                return
            payload["voice_channel_id"] = self.voice_channel_id
        if action == "LOOP":
            payload["loop_mode"] = self.loop_mode
        if action == "FILTER":
            payload["filter_mode"] = self.filter_mode

        def fire() -> Any:
            return api.post("/api/bots/control", {
                "botKey": self.bot_key, "guildId": self.guild_id,
                "action": action, "payload": payload,
            })

        if action in ("STOP", "CLEAR", "RESET_QUEUE", "LEAVE"):
            w.confirm(
                self, f"{action_label(action)}?",
                f"Send {action} to {self.bot_key} in guild {self.guild_id}.",
                action_label(action),
                lambda: self.act(fire, success=f"{action_label(action)} sent."),
            )
            return
        self.act(fire, success=f"{action_label(action)} sent.")

    def _queue_group(self) -> Gtk.Widget:
        group = w.section("Saved queues", "Snapshots of a live queue you can reload.")
        save = Adw.ActionRow(title="Save the current queue")
        preview = field(self.control_state, "queuePreview", []) or []
        save.set_subtitle(f"{len(preview)} track(s) in the live queue"
                          if preview else "The live queue is empty — nothing to save.")
        button = Gtk.Button(label="Save…")
        button.add_css_class("pill")
        button.set_valign(Gtk.Align.CENTER)
        button.set_sensitive(bool(preview))
        button.connect("clicked", lambda _b: self._save_queue(preview))
        save.add_suffix(button)
        group.add(save)

        if not self.saved_queues:
            group.add(Adw.ActionRow(title="No saved queues yet"))
            return group
        for queue in self.saved_queues:
            queue_id = field(queue, "id")
            name = str(field(queue, "name", "Saved Queue"))
            items = field(queue, "items", []) or []
            row = Adw.ActionRow(title=name, subtitle=f"{len(items)} track(s)")
            for label, handler, css in (
                ("Load", lambda _b, q=queue: self._load_queue(q), "suggested-action"),
                ("Rename", lambda _b, q=queue_id, n=name: self._rename_queue(q, n), ""),
                ("Delete", lambda _b, q=queue_id, n=name: self._delete_queue(q, n), "destructive-action"),
            ):
                action_button = Gtk.Button(label=label)
                action_button.add_css_class("flat")
                if css:
                    action_button.add_css_class(css)
                action_button.set_valign(Gtk.Align.CENTER)
                action_button.connect("clicked", handler)
                row.add_suffix(action_button)
            group.add(row)
        return group

    def _save_queue(self, preview: list[Any]) -> None:
        if not preview:
            w.toast(self.window, "The live queue is empty — nothing to save.")
            return
        items = [
            {"videoUrl": str(field(item, "videoUrl", "")),
             "title": field(item, "title")}
            for item in preview if field(item, "videoUrl")
        ]
        w.prompt(
            self, "Save queue", "Name this snapshot of the live queue.",
            "Saved Queue", "Save",
            lambda name: self.act(
                lambda: api.post("/api/queues", {
                    "guildId": self.guild_id, "botKey": self.bot_key,
                    "name": name, "items": items,
                }),
                success="Queue saved.",
            ),
        )

    def _load_queue(self, queue: Any) -> None:
        """Replay a saved queue as a series of PLAY orders.

        There is no "load queue" endpoint -- the panel and the iOS client
        both just send one PLAY per saved item, so this does the same rather
        than calling a route that does not exist.
        """
        items = [str(field(item, "videoUrl", "")) for item in
                 (field(queue, "items", []) or [])]
        items = [url for url in items if url]
        name = str(field(queue, "name", "Saved Queue"))
        if not items:
            w.toast(self.window, f"“{name}” has no tracks.")
            return
        if not self.voice_channel_id:
            w.toast(self.window, "Select a voice channel before loading a saved queue.")
            return

        def run() -> Any:
            for url in items:
                api.post("/api/bots/control", {
                    "botKey": self.bot_key, "guildId": self.guild_id,
                    "action": "PLAY",
                    "payload": {"source_url": url,
                                "voice_channel_id": self.voice_channel_id},
                })
            return len(items)

        self.act(run, success=f"Queued {len(items)} track(s) from “{name}”.")

    def _rename_queue(self, queue_id: Any, current: str) -> None:
        w.prompt(
            self, "Rename queue", "", current, "Rename",
            lambda name: self.act(
                lambda: api.post(f"/api/queues/{queue_id}/rename",
                                 {"guildId": self.guild_id, "name": name}),
                success="Queue renamed.",
            ),
            initial=current,
        )

    def _delete_queue(self, queue_id: Any, name: str) -> None:
        w.confirm(
            self, "Delete saved queue?", f"“{name}” will be removed.", "Delete",
            lambda: self.act(
                lambda: api.post(f"/api/queues/{queue_id}/delete",
                                 {"guildId": self.guild_id}),
                success="Queue deleted.",
            ),
        )

    def _overview_group(self) -> Gtk.Widget:
        group = w.section("Guild overview", f"Every bot's state in guild {self.guild_id}.")
        if not self.overview:
            group.add(Adw.ActionRow(title="No overview available"))
            return group
        for bot in self.overview:
            # A bot whose state couldn't be read comes back flat, with an
            # `error` instead of a nested session, so read both shapes.
            key = fields(bot, "key", "botKey", default="")
            label = fields(bot, "displayName", "botDisplay", default=key) or key
            error = field(bot, "error")
            session = field(bot, "session")
            row = Adw.ActionRow(title=str(label))
            if error:
                row.set_subtitle(str(error))
                row.add_suffix(w.pill("error", css="swarm-bad"))
                group.add(row)
                continue
            title = field(session, "title")
            channel = labels.channel_label(session)
            state = field(session, "sessionStateLabel")
            row.set_subtitle(" · ".join(x for x in (
                w.text_or_dash(title),
                f"#{channel}" if channel else "",
                f"queue {_as_int(field(session, 'queueCount', 0))}",
            ) if x))
            if state:
                row.add_suffix(w.pill(str(state), css=w.status_css(state)))
            for icon, action, tip in (
                ("media-playback-pause-symbolic", "PAUSE", "Pause"),
                ("media-playback-start-symbolic", "RESUME", "Resume"),
                ("media-skip-forward-symbolic", "SKIP", "Skip"),
            ):
                button = Gtk.Button(icon_name=icon)
                button.add_css_class("flat")
                button.set_valign(Gtk.Align.CENTER)
                button.set_tooltip_text(tip)
                button.connect("clicked",
                               lambda _b, k=str(key), a=action: self._overview_action(k, a))
                row.add_suffix(button)
            group.add(row)
        return group

    def _overview_action(self, bot_key: str, action: str) -> None:
        if not bot_key:
            return
        self.act(
            lambda: api.post("/api/bots/control", {
                "botKey": bot_key, "guildId": self.guild_id,
                "action": action, "payload": {},
            }),
            success=f"{action_label(action)} sent to {bot_key}.",
        )

    def _convert_group(self) -> Gtk.Widget:
        group = w.section(
            "Channel conversion",
            "Convert this guild's bot home channels between voice and stage.",
        )
        for direction in ("voice", "stage"):
            row = Adw.ActionRow(title=f"Convert to {direction}")
            button = Gtk.Button(label=f"Convert to {direction}")
            button.add_css_class("pill")
            button.set_valign(Gtk.Align.CENTER)
            button.connect("clicked", lambda _b, d=direction: self._convert(d))
            row.add_suffix(button)
            group.add(row)
        return group

    def _convert(self, direction: str) -> None:
        def run() -> Any:
            return api.post(f"/api/guilds/{self.guild_id}/convert-channels",
                            {"direction": direction})

        def done(result: Any) -> None:
            converted = field(result, "converted", []) or []
            warnings = [field(c, "warning") for c in converted if field(c, "warning")]
            failures = [field(f, "error") for f in (field(result, "failed", []) or [])
                        if field(f, "error")]
            skipped = _as_int(field(result, "skippedNoHomeChannel", 0))
            message = f"Converted {len(converted)} channel(s) to {direction}"
            if skipped:
                message += f" ({skipped} bot(s) had no home channel set)"
            if warnings:
                message += " — warnings: " + "; ".join(str(x) for x in warnings)
            if failures:
                message += f" — {len(failures)} failed: " + "; ".join(str(x) for x in failures)
            w.toast(self.window, message + ".", timeout=8)
            self.refresh(show_spinner=False)

        w.confirm(
            self, f"Convert to {direction}?",
            f"Every bot home channel in guild {self.guild_id} will be converted "
            f"to {direction}.",
            f"Convert to {direction}",
            lambda: self.act(run, then=done),
        )


# -- Invites ----------------------------------------------------------------


class InvitesScreen(Screen):
    fallback_interval = 0.0  # static data; the refresh button is enough

    def load(self) -> Any:
        return api.get("/api/bots")

    def render(self, payload: Any) -> Gtk.Widget:
        bots = field(payload, "inviteBots", []) or []
        box = self.body()
        if not bots:
            box.append(w.empty_state("No invite links",
                                     "No bots are configured for invites.",
                                     "insert-link-symbolic"))
            return w.clamp_of(box)

        group = w.section("Invite links", f"{len(bots)} bot(s)")
        for bot in bots:
            key = str(field(bot, "key", ""))
            label = str(field(bot, "displayName") or key)
            summary = field(bot, "capabilitySummary")
            identity = field(bot, "identityName")
            row = self.expander(
                f"invite:{key}", label,
                " · ".join(str(x) for x in (identity, summary) if x) or key)
            row.add_prefix(w.avatar(field(bot, "iconUrl"), label, size=32))

            if field(bot, "tokenConfigured") is False:
                row.add_suffix(w.pill("no token", css="swarm-bad"))
            if field(bot, "connectedToSessionGuild"):
                row.add_suffix(w.pill("in your guild", css="swarm-ok"))

            error = field(bot, "identityError")
            if error:
                row.add_row(Adw.ActionRow(title="Identity error", subtitle=str(error)))

            invite_url = field(bot, "inviteUrl")
            if invite_url:
                invite_row = Adw.ActionRow(title="Invite URL")
                invite_row.set_subtitle(str(invite_url))
                invite_row.set_subtitle_lines(2)
                open_button = Gtk.Button(icon_name="adw-external-link-symbolic")
                open_button.add_css_class("flat")
                open_button.set_valign(Gtk.Align.CENTER)
                open_button.set_tooltip_text("Open invite")
                open_button.connect(
                    "clicked",
                    lambda _b, u=str(invite_url): Gtk.UriLauncher(uri=u).launch(),
                )
                copy_button = Gtk.Button(icon_name="edit-copy-symbolic")
                copy_button.add_css_class("flat")
                copy_button.set_valign(Gtk.Align.CENTER)
                copy_button.set_tooltip_text("Copy invite URL")
                copy_button.connect(
                    "clicked",
                    lambda _b, u=str(invite_url): w.copy_to_clipboard(self, u),
                )
                invite_row.add_suffix(copy_button)
                invite_row.add_suffix(open_button)
                row.add_row(invite_row)
            else:
                row.add_row(Adw.ActionRow(
                    title="No invite URL",
                    subtitle="This bot has no invite configured.",
                ))

            permissions = field(bot, "permissions", []) or []
            if permissions:
                row.add_row(Adw.ActionRow(
                    title="Permissions",
                    subtitle=", ".join(str(p) for p in permissions),
                ))
            group.add(row)
        box.append(group)
        return w.clamp_of(box)


def _as_int(value: Any) -> int:
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return 0
