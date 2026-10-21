"""Shared presentation pieces -- the port of Sources/DesignSystem.

Everything here is plain GTK4/libadwaita so the app inherits the user's
system theme, accent colour and font size instead of re-implementing a look.
"""

from __future__ import annotations

import datetime as _dt
import json
import math
from typing import Any, Callable, Iterable

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, GLib, Gtk, Pango

from ..core.casing import field, fields

# -- formatting -------------------------------------------------------------

DASH = "—"  # em dash, the panel's "no value" marker


def text_or_dash(value: Any) -> str:
    if value is None:
        return DASH
    text = str(value).strip()
    return text or DASH


def format_duration(seconds: Any) -> str:
    """`3725` -> `1:02:05`. Mirrors the panel's track-time rendering."""
    try:
        total = int(float(seconds))
    except (TypeError, ValueError):
        return DASH
    if total < 0:
        return DASH
    hours, remainder = divmod(total, 3600)
    minutes, secs = divmod(remainder, 60)
    if hours:
        return f"{hours}:{minutes:02d}:{secs:02d}"
    return f"{minutes}:{secs:02d}"


def format_count(value: Any) -> str:
    try:
        number = int(float(value))
    except (TypeError, ValueError):
        return DASH
    return f"{number:,}"


def parse_timestamp(value: Any) -> _dt.datetime | None:
    """Parse the ISO-8601 the backend emits, tolerating a trailing `Z` and
    missing timezone (treated as UTC, which is what the backend stores)."""
    if not isinstance(value, str) or not value.strip():
        return None
    text = value.strip().replace("Z", "+00:00")
    try:
        parsed = _dt.datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=_dt.timezone.utc)
    return parsed


def format_relative(value: Any) -> str:
    """`5m ago` / `in 2h` -- the heartbeat and drift columns' format."""
    parsed = parse_timestamp(value)
    if parsed is None:
        return text_or_dash(value)
    delta = (_dt.datetime.now(_dt.timezone.utc) - parsed).total_seconds()
    future = delta < 0
    delta = abs(delta)
    if delta < 10:
        return "just now"
    for limit, divisor, suffix in (
        (90, 1, "s"), (90 * 60, 60, "m"), (36 * 3600, 3600, "h"),
        (30 * 86400, 86400, "d"),
    ):
        if delta < limit:
            amount = int(delta / divisor)
            return f"in {amount}{suffix}" if future else f"{amount}{suffix} ago"
    return parsed.astimezone().strftime("%Y-%m-%d")


def format_absolute(value: Any) -> str:
    parsed = parse_timestamp(value)
    if parsed is None:
        return text_or_dash(value)
    return parsed.astimezone().strftime("%Y-%m-%d %H:%M")


def pretty_json(value: Any) -> str:
    """Readable rendering of a free-form payload.

    Postgres text columns that *contain* JSON (panel_preferences,
    profile_tags/links) arrive as a plain string; the web panel's
    swarmTableCell() and JSONValue.displayString both re-parse those rather
    than showing one unreadable run-on line, so this does too.
    """
    if isinstance(value, str):
        trimmed = value.strip()
        if len(trimmed) > 2 and trimmed[0] in "{[":
            try:
                return json.dumps(json.loads(trimmed), indent=2, sort_keys=True)
            except ValueError:
                return value
        return value
    if value is None:
        return DASH
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float) and not math.isnan(value):
        return str(int(value)) if value.is_integer() else str(value)
    if isinstance(value, (dict, list)):
        try:
            return json.dumps(value, indent=2, sort_keys=True, default=str)
        except (TypeError, ValueError):
            return str(value)
    return str(value)


def cell_text(value: Any) -> str:
    """One-line form of a value for a table cell."""
    rendered = pretty_json(value)
    collapsed = " ".join(rendered.split())
    return collapsed if len(collapsed) <= 240 else collapsed[:239] + "…"


# -- small widgets ----------------------------------------------------------

def pill(text: str, *, css: str = "") -> Gtk.Widget:
    """Status chip. `css` picks up libadwaita's semantic colours
    (success/warning/error/accent) so it tracks the system theme."""
    label = Gtk.Label(label=text or DASH)
    label.add_css_class("caption-heading")
    label.add_css_class("swarm-pill")
    if css:
        label.add_css_class(css)
    label.set_ellipsize(Pango.EllipsizeMode.END)
    return label


def status_css(status: Any) -> str:
    """Map a backend status/heartbeat word onto a semantic colour class.

    Unknown values deliberately get no colour rather than a guessed one --
    a wrong green is worse than a neutral chip.
    """
    text = str(status or "").strip().lower()
    if text in ("ok", "online", "healthy", "up", "playing", "active",
                "connected", "verified", "resolved", "good"):
        return "swarm-ok"
    if text in ("warn", "warning", "stale", "degraded", "paused", "idle",
                "pending", "partial", "drifting"):
        return "swarm-warn"
    if text in ("error", "offline", "down", "failed", "fail", "critical",
                "missing", "dead", "unhealthy"):
        return "swarm-bad"
    return ""


def heading(text: str, *, level: int = 2) -> Gtk.Label:
    label = Gtk.Label(label=text, xalign=0)
    label.add_css_class("title-%d" % max(1, min(4, level)))
    label.set_wrap(True)
    return label


def dim(text: str, *, wrap: bool = True) -> Gtk.Label:
    label = Gtk.Label(label=text, xalign=0)
    label.add_css_class("dim-label")
    label.add_css_class("caption")
    label.set_wrap(wrap)
    return label


def monospace(text: str) -> Gtk.Widget:
    """Scrollable fixed-width block for raw payloads (JsonPanel's port)."""
    view = Gtk.TextView()
    view.set_editable(False)
    view.set_monospace(True)
    view.set_cursor_visible(False)
    view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
    view.add_css_class("swarm-code")
    view.get_buffer().set_text(text or DASH)
    scroller = Gtk.ScrolledWindow()
    scroller.set_child(view)
    scroller.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
    scroller.set_min_content_height(160)
    scroller.set_propagate_natural_height(True)
    scroller.set_max_content_height(520)
    scroller.add_css_class("card")
    return scroller


def stat_tile(title: str, value: str, subtitle: str = "") -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
    box.add_css_class("card")
    box.add_css_class("swarm-tile")
    caption = Gtk.Label(label=title.upper(), xalign=0)
    caption.add_css_class("caption")
    caption.add_css_class("dim-label")
    amount = Gtk.Label(label=value, xalign=0)
    amount.add_css_class("title-2")
    box.append(caption)
    box.append(amount)
    if subtitle:
        box.append(dim(subtitle))
    return box


def stat_row(tiles: Iterable[Gtk.Widget]) -> Gtk.Widget:
    """Tiles that wrap instead of overflowing on a narrow window."""
    flow = Gtk.FlowBox()
    flow.set_selection_mode(Gtk.SelectionMode.NONE)
    flow.set_homogeneous(True)
    flow.set_column_spacing(12)
    flow.set_row_spacing(12)
    flow.set_min_children_per_line(2)
    flow.set_max_children_per_line(6)
    for tile in tiles:
        flow.append(tile)
    return flow


def key_value(label: str, value: Any, *, copyable: bool = False) -> Adw.ActionRow:
    row = Adw.ActionRow(title=label)
    text = cell_text(value)
    row.set_subtitle(text)
    row.set_subtitle_selectable(True)
    if copyable and text != DASH:
        button = Gtk.Button(icon_name="edit-copy-symbolic")
        button.set_tooltip_text("Copy")
        button.add_css_class("flat")
        button.set_valign(Gtk.Align.CENTER)
        button.connect("clicked", lambda _b: copy_to_clipboard(row, pretty_json(value)))
        row.add_suffix(button)
    return row


def copy_to_clipboard(widget: Gtk.Widget, text: str) -> None:
    display = widget.get_display()
    if display is not None:
        display.get_clipboard().set(text)


def section(title: str, subtitle: str = "") -> Adw.PreferencesGroup:
    group = Adw.PreferencesGroup()
    group.set_title(title)
    if subtitle:
        group.set_description(subtitle)
    return group


def spinner_page(message: str = "Loading…") -> Gtk.Widget:
    """Port of SkeletonLoading -- a plain spinner reads better than fake
    content on desktop, where loads are fast and the window is large."""
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
    box.set_valign(Gtk.Align.CENTER)
    box.set_halign(Gtk.Align.CENTER)
    box.set_vexpand(True)
    spinner = Adw.Spinner()
    spinner.set_size_request(32, 32)
    box.append(spinner)
    box.append(dim(message, wrap=False))
    return box


def empty_state(title: str, description: str = "", icon: str = "folder-symbolic") -> Gtk.Widget:
    status = Adw.StatusPage()
    status.set_icon_name(icon)
    status.set_title(title)
    if description:
        status.set_description(description)
    status.set_vexpand(True)
    return status


def error_state(message: str, on_retry: Callable[[], None] | None = None) -> Gtk.Widget:
    status = Adw.StatusPage()
    status.set_icon_name("dialog-error-symbolic")
    status.set_title("Couldn't load this")
    status.set_description(message)
    status.set_vexpand(True)
    if on_retry is not None:
        button = Gtk.Button(label="Try again")
        button.add_css_class("pill")
        button.add_css_class("suggested-action")
        button.set_halign(Gtk.Align.CENTER)
        button.connect("clicked", lambda _b: on_retry())
        status.set_child(button)
    return status


def scrolled(child: Gtk.Widget) -> Gtk.ScrolledWindow:
    scroller = Gtk.ScrolledWindow()
    scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
    scroller.set_vexpand(True)
    scroller.set_child(child)
    return scroller


def page_body(spacing: int = 18) -> Gtk.Box:
    """Centred, width-clamped column -- the standard Adwaita content shape."""
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=spacing)
    box.set_margin_top(18)
    box.set_margin_bottom(24)
    box.set_margin_start(12)
    box.set_margin_end(12)
    clamp = Adw.Clamp()
    clamp.set_maximum_size(980)
    clamp.set_tightening_threshold(760)
    clamp.set_child(box)
    box._clamp = clamp  # kept so callers can add the clamp to a parent
    return box


def clamp_of(box: Gtk.Box) -> Gtk.Widget:
    return getattr(box, "_clamp", box)


# -- images -----------------------------------------------------------------

def artwork(url: Any, size: int = 56, *, rounded: bool = True) -> Gtk.Widget:
    """Track/bot artwork that fills in once the image arrives.

    Always returns immediately at its final size, showing a muted
    placeholder, so a card does not resize or jump when the picture lands --
    which matters here because a live screen re-renders every couple of
    seconds.
    """
    from ..core import images

    frame = Gtk.Box()
    frame.set_size_request(size, size)
    frame.set_valign(Gtk.Align.CENTER)
    frame.set_overflow(Gtk.Overflow.HIDDEN)
    if rounded:
        frame.add_css_class("swarm-art")

    placeholder = Gtk.Image.new_from_icon_name("audio-x-generic-symbolic")
    placeholder.set_pixel_size(max(16, size // 2))
    placeholder.set_hexpand(True)
    placeholder.add_css_class("dim-label")
    frame.append(placeholder)

    def ready(texture) -> None:
        picture = Gtk.Picture.new_for_paintable(texture)
        picture.set_content_fit(Gtk.ContentFit.COVER)
        picture.set_size_request(size, size)
        existing = frame.get_first_child()
        while existing is not None:
            nxt = existing.get_next_sibling()
            frame.remove(existing)
            existing = nxt
        frame.append(picture)

    images.load(url, ready)
    return frame


def avatar(url: Any, name: str = "", size: int = 36) -> Gtk.Widget:
    """Adwaita avatar, with the remote image applied when it loads.

    Falls back to Adw.Avatar's own initials rendering, which is why the
    display name is worth passing even when there is a picture.
    """
    from ..core import images

    widget = Adw.Avatar(size=size, show_initials=bool(name))
    widget.set_text(name or "")
    widget.set_valign(Gtk.Align.CENTER)
    images.load(images.avatar_url(url), lambda texture: widget.set_custom_image(texture))
    return widget


def progress(position: Any, duration: Any) -> Gtk.Widget | None:
    """Track progress bar, or None when there is nothing sensible to show."""
    try:
        total = float(duration)
        at = float(position)
    except (TypeError, ValueError):
        return None
    if total <= 0:
        return None
    bar = Gtk.ProgressBar()
    bar.set_fraction(max(0.0, min(1.0, at / total)))
    bar.set_valign(Gtk.Align.CENTER)
    bar.add_css_class("osd")
    bar.set_hexpand(True)
    return bar


# -- tables -----------------------------------------------------------------

def table(columns: list[str], rows: list[list[Any]], *, max_height: int = 460) -> Gtk.Widget:
    """Read-only grid for audit rows, database tables and leaderboards.

    Uses a ColumnView over a plain list store so wide results scroll
    horizontally rather than forcing the window wider.
    """
    store = Gtk.StringList()
    # One store entry per row index; the factory reads from `rows` directly so
    # arbitrary cell types survive without stringifying the whole table twice.
    for index in range(len(rows)):
        store.append(str(index))
    selection = Gtk.NoSelection(model=store)
    view = Gtk.ColumnView(model=selection)
    view.add_css_class("swarm-table")
    view.set_show_row_separators(True)
    view.set_show_column_separators(False)

    for column_index, title in enumerate(columns):
        factory = Gtk.SignalListItemFactory()

        def setup(_factory, item):
            label = Gtk.Label(xalign=0)
            label.set_ellipsize(Pango.EllipsizeMode.END)
            label.set_selectable(True)
            item.set_child(label)

        def bind(_factory, item, column_index=column_index):
            label = item.get_child()
            row_index = int(item.get_item().get_string())
            row = rows[row_index]
            value = row[column_index] if column_index < len(row) else None
            text = cell_text(value)
            label.set_text(text)
            label.set_tooltip_text(text if text != DASH else None)

        factory.connect("setup", setup)
        factory.connect("bind", bind)
        column = Gtk.ColumnViewColumn(title=title, factory=factory)
        column.set_resizable(True)
        column.set_expand(True)
        view.append_column(column)

    scroller = Gtk.ScrolledWindow()
    scroller.set_policy(Gtk.PolicyType.AUTOMATIC, Gtk.PolicyType.AUTOMATIC)
    scroller.set_child(view)
    # Sized to the rows actually present: a floor of 120px left a visible
    # dead gap under short tables.
    scroller.set_min_content_height(min(max_height, 40 * (len(rows) + 1)))
    scroller.set_max_content_height(max_height)
    scroller.set_propagate_natural_height(True)
    scroller.add_css_class("card")
    return scroller


def rows_to_table(records: list[dict], *, columns: list[str] | None = None,
                  max_columns: int = 12) -> Gtk.Widget:
    """Table from a list of row dicts, preserving the server's key order.

    Column names are shown exactly as the backend sent them -- these are
    real database column names, so they must not be prettified.
    """
    if not records:
        return empty_state("No rows", "This table is empty.", "view-list-symbolic")
    if columns is None:
        seen: list[str] = []
        for record in records:
            if isinstance(record, dict):
                for key in record:
                    if key not in seen:
                        seen.append(key)
        columns = seen[:max_columns]
    body = [[record.get(column) if isinstance(record, dict) else None
             for column in columns] for record in records]
    return table(columns, body)


# -- dialogs ----------------------------------------------------------------

def toast(window: Any, message: str, *, timeout: int = 4) -> None:
    overlay = getattr(window, "toast_overlay", None)
    if overlay is None:
        return
    item = Adw.Toast(title=message)
    item.set_timeout(timeout)
    overlay.add_toast(item)


def confirm(parent: Gtk.Widget, title: str, body: str, confirm_label: str,
            on_confirm: Callable[[], None], *, destructive: bool = True) -> None:
    dialog = Adw.AlertDialog(heading=title, body=body)
    dialog.add_response("cancel", "Cancel")
    dialog.add_response("go", confirm_label)
    dialog.set_response_appearance(
        "go",
        Adw.ResponseAppearance.DESTRUCTIVE if destructive
        else Adw.ResponseAppearance.SUGGESTED,
    )
    dialog.set_default_response("cancel")
    dialog.set_close_response("cancel")

    def responded(_dialog, response):
        if response == "go":
            on_confirm()

    dialog.connect("response", responded)
    dialog.present(_root_of(parent))


def confirm_phrase(parent: Gtk.Widget, title: str, body: str, phrase: str,
                   confirm_label: str, on_confirm: Callable[[], None],
                   *, extra_label: str | None = None,
                   on_confirm_extra: Callable[[str], None] | None = None) -> None:
    """Type-the-phrase gate for destructive database actions.

    Both the phrase and (for truncates) a second owner phrase are validated
    server-side regardless of what the client sends, so this is a UX guard
    against a mis-click -- not the real authority.
    """
    dialog = Adw.AlertDialog(heading=title, body=body)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    hint = Gtk.Label(xalign=0)
    hint.set_markup(f"Type <tt>{GLib.markup_escape_text(phrase)}</tt> to continue.")
    hint.set_wrap(True)
    entry = Gtk.Entry(placeholder_text=phrase)
    box.append(hint)
    box.append(entry)
    extra_entry = None
    if extra_label:
        box.append(dim(extra_label))
        extra_entry = Gtk.PasswordEntry(show_peek_icon=True)
        box.append(extra_entry)
    dialog.set_extra_child(box)
    dialog.add_response("cancel", "Cancel")
    dialog.add_response("go", confirm_label)
    dialog.set_response_appearance("go", Adw.ResponseAppearance.DESTRUCTIVE)
    dialog.set_response_enabled("go", False)
    dialog.set_default_response("cancel")
    dialog.set_close_response("cancel")

    def validate(*_a):
        typed_ok = entry.get_text().strip() == phrase
        if extra_entry is not None:
            typed_ok = typed_ok and bool(extra_entry.get_text().strip())
        dialog.set_response_enabled("go", typed_ok)

    entry.connect("changed", validate)
    if extra_entry is not None:
        extra_entry.connect("changed", validate)

    def responded(_dialog, response):
        if response != "go":
            return
        if on_confirm_extra is not None and extra_entry is not None:
            on_confirm_extra(extra_entry.get_text())
        else:
            on_confirm()

    dialog.connect("response", responded)
    dialog.present(_root_of(parent))


def prompt(parent: Gtk.Widget, title: str, body: str, placeholder: str,
           confirm_label: str, on_submit: Callable[[str], None],
           *, initial: str = "") -> None:
    dialog = Adw.AlertDialog(heading=title, body=body)
    entry = Gtk.Entry(placeholder_text=placeholder, text=initial)
    entry.set_activates_default(True)
    dialog.set_extra_child(entry)
    dialog.add_response("cancel", "Cancel")
    dialog.add_response("go", confirm_label)
    dialog.set_response_appearance("go", Adw.ResponseAppearance.SUGGESTED)
    dialog.set_default_response("go")
    dialog.set_close_response("cancel")

    def responded(_dialog, response):
        if response == "go":
            text = entry.get_text().strip()
            if text:
                on_submit(text)

    dialog.connect("response", responded)
    dialog.present(_root_of(parent))


def _root_of(widget: Gtk.Widget) -> Gtk.Widget:
    root = widget.get_root() if hasattr(widget, "get_root") else None
    return root or widget
