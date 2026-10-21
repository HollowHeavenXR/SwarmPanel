"""Insights section -- Leaderboard and Learning.

Ported from Features/Leaderboard. Both endpoints return "whatever the
recommendation engine knows" shapes that have drifted over time, so these
screens render the parts they recognise and fall back to a readable raw
payload for the rest rather than guessing at field names -- the same
approach the web panel's JsonPanel and the iOS JSONValue path take.
"""

from __future__ import annotations

from typing import Any

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gtk

from ...core.api import api
from ...core import images
from ...core.casing import field, fields, unwrap
from .. import widgets as w
from .base import Screen


class LeaderboardScreen(Screen):
    """Top tracks across the swarm -- GET /api/swarm-leaderboard.

    The response is a flat `{tracks, bots_queried, bots_errored,
    window_days}`: one ranked list built from every bot's play history.
    There is no separate "listeners" list and no `scope` parameter -- the
    only knob is `window_days`, so that is what this screen offers.
    """

    fallback_interval = 60.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.window_days = 30
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        return api.get("/api/swarm-leaderboard", {"days": str(self.window_days)})

    def render(self, payload: Any) -> Gtk.Widget:
        tracks = fields(payload, "tracks", "topTracks", default=[]) or []
        box = self.body()

        window_row = Adw.ComboRow(title="Window")
        windows = [7, 30, 90, 365]
        model = Gtk.StringList()
        for label in ("Last 7 days", "Last 30 days", "Last 90 days", "Last year"):
            model.append(label)
        window_row.set_model(model)
        actual = _as_int(field(payload, "windowDays", self.window_days))
        window_row.set_selected(windows.index(self.window_days)
                                if self.window_days in windows else 1)
        window_row.connect("notify::selected",
                           lambda row, _p: self._choose_window(windows, row.get_selected()))
        group = Adw.PreferencesGroup()
        group.add(window_row)
        box.append(group)

        queried = _as_int(field(payload, "botsQueried", 0))
        errored = _as_int(field(payload, "botsErrored", 0))
        plays = sum(_as_int(field(t, "playCount", 0)) for t in tracks)
        box.append(w.stat_row([
            w.stat_tile("Tracks", w.format_count(len(tracks))),
            w.stat_tile("Plays", w.format_count(plays)),
            w.stat_tile("Window", f"{actual}d"),
            w.stat_tile("Bots queried", w.format_count(queried),
                        f"{errored} errored" if errored else "all responded"),
        ]))

        if not tracks:
            box.append(w.empty_state(
                "Nothing charted yet",
                "No plays recorded in this window.",
                "starred-symbolic",
            ))
            return w.clamp_of(box)

        listing = w.section("Top tracks", f"Across the last {actual} day(s)")
        for index, track in enumerate(tracks[:100], start=1):
            listing.add(self._track_row(index, track))
        box.append(listing)
        return w.clamp_of(box)

    def _track_row(self, index: int, track: Any) -> Gtk.Widget:
        title = str(fields(track, "title", "videoUrl", default=w.DASH))
        row = Adw.ActionRow(title=f"{index}. {title}")
        row.add_prefix(w.artwork(images.youtube_thumbnail(field(track, "videoUrl")),
                                 size=40))
        bits = [f"{w.format_count(field(track, 'playCount', 0))} plays"]
        for label, key in (("finished", "finishCount"), ("skipped", "skipCount"),
                           ("liked", "likeCount"), ("disliked", "dislikeCount")):
            value = _as_int(field(track, key, 0))
            if value:
                bits.append(f"{value} {label}")
        bot = fields(track, "botDisplay", "botKey", default=None)
        if bot:
            bits.insert(0, str(bot))
        last = field(track, "lastPlayed")
        if last:
            bits.append(w.format_relative(last))
        row.set_subtitle(" · ".join(bits))
        row.set_subtitle_lines(2)

        score = field(track, "smartScore")
        if score is not None:
            try:
                row.add_suffix(w.pill(f"score {float(score):.2f}"))
            except (TypeError, ValueError):
                pass

        url = field(track, "videoUrl")
        if url:
            open_button = Gtk.Button(icon_name="adw-external-link-symbolic")
            open_button.add_css_class("flat")
            open_button.set_valign(Gtk.Align.CENTER)
            open_button.connect("clicked",
                                lambda _b, u=str(url): Gtk.UriLauncher(uri=u).launch())
            row.add_suffix(open_button)
        return row

    def _choose_window(self, windows: list[int], index: int) -> None:
        if 0 <= index < len(windows) and windows[index] != self.window_days:
            self.window_days = windows[index]
            self.refresh(show_spinner=True)


class LearningScreen(Screen):
    """What the recommendation engine has learned.

    GET /api/music-intelligence answers `{"ok": true, "data": {"totals": {...},
    "bots": [...]}}` -- the figures live one level down, inside `data`.
    """

    fallback_interval = 60.0

    def load(self) -> Any:
        return unwrap(api.get("/api/music-intelligence"))

    def render(self, payload: Any) -> Gtk.Widget:
        totals = field(payload, "totals") or {}
        bots = field(payload, "bots", []) or []
        box = self.body()

        if totals:
            box.append(w.stat_row([
                w.stat_tile(str(key).replace("_", " ").title(), w.format_count(value))
                for key, value in list(totals.items())[:6]
            ]))

        if not bots:
            box.append(w.empty_state(
                "Nothing learned yet",
                "The recommendation engine hasn't built a model yet.",
                "emoji-symbols-symbolic",
            ))
            return w.clamp_of(box)

        group = w.section("Per bot", f"{len(bots)} node(s) with a model")
        for bot in bots:
            group.add(self._bot_row(bot))
        box.append(group)
        return w.clamp_of(box)

    def _bot_row(self, bot: Any) -> Gtk.Widget:
        key = str(fields(bot, "botKey", "schema", default=""))
        label = str(fields(bot, "botDisplay", "botKey", default=key) or key)
        bits = []
        for title, name in (("learned", "learnedTracks"), ("plays", "plays"),
                            ("likes", "likes"), ("dislikes", "dislikes"),
                            ("skips", "skips"), ("finishes", "finishes"),
                            ("recommended", "recommendations")):
            value = _as_int(field(bot, name, 0))
            if value:
                bits.append(f"{w.format_count(value)} {title}")
        row = self.expander(f"learn:{key}", label, " · ".join(bits) or w.DASH)

        top = field(bot, "topTracks", []) or []
        if not top:
            row.add_row(Adw.ActionRow(title="No learned tracks yet"))
        for index, track in enumerate(top[:15], start=1):
            inner = Adw.ActionRow(
                title=f"{index}. {fields(track, 'title', 'videoUrl', default=w.DASH)}")
            detail = []
            for title, name in (("plays", "playCount"), ("likes", "likeCount"),
                                ("skips", "skipCount")):
                value = _as_int(field(track, name, 0))
                if value:
                    detail.append(f"{value} {title}")
            score = field(track, "smartScore")
            if score is not None:
                try:
                    detail.append(f"score {float(score):.2f}")
                except (TypeError, ValueError):
                    pass
            inner.set_subtitle(" · ".join(detail))
            row.add_row(inner)
        return row


def _as_int(value: Any) -> int:
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return 0
