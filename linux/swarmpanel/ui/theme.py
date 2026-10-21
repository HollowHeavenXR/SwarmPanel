"""Apply the account's panel preferences to this window.

The panel's appearance settings belong to the *account*, so the web UI, the
iPhone app and the Apple TV all follow them. This makes the desktop client
follow them too, for the subset that has a sensible native meaning:

* `theme_mode` maps onto Adw.StyleManager's colour scheme.
* `accent_color` is injected as the libadwaita accent colour, so buttons,
  switches and the selected sidebar row pick up the operator's own colour.
* `density`, `font_scale`, `card_shape` and `motion` become CSS classes on
  the window, which style.css keys off.

Deliberately *not* followed: `background_mode`/`background_image_url`. A
desktop window that paints its own aurora gradient stops looking like a GTK
application, and the user's local light/dark choice is a separate setting on
the Appearance screen for the same reason.

Values are validated against profiles.lua's PANEL_* enums; anything
unrecognised is ignored rather than passed through, so a preset added
server-side can never produce a broken stylesheet here.
"""

from __future__ import annotations

import re

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gdk, Gtk

from ..core.casing import field
from ..core.config import settings

THEME_MODES = {"dark", "light", "system"}
DENSITIES = {"comfortable", "compact"}
FONT_SCALES = {"normal", "large", "dense"}
CARD_SHAPES = {"soft", "crisp"}
MOTIONS = {"standard", "reduced"}

_HEX = re.compile(r"^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$")
_accent_provider: Gtk.CssProvider | None = None


def _valid_hex(value) -> str | None:
    if isinstance(value, str) and _HEX.match(value.strip()):
        return value.strip()
    return None


def apply_color_scheme() -> None:
    """Honour the *local* light/dark choice, which always wins.

    A Linux desktop should follow the system theme by default rather than
    whatever the phone was last set to, so the account's `theme_mode` is
    only consulted when the local setting is "follow the panel".
    """
    manager = Adw.StyleManager.get_default()
    local = settings.color_scheme
    if local == "light":
        manager.set_color_scheme(Adw.ColorScheme.FORCE_LIGHT)
        return
    if local == "dark":
        manager.set_color_scheme(Adw.ColorScheme.FORCE_DARK)
        return
    if local == "panel":
        mode = settings.get("panelThemeMode", "system")
        manager.set_color_scheme({
            "dark": Adw.ColorScheme.FORCE_DARK,
            "light": Adw.ColorScheme.FORCE_LIGHT,
        }.get(mode, Adw.ColorScheme.DEFAULT))
        return
    manager.set_color_scheme(Adw.ColorScheme.DEFAULT)


def apply_accent(hex_color) -> None:
    """Override libadwaita's accent colour with the panel accent.

    Done with a CSS provider rather than Adw.StyleManager's accent property,
    which only accepts a fixed palette of named colours -- the panel stores
    an arbitrary hex value.
    """
    global _accent_provider
    display = Gdk.Display.get_default()
    if display is None:
        return
    colour = _valid_hex(hex_color)
    if _accent_provider is not None:
        Gtk.StyleContext.remove_provider_for_display(display, _accent_provider)
        _accent_provider = None
    if colour is None:
        return
    provider = Gtk.CssProvider()
    # Both spellings: the @define-color form for widgets still reading the
    # named colour, and the CSS variable GTK 4.16+ resolves.
    provider.load_from_string(
        f"@define-color accent_bg_color {colour};\n"
        f"@define-color accent_color {colour};\n"
        f":root {{ --accent-bg-color: {colour}; --accent-color: {colour}; }}\n"
    )
    Gtk.StyleContext.add_provider_for_display(
        display, provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 1
    )
    _accent_provider = provider


def apply_preferences(window: Gtk.Widget, preferences) -> None:
    """Apply a decoded panel-preferences payload to `window`."""
    if not preferences:
        return

    theme_mode = field(preferences, "themeMode")
    if theme_mode in THEME_MODES:
        # Remembered so the "follow the panel" option still works on the
        # next launch, before the account feed has reconnected.
        settings.set("panelThemeMode", theme_mode)
    apply_color_scheme()
    apply_accent(field(preferences, "accentColor"))

    for group, value in (
        (DENSITIES, field(preferences, "density")),
        (FONT_SCALES, field(preferences, "fontScale")),
        (CARD_SHAPES, field(preferences, "cardShape")),
        (MOTIONS, field(preferences, "motion")),
    ):
        # Drop any previous value from this group before adding the new one,
        # so switching presets does not leave both classes applied.
        for candidate in group:
            window.remove_css_class(f"swarm-{candidate}")
        if value in group:
            window.add_css_class(f"swarm-{value}")
