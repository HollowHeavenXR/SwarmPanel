"""Drive the app under Xvfb and capture a PNG of every screen and action.

Not a pass/fail test -- it produces images for a human (or me) to actually
look at, which is the only way to catch "this renders but looks wrong":
empty panels, missing artwork, clipped text, a dialog that opens blank.

Runs against tests/fake_backend.py, so it needs no credentials and no
network, and every screen is populated rather than showing its empty state.
"""

import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import isolate  # noqa: F401  -- must precede every swarmpanel import

import fake_backend

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, GLib, Gtk

import os

OUT = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/swarmpanel-shots")
OUT.mkdir(parents=True, exist_ok=True)

# Live mode drives a REAL backend with REAL credentials, so it is strictly
# read-only: every step that would write to the account is skipped, and the
# destructive dialogs are only ever opened to photograph them, never
# confirmed. Credentials come from the environment so they are never written
# to this file or to disk.
LIVE_URL = os.environ.get("SWARMPANEL_URL") or ""
LIVE_USER = os.environ.get("SWARMPANEL_USER") or ""
LIVE_PASSWORD = os.environ.get("SWARMPANEL_PASSWORD") or ""
LIVE = bool(LIVE_URL and LIVE_USER and LIVE_PASSWORD)

from swarmpanel.core import api as api_module
from swarmpanel.core.config import settings

if LIVE:
    settings.base_url = LIVE_URL
    print(f"LIVE run against {LIVE_URL} as {LIVE_USER} (read-only)")
else:
    settings.base_url = fake_backend.start()

from swarmpanel.ui.window import MainWindow

SHOTS = []
NOTES = []


def capture(name: str) -> None:
    index = len(SHOTS) + 1
    path = OUT / f"{index:02d}-{name}.png"
    result = subprocess.run(
        ["import", "-window", "root", str(path)],
        capture_output=True,
    )
    if result.returncode != 0 or not path.exists():
        NOTES.append(f"capture failed for {name}: {result.stderr.decode()[:200]}")
        return
    # Trim to a sane width so many images stay reviewable, and flatten the
    # alpha the root grab leaves behind.
    subprocess.run(["mogrify", "-background", "#1c1c22", "-alpha", "remove",
                    "-resize", "1200x", str(path)], capture_output=True)
    SHOTS.append(path.name)
    print(f"  {index:02d} {name}")


def find(widget, predicate):
    if predicate(widget):
        return widget
    child = widget.get_first_child()
    while child is not None:
        found = find(child, predicate)
        if found is not None:
            return found
        child = child.get_next_sibling()
    return None


def find_all(widget, predicate, out=None):
    out = [] if out is None else out
    if predicate(widget):
        out.append(widget)
    child = widget.get_first_child()
    while child is not None:
        find_all(child, predicate, out)
        child = child.get_next_sibling()
    return out


def button_labelled(root, text):
    return find(root, lambda x: isinstance(x, Gtk.Button)
                and (x.get_label() or "") == text)


def close_dialogs(window):
    for dialog in list(find_all(window, lambda x: isinstance(x, Adw.Dialog))):
        try:
            dialog.force_close()
        except Exception:
            pass
    popover = find(window, lambda x: isinstance(x, Gtk.Popover) and x.get_visible())
    if popover is not None:
        popover.popdown()


class Driver(Adw.Application):
    def __init__(self):
        super().__init__(application_id="studio.xenusanimations.SwarmPanelShots")
        self.window = None
        self.steps = []

    def do_activate(self):
        self.window = MainWindow(self)
        self.window.set_default_size(1340, 860)
        self.window.present()
        self.steps = self.build_steps()
        GLib.timeout_add(1800, self.run_step, 0)

    # -- the script --------------------------------------------------------

    def build_steps(self):
        w = self.window
        steps = []

        # Auth, before any session exists.
        steps.append(("login", lambda: None))
        steps.append(("register",
                      lambda: w._auth._stack.set_visible_child_name("register")))
        steps.append(("login-server-address", lambda: w._auth._edit_server()))
        steps.append((None, lambda: close_dialogs(w)))

        # Sign in with the real login payload shape.
        def sign_in():
            w._auth._stack.set_visible_child_name("login")
            if LIVE:
                # Authenticate over the API rather than typing into the form,
                # so the password is never rendered into a screenshot.
                payload = api_module.login(LIVE_USER, LIVE_PASSWORD)
                NOTES.append(
                    "signed in live as "
                    f"{payload.get('username')} role={payload.get('role')} "
                    f"admin_mode={payload.get('admin_mode')} "
                    f"site_owner={payload.get('site_owner')}")
                w._enter_shell(payload)
            else:
                api_module.api.token = "smoke-token"
                w._enter_shell(dict(fake_backend.LOGIN))

        steps.append((None, sign_in))

        from swarmpanel.core import nav

        for item in nav.all_items():
            label = item.label.lower().replace(" ", "-")
            steps.append((f"screen-{label}",
                          lambda to=item.to: w.navigate(to)))

            # Per-screen actions worth seeing.
            if item.to == "/":
                steps.append(("action-bot-detail", self.open_bot_detail))
                steps.append((None, lambda: close_dialogs(w)))
            if item.to == "/controls":
                steps.append(("action-controls-save-queue",
                              lambda: self.click("Save…")))
                steps.append((None, lambda: close_dialogs(w)))
                steps.append(("action-controls-destructive-confirm",
                              lambda: self.click("Stop")))
                steps.append((None, lambda: close_dialogs(w)))
            if item.to == "/users":
                steps.append(("action-public-profile",
                              lambda: self.click("Profile")))
                steps.append((None, lambda: close_dialogs(w)))
            if item.to == "/profile":
                steps.append(("action-change-password",
                              lambda: self.click("Change…")))
                steps.append((None, lambda: close_dialogs(w)))
            if item.to == "/databases" and not LIVE:
                # Browsing a real schema is a read, but the truncate dialog
                # sits one keystroke from destroying a production table --
                # photographed against the fake backend only.
                steps.append(("action-pick-schema", self.pick_database))
                steps.append(("action-truncate-confirm",
                              lambda: self.click("Truncate table")))
                steps.append((None, lambda: close_dialogs(w)))
            if item.to == "/audit-log":
                steps.append(("action-audit-revert-confirm",
                              lambda: self.click("Revert")))
                steps.append((None, lambda: close_dialogs(w)))
            if item.to == "/accounts":
                steps.append(("action-expand-account", self.expand_first))
                steps.append(("action-account-delete-confirm",
                              lambda: self.click("Delete")))
                steps.append((None, lambda: close_dialogs(w)))
            if item.to == "/appearance" and not LIVE:
                # Writes to the account and would change the look of the web
                # panel, the iPhone app and the Apple TV -- never in live mode.
                steps.append(("action-apply-preset", lambda: self.click("Apply")))

        # Window chrome.
        steps.append(("chrome-notifications", lambda: w._bell.popup()))
        steps.append((None, lambda: close_dialogs(w)))
        if not LIVE:
            # Toggling admin mode POSTs to the session, changing the real
            # account's mode -- demonstrated against the fake backend only.
            steps.append(("chrome-admin-mode-off",
                          lambda: w._admin_toggle.set_active(False)))
            steps.append(("chrome-sidebar-non-admin", lambda: w.navigate("/")))
            steps.append((None, lambda: w._admin_toggle.set_active(True)))
        steps.append(("chrome-about", lambda: w._show_about()))
        steps.append((None, lambda: close_dialogs(w)))
        steps.append(("chrome-sign-out-confirm", lambda: w._sign_out()))
        steps.append((None, lambda: close_dialogs(w)))
        return steps

    # -- step helpers ------------------------------------------------------

    def click(self, label):
        target = button_labelled(self.window, label)
        if target is None:
            NOTES.append(f"no button labelled {label!r} to click")
            return
        target.emit("clicked")

    def open_bot_detail(self):
        screen = self.window._screens.get("/")
        if screen is None:
            return
        bots = (screen.payload or {}).get("bots") or []
        if bots:
            screen._open_detail(bots[0])

    def expand_first(self):
        row = find(self.window, lambda x: isinstance(x, Adw.ExpanderRow))
        if row is not None:
            row.set_expanded(True)

    def pick_database(self):
        screen = self.window._screens.get("/databases")
        if screen is None:
            return
        screen.schema = "panel"
        screen.table = "sessions"
        screen.refresh(show_spinner=False)

    # -- loop --------------------------------------------------------------

    def run_step(self, index):
        if index >= len(self.steps):
            GLib.timeout_add(500, lambda: self.quit() or GLib.SOURCE_REMOVE)
            return GLib.SOURCE_REMOVE
        name, action = self.steps[index]
        try:
            action()
        except Exception as error:
            import traceback
            traceback.print_exc()
            NOTES.append(f"step {name or '(setup)'} raised {error!r}")

        def then():
            if name:
                capture(name)
            self.run_step(index + 1)
            return GLib.SOURCE_REMOVE

        # Long enough for a worker thread to land its payload, render, and
        # for images to arrive. A real backend is far slower than the fake
        # one -- the audit log and the admin tables take seconds -- so live
        # runs wait proportionally longer rather than photographing a
        # spinner.
        GLib.timeout_add(2600 if LIVE else 900, then)
        return GLib.SOURCE_REMOVE


app = Driver()
app.run([])

print(f"\n{len(SHOTS)} screenshots in {OUT}")
if NOTES:
    print("\nnotes:")
    for note in NOTES:
        print("  -", note)
