"""Headless smoke test: build the window and visit all 19 screens.

Fails on any exception *and* on any GTK/GLib warning or criticals, so a
mis-used widget API shows up here instead of in front of the user.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import isolate  # noqa: F401  -- must precede every swarmpanel import

import fake_backend

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, GLib, Gtk

PROBLEMS = []

def _log_handler(domain, level, message, _user_data=None):
    text = f"{domain}: {message}"
    # Missing icon themes and a11y bus noise are environment artefacts of a
    # bare Xvfb, not defects in this code.
    for benign in ("Unable to acquire", "a11y", "atk-bridge", "dbus",
                   "Failed to load module", "GtkFileDialog",
                   "gtk_widget_get_display"):
        if benign.lower() in text.lower():
            return
    PROBLEMS.append(f"[{level.value_nicks if hasattr(level,'value_nicks') else level}] {text}")

for domain in (None, "Gtk", "Gdk", "Adw", "GLib", "GLib-GObject", "Pango",
               "GLib-GIO"):
    GLib.log_set_handler(
        domain,
        GLib.LogLevelFlags.LEVEL_WARNING | GLib.LogLevelFlags.LEVEL_CRITICAL,
        _log_handler, None,
    )

base_url = fake_backend.start()

from swarmpanel.core import api as api_module
from swarmpanel.core import nav
from swarmpanel.core.config import settings

settings.base_url = base_url
api_module.api.token = "smoke-token"

from swarmpanel.ui.window import MainWindow

RESULTS = []


class SmokeApp(Adw.Application):
    def __init__(self):
        super().__init__(application_id="studio.xenusanimations.SwarmPanelSmoke")

    def do_activate(self):
        # An exception here would otherwise leave the app with no window and
        # no scheduled quit, i.e. hang until the outer timeout kills it.
        try:
            window = MainWindow(self)
            window.present()
        except Exception as error:
            import traceback
            traceback.print_exc()
            PROBLEMS.append(f"MainWindow construction raised {error!r}")
            GLib.idle_add(lambda: self.quit() or GLib.SOURCE_REMOVE)
            return
        # The window booted from a stored token, i.e. via GET /api/session.
        # Re-enter the shell with a *login* payload too: that route omits
        # `authenticated`, and previously left the sidebar empty and every
        # screen unreachable.
        try:
            window._enter_shell(dict(fake_backend.LOGIN))
            buttons = len(getattr(window, "_nav_buttons", {}))
            if buttons != 19:
                PROBLEMS.append(
                    f"sidebar has {buttons} items after a login payload, expected 19"
                )
            else:
                print("ok  sidebar populated (19 items) from a login payload")
        except Exception as error:
            import traceback
            traceback.print_exc()
            PROBLEMS.append(f"_enter_shell(login payload) raised {error!r}")

        _check_icons()
        GLib.timeout_add(1500, lambda: self._visit(window, 0))

    def _visit(self, window, index):
        routes = [item.to for item in nav.all_items()]
        if index >= len(routes):
            GLib.timeout_add(800, lambda: self.quit() or GLib.SOURCE_REMOVE)
            return GLib.SOURCE_REMOVE
        route = routes[index]
        if route == "/":
            self._check_rerender_keeps_state(window)
        try:
            window.navigate(route)
        except Exception as error:
            import traceback
            traceback.print_exc()
            PROBLEMS.append(f"navigate({route}) raised {error!r}")
            GLib.timeout_add(50, lambda: self._visit(window, index + 1))
            return GLib.SOURCE_REMOVE

        def record():
            # Sampled *after* a delay: the screen's load() runs on a worker
            # thread, so its render lands a beat after navigate() returns.
            screen = window._screens.get(route)
            state = screen._stack.get_visible_child_name() if screen else "missing"
            RESULTS.append((route, type(screen).__name__ if screen else "?", state))
            self._visit(window, index + 1)
            return GLib.SOURCE_REMOVE

        GLib.timeout_add(900, record)
        return GLib.SOURCE_REMOVE


def _check_icons():
    """Every icon name the app uses must exist in the theme.

    A missing name is not an error at runtime -- GTK silently substitutes a
    blank "missing image" square -- so this is the only way the problem
    surfaces short of looking at a screenshot.
    """
    import re
    from gi.repository import Gdk

    root = Path(__file__).resolve().parents[1] / "swarmpanel"
    pattern = re.compile(r'icon_name="([a-z0-9.-]+)"'
                         r'|set_icon_name\("([a-z0-9.-]+)"\)'
                         r'|"([a-z0-9.-]+-symbolic)"')
    names = set()
    for path in root.rglob("*.py"):
        for groups in pattern.findall(path.read_text()):
            for name in groups:
                if name:
                    names.add(name)
    theme = Gtk.IconTheme.get_for_display(Gdk.Display.get_default())
    missing = sorted(n for n in names if not theme.has_icon(n))
    if missing:
        PROBLEMS.append(f"icon names not in the theme (render as a blank "
                        f"square): {missing}")
    else:
        print(f"ok  all {len(names)} icon names exist in the theme")


def _find(widget, predicate):
    """Depth-first search of a widget tree."""
    if predicate(widget):
        return widget
    child = widget.get_first_child()
    while child is not None:
        found = _find(child, predicate)
        if found is not None:
            return found
        child = child.get_next_sibling()
    return None


def _check_rerender_keeps_state(self, window):
    """A live snapshot must not collapse a row the user opened.

    The dashboard receives a snapshot roughly every two seconds because the
    track position advances, and each one re-renders the screen. Before the
    keyed-expander fix this closed an expanded bot row almost immediately --
    the "clicking a bot closes after a few seconds" bug.
    """
    window.navigate("/")
    screen = window._screens.get("/")
    if screen is None:
        PROBLEMS.append("dashboard screen missing for re-render check")
        return
    screen.apply(dict(fake_backend.DASHBOARD))

    row = _find(screen, lambda widget: isinstance(widget, Adw.ExpanderRow))
    if row is None:
        PROBLEMS.append("no ExpanderRow on the dashboard to test re-render with")
        return
    row.set_expanded(True)
    if not screen._expanded:
        PROBLEMS.append("expanding a bot row did not record any state")
        return
    remembered = set(screen._expanded)

    # Two more snapshots, as the live feed would deliver.
    screen.apply(dict(fake_backend.DASHBOARD))
    screen.apply(dict(fake_backend.DASHBOARD))

    row_after = _find(screen, lambda widget: isinstance(widget, Adw.ExpanderRow))
    if row_after is None:
        PROBLEMS.append("ExpanderRow vanished after a re-render")
    elif not row_after.get_expanded():
        PROBLEMS.append(
            "an expanded bot row collapsed after a live snapshot re-render")
    elif screen._expanded != remembered:
        PROBLEMS.append(f"expanded state drifted: {screen._expanded} != {remembered}")
    else:
        print("ok  expanded bot row survives repeated live snapshots")


SmokeApp._check_rerender_keeps_state = _check_rerender_keeps_state

app = SmokeApp()
app.run([])

print(f"\nvisited {len(RESULTS)} screens\n")
width = max((len(r) for r, _, _ in RESULTS), default=10)
bad = []
for route, name, state in RESULTS:
    mark = "ok " if state == "content" else "BAD"
    if state != "content":
        bad.append((route, state))
    print(f"  {mark} {route:<{width}}  {name:<22} -> {state}")

if PROBLEMS:
    print("\nGTK/GLib problems:")
    for problem in sorted(set(PROBLEMS)):
        print("  " + problem)

print()
if bad:
    print(f"FAIL: {len(bad)} screen(s) did not reach content: {bad}")
    sys.exit(1)
if PROBLEMS:
    print(f"FAIL: {len(set(PROBLEMS))} GTK/GLib warning(s)")
    sys.exit(1)
if len(RESULTS) != 19:
    print(f"FAIL: expected 19 screens, visited {len(RESULTS)}")
    sys.exit(1)
print("ALL 19 SCREENS RENDERED CLEANLY")
