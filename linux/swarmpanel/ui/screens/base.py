"""Base class for every panel screen -- the port of the SwiftUI view models'
shared lifecycle.

Each screen gets the same three-state body (loading / error / content), an
optional live `watch` key, and a REST fallback poll used only while the
socket is down -- the same arrangement as the iOS view models'
`if !socket.isConnected { poll }`.

Lifecycle is driven by `activate()` / `deactivate()`, called by the window
when the screen becomes or stops being the visible one. A deactivated screen
unwatches its key, stops polling, and cancels in-flight work so a late
response can't land on a screen the user has left.
"""

from __future__ import annotations

from typing import Any, Callable

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, GLib, Gtk

from ...core import work
from ...core.live import live
from .. import widgets as w


class Screen(Gtk.Box):
    """A panel screen.

    Subclasses implement `load()` (fetch via REST) and/or set `watch_key`,
    then `render(payload)` to build the content widget.
    """

    #: live watch key from routes.lua's registry, when this screen has one
    watch_key: str | None = None
    #: seconds between REST refreshes while the live socket is down
    fallback_interval: float = 15.0
    #: shown above the content; usually the nav blurb
    subtitle: str = ""
    #: whether the window should wrap this screen in a vertical scroller.
    #: Screens that own their own scrolling (split panes, chat transcripts)
    #: set this False so they don't end up with two nested scrollbars.
    scrollable: bool = True

    def __init__(self, window: Any, title: str = "", subtitle: str = "") -> None:
        super().__init__(orientation=Gtk.Orientation.VERTICAL)
        self.window = window
        self.title = title
        if subtitle:
            self.subtitle = subtitle
        self._stack = Gtk.Stack()
        self._stack.set_transition_type(Gtk.StackTransitionType.CROSSFADE)
        self._stack.set_vexpand(True)
        self.append(self._stack)

        self._loading = w.spinner_page()
        self._stack.add_named(self._loading, "loading")
        self._content_holder = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self._content_holder.set_vexpand(True)
        self._stack.add_named(self._content_holder, "content")
        self._error_holder = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self._error_holder.set_vexpand(True)
        self._stack.add_named(self._error_holder, "error")
        self._stack.set_visible_child_name("loading")

        self._job: work.Job | None = None
        self._poll_source: int | None = None
        self._active = False
        self._loaded_once = False
        self.payload: Any = None

        # UI state that must outlive a re-render. A live screen rebuilds its
        # content whenever a snapshot arrives -- for the dashboard that is
        # every couple of seconds, because the track position moves -- and
        # without this an expanded row would collapse under the user's
        # cursor and the view would jump back to the top mid-scroll.
        self._expanded: set[str] = set()
        self._scroll_at: float | None = None

    # -- subclass hooks ----------------------------------------------------

    def load(self) -> Any:
        """Fetch this screen's data. Runs on a worker thread; may raise
        ApiError. Return None to indicate "nothing to fetch over REST"."""
        return None

    def render(self, payload: Any) -> Gtk.Widget:
        """Build the content widget for `payload`. Runs on the main thread."""
        raise NotImplementedError

    def on_account(self, account: Any) -> None:
        """Called on the Appearance screen when the `account` feed updates."""

    def on_live_snapshot(self, payload: Any) -> None:
        """Default: treat a snapshot exactly like a fresh REST payload."""
        self.apply(payload)

    # -- lifecycle ---------------------------------------------------------

    def activate(self) -> None:
        if self._active:
            return
        self._active = True
        if self.watch_key:
            live.watch(
                self.watch_key,
                self.on_live_snapshot,
                params=self.watch_params(),
                on_error=self.show_error,
            )
        # Always do one REST read: it fills the screen immediately rather
        # than waiting for the next broadcast tick, and it is the only
        # source for screens with no watch key at all.
        if not self._loaded_once or not self.watch_key:
            self.refresh()
        self._start_fallback_poll()

    def deactivate(self) -> None:
        if not self._active:
            return
        self._active = False
        if self.watch_key:
            live.unwatch(self.watch_key)
        self._stop_fallback_poll()
        if self._job is not None:
            self._job.cancel()
            self._job = None

    def watch_params(self) -> dict[str, str] | None:
        return None

    # -- data --------------------------------------------------------------

    def refresh(self, *, show_spinner: bool | None = None) -> None:
        if show_spinner is None:
            show_spinner = not self._loaded_once
        if show_spinner:
            self._stack.set_visible_child_name("loading")
        if self._job is not None:
            self._job.cancel()
        self._job = work.run(
            self.load,
            on_success=self._on_loaded,
            on_error=self.show_error,
        )

    def _on_loaded(self, payload: Any) -> None:
        if payload is None and self._loaded_once:
            return
        self.apply(payload)

    def apply(self, payload: Any) -> None:
        """Replace the content with a fresh render of `payload`."""
        self.payload = payload
        try:
            child = self.render(payload)
        except Exception as error:  # a malformed payload must not blank the app
            import traceback

            traceback.print_exc()
            self.show_error(f"Couldn't display this: {error}")
            return
        self._set_content(child)
        self._loaded_once = True
        self._stack.set_visible_child_name("content")

    def _set_content(self, child: Gtk.Widget) -> None:
        self._remember_scroll()
        existing = self._content_holder.get_first_child()
        while existing is not None:
            nxt = existing.get_next_sibling()
            self._content_holder.remove(existing)
            existing = nxt
        self._content_holder.append(child)
        self._restore_scroll()

    # -- re-render state preservation --------------------------------------

    def expander(self, key: str, title: str, subtitle: str = "") -> Adw.ExpanderRow:
        """An ExpanderRow whose open/closed state survives a re-render.

        `key` must be stable for the thing the row represents (a bot key, an
        account id) -- that is what lets the rebuilt row be recognised as
        the same row the user had opened.
        """
        row = Adw.ExpanderRow(title=title)
        if subtitle:
            row.set_subtitle(subtitle)
        row.set_expanded(key in self._expanded)
        row.connect("notify::expanded", self._on_expanded, key)
        return row

    def _on_expanded(self, row: Adw.ExpanderRow, _param, key: str) -> None:
        if row.get_expanded():
            self._expanded.add(key)
        else:
            self._expanded.discard(key)

    def _scroller(self) -> Gtk.ScrolledWindow | None:
        """The window wraps scrollable screens, so the scroller is this
        widget's parent rather than something it owns."""
        parent = self.get_parent()
        return parent if isinstance(parent, Gtk.ScrolledWindow) else None

    def _remember_scroll(self) -> None:
        scroller = self._scroller()
        if scroller is None:
            return
        adjustment = scroller.get_vadjustment()
        if adjustment is not None and adjustment.get_value() > 0:
            self._scroll_at = adjustment.get_value()

    def _restore_scroll(self) -> None:
        if self._scroll_at is None:
            return
        scroller = self._scroller()
        if scroller is None:
            return
        target = self._scroll_at

        def apply_scroll() -> bool:
            # Deferred: the fresh content has no allocated height yet, so
            # setting the value now would clamp it to zero.
            adjustment = scroller.get_vadjustment()
            if adjustment is not None:
                upper = adjustment.get_upper() - adjustment.get_page_size()
                adjustment.set_value(min(target, max(0.0, upper)))
            return GLib.SOURCE_REMOVE

        GLib.idle_add(apply_scroll)

    def show_error(self, message: str) -> None:
        if self._loaded_once:
            # Content is already on screen; a transient failure should not
            # throw it away -- surface it as a toast instead.
            w.toast(self.window, message)
            return
        existing = self._error_holder.get_first_child()
        while existing is not None:
            nxt = existing.get_next_sibling()
            self._error_holder.remove(existing)
            existing = nxt
        self._error_holder.append(
            w.error_state(message, on_retry=lambda: self.refresh(show_spinner=True))
        )
        self._stack.set_visible_child_name("error")

    # -- REST fallback polling --------------------------------------------

    def _start_fallback_poll(self) -> None:
        if self._poll_source is not None or not self.fallback_interval:
            return
        self._poll_source = GLib.timeout_add_seconds(
            int(self.fallback_interval), self._fallback_tick
        )

    def _stop_fallback_poll(self) -> None:
        if self._poll_source is not None:
            GLib.source_remove(self._poll_source)
            self._poll_source = None

    def _fallback_tick(self) -> bool:
        if not self._active:
            self._poll_source = None
            return GLib.SOURCE_REMOVE
        # A screen fed by a healthy live socket needs no polling at all --
        # this is exactly the iOS clients' `if !socket.isConnected` guard.
        if self.watch_key and live.connected:
            return GLib.SOURCE_CONTINUE
        self.refresh(show_spinner=False)
        return GLib.SOURCE_CONTINUE

    # -- helpers for subclasses -------------------------------------------

    def act(self, work_fn: Callable[[], Any], *, success: str = "",
            then: Callable[[Any], None] | None = None) -> None:
        """Fire a mutating request, toast the outcome, then refresh.

        Centralising this is what keeps every action button in the app
        consistent about reporting failure instead of silently doing nothing.
        """

        def on_success(result: Any) -> None:
            if success:
                w.toast(self.window, success)
            if then is not None:
                then(result)
            else:
                self.refresh(show_spinner=False)

        work.run(work_fn, on_success=on_success,
                 on_error=lambda message: w.toast(self.window, message))

    def body(self) -> Gtk.Box:
        """Standard page column, with the screen's blurb at the top."""
        box = w.page_body()
        if self.subtitle:
            box.append(w.dim(self.subtitle))
        return box
