"""Adw.Application entry point."""

from __future__ import annotations

import os
import pathlib
import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gdk, Gio, Gtk

from ..core.config import APP_ID, settings


class SwarmPanelApp(Adw.Application):
    def __init__(self) -> None:
        super().__init__(
            application_id=APP_ID,
            flags=Gio.ApplicationFlags.DEFAULT_FLAGS,
        )
        self._window = None

    def do_startup(self) -> None:
        Adw.Application.do_startup(self)
        self._load_css()
        self._apply_color_scheme()
        self.set_accels_for_action("win.refresh-session", ["<Primary>r"])

    def do_activate(self) -> None:
        from .window import MainWindow

        if self._window is None:
            self._window = MainWindow(self)
        self._window.present()

    def _load_css(self) -> None:
        css_path = Path(__file__).with_name("style.css")
        if not css_path.exists():
            return
        provider = Gtk.CssProvider()
        provider.load_from_path(str(css_path))
        display = Gdk.Display.get_default()
        if display is not None:
            Gtk.StyleContext.add_provider_for_display(
                display, provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
            )

    def _apply_color_scheme(self) -> None:
        # Shared with ui/theme.py so the "follow my panel theme" option is
        # honoured at launch, before the account feed has connected, using
        # the last theme_mode we saw.
        from .theme import apply_color_scheme

        apply_color_scheme()


def self_check() -> int:
    """Report whether this install's runtime environment is complete.

    Packaging a GTK app is mostly about environment -- typelibs, icon theme,
    GSettings schemas -- and a missing piece usually shows up as a blank
    window or a silently degraded feature rather than a crash. This makes
    that state inspectable: `swarmpanel --self-check` after a rebuild tells
    you whether the wrapper actually found everything.
    """
    from ..core import secrets
    from ..core.config import SYSTEM_CONFIG, settings

    ok = True
    print("SwarmPanel self-check")
    print(f"  backend        {settings.base_url}")

    namespaces = (("Gtk", "4.0"), ("Gdk", "4.0"), ("Adw", "1"), ("Secret", "1"))
    for namespace, version in namespaces:
        try:
            gi.require_version(namespace, version)
            __import__(f"gi.repository.{namespace}")
            print(f"  {namespace + ' ' + version:<14} ok")
        except (ValueError, ImportError) as error:
            # Secret is optional -- the app falls back to a 0600 file -- so
            # only the GTK stack is treated as fatal.
            fatal = namespace != "Secret"
            ok = ok and not fatal
            print(f"  {namespace + ' ' + version:<14} "
                  f"{'MISSING' if fatal else 'missing (optional)'}: {error}")

    try:
        import websockets

        print(f"  websockets     {websockets.__version__}")
    except ImportError as error:
        ok = False
        print(f"  websockets     MISSING: {error}")

    print(f"  token storage  {secrets.backend_name()}")
    if secrets.backend_name() != "Secret Service":
        print("                 (enable gnome-keyring for encrypted storage)")
    print(f"  system config  {SYSTEM_CONFIG}"
          f"{'' if SYSTEM_CONFIG.exists() else ' (absent)'}")

    icon_theme_found = any(
        (pathlib.Path(directory) / "icons" / "Adwaita" / "index.theme").exists()
        for directory in (os.environ.get("XDG_DATA_DIRS") or "").split(":")
        if directory
    )
    print(f"  icon theme     {'Adwaita found' if icon_theme_found else 'Adwaita NOT found'}")

    print("\n" + ("Ready." if ok else "Incomplete install — see MISSING above."))
    return 0 if ok else 1


def main(argv: list[str] | None = None) -> int:
    argv = list(argv if argv is not None else sys.argv)
    if "--self-check" in argv:
        return self_check()
    if "--version" in argv:
        print("SwarmPanel 1.0.0")
        return 0
    return SwarmPanelApp().run(argv)
