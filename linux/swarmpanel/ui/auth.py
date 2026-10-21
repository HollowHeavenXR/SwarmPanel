"""Sign-in and registration -- port of Features/Auth.

Registration mirrors the backend's two proof paths exactly: either a Discord
*webhook URL* belonging to the guild being claimed, or a Discord *user ID*
the backend DMs a code to. One of the two is required, which is why the form
makes the choice explicit rather than leaving two optional boxes.
"""

from __future__ import annotations

from typing import Callable

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gtk

from ..core import api as api_module
from ..core import work
from ..core.casing import field
from ..core.config import settings
from . import widgets as w


class AuthView(Gtk.Box):
    """Login, with a registration flow and a server-address override."""

    def __init__(self, on_signed_in: Callable[[dict], None]) -> None:
        super().__init__(orientation=Gtk.Orientation.VERTICAL)
        self.on_signed_in = on_signed_in
        self._busy = False

        self.toast_overlay = Adw.ToastOverlay()
        self.append(self.toast_overlay)
        self.toast_overlay.set_vexpand(True)

        self._stack = Gtk.Stack()
        self._stack.set_transition_type(Gtk.StackTransitionType.SLIDE_LEFT_RIGHT)
        self.toast_overlay.set_child(self._stack)
        self._stack.add_named(self._build_login(), "login")
        self._stack.add_named(self._build_register(), "register")

    # -- login -------------------------------------------------------------

    def _build_login(self) -> Gtk.Widget:
        page = Adw.StatusPage()
        page.set_icon_name("network-server-symbolic")
        page.set_title("SwarmPanel")
        page.set_description("Command centre for Aria and the music bot fleet.")

        column = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        column.set_halign(Gtk.Align.CENTER)

        group = Adw.PreferencesGroup()
        self.username = Adw.EntryRow(title="Username")
        self.username.set_input_hints(Gtk.InputHints.NO_SPELLCHECK)
        self.password = Adw.PasswordEntryRow(title="Password")
        self.guild = Adw.EntryRow(title="Guild ID (optional)")
        group.add(self.username)
        group.add(self.password)
        group.add(self.guild)

        remember = Adw.SwitchRow(title="Remember this device")
        remember.set_subtitle("Keeps a long-lived session so you stay signed in.")
        remember.set_active(settings.remember_device)
        remember.connect(
            "notify::active",
            lambda row, _p: setattr(settings, "remember_device", row.get_active()),
        )
        group.add(remember)
        column.append(group)

        self.sign_in_button = Gtk.Button(label="Sign in")
        self.sign_in_button.add_css_class("suggested-action")
        self.sign_in_button.add_css_class("pill")
        self.sign_in_button.connect("clicked", lambda _b: self._sign_in())
        column.append(self.sign_in_button)

        # Enter submits from any field.
        for row in (self.username, self.password, self.guild):
            row.connect("entry-activated", lambda _r: self._sign_in())

        links = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        links.set_halign(Gtk.Align.CENTER)
        create = Gtk.Button(label="Create an account")
        create.add_css_class("flat")
        create.connect("clicked", lambda _b: self._stack.set_visible_child_name("register"))
        server = Gtk.Button(label="Server address")
        server.add_css_class("flat")
        server.connect("clicked", lambda _b: self._edit_server())
        links.append(create)
        links.append(server)
        column.append(links)

        self.server_label = w.dim("")
        self.server_label.set_halign(Gtk.Align.CENTER)
        self._refresh_server_label()
        column.append(self.server_label)

        page.set_child(column)
        return w.scrolled(page)

    def _refresh_server_label(self) -> None:
        self.server_label.set_text(f"{settings.base_url}  ·  token in {_secret_backend()}")

    def _edit_server(self) -> None:
        w.prompt(
            self, "Server address",
            "The panel backend to sign in against. Leave the default unless "
            "you're testing a local container.",
            "https://swarmpanel.xenusanimations.studio", "Save",
            self._apply_server, initial=settings.base_url,
        )

    def _apply_server(self, value: str) -> None:
        if not value.startswith(("http://", "https://")):
            value = "https://" + value
        settings.base_url = value
        self._refresh_server_label()
        w.toast(self, f"Server set to {settings.base_url}")

    def _sign_in(self) -> None:
        username = self.username.get_text().strip()
        password = self.password.get_text()
        if not username or not password:
            w.toast(self, "Enter your username and password.")
            return
        self._set_busy(True)
        work.run(
            lambda: api_module.login(username, password, self.guild.get_text().strip() or None),
            on_success=self._signed_in,
            on_error=lambda message: w.toast(self, message),
            on_finally=lambda: self._set_busy(False),
        )

    def _signed_in(self, payload: dict) -> None:
        if field(payload, "verificationSent"):
            w.toast(self, "Check your email to finish verifying this account.")
        self.password.set_text("")
        self.on_signed_in(payload)

    def _set_busy(self, busy: bool) -> None:
        self._busy = busy
        self.sign_in_button.set_sensitive(not busy)
        self.sign_in_button.set_label("Signing in…" if busy else "Sign in")
        self.register_button.set_sensitive(not busy)

    # -- register ----------------------------------------------------------

    def _build_register(self) -> Gtk.Widget:
        page = Adw.StatusPage()
        page.set_icon_name("contact-new-symbolic")
        page.set_title("Create an account")
        page.set_description(
            "Registering a guild account needs proof you control that Discord "
            "server: either a webhook URL from it, or your Discord user ID for "
            "a verification DM."
        )

        column = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        column.set_halign(Gtk.Align.CENTER)

        group = Adw.PreferencesGroup(title="Account")
        self.r_username = Adw.EntryRow(title="Username")
        self.r_guild = Adw.EntryRow(title="Guild ID")
        self.r_password = Adw.PasswordEntryRow(title="Password")
        self.r_email = Adw.EntryRow(title="Email (optional)")
        for row in (self.r_username, self.r_guild, self.r_password, self.r_email):
            group.add(row)
        column.append(group)

        proof = Adw.PreferencesGroup(
            title="Ownership proof",
            description="Owner elevation also requires a verified email that "
                        "matches the site owner address.",
        )
        self.proof_choice = Adw.ComboRow(title="Method")
        model = Gtk.StringList()
        model.append("Guild webhook URL")
        model.append("Discord user ID (DM code)")
        self.proof_choice.set_model(model)
        self.r_webhook = Adw.EntryRow(title="Webhook URL")
        self.r_discord_id = Adw.EntryRow(title="Discord user ID")
        self.r_discord_id.set_visible(False)

        def choice_changed(row, _param):
            webhook = row.get_selected() == 0
            self.r_webhook.set_visible(webhook)
            self.r_discord_id.set_visible(not webhook)

        self.proof_choice.connect("notify::selected", choice_changed)
        proof.add(self.proof_choice)
        proof.add(self.r_webhook)
        proof.add(self.r_discord_id)
        column.append(proof)

        self.register_button = Gtk.Button(label="Create account")
        self.register_button.add_css_class("suggested-action")
        self.register_button.add_css_class("pill")
        self.register_button.connect("clicked", lambda _b: self._register())
        column.append(self.register_button)

        back = Gtk.Button(label="Back to sign in")
        back.add_css_class("flat")
        back.connect("clicked", lambda _b: self._stack.set_visible_child_name("login"))
        column.append(back)

        page.set_child(column)
        return w.scrolled(page)

    def _register(self) -> None:
        username = self.r_username.get_text().strip()
        guild_id = self.r_guild.get_text().strip()
        password = self.r_password.get_text()
        if not (username and guild_id and password):
            w.toast(self, "Username, guild ID and password are all required.")
            return
        use_webhook = self.proof_choice.get_selected() == 0
        webhook = self.r_webhook.get_text().strip() if use_webhook else ""
        discord_id = "" if use_webhook else self.r_discord_id.get_text().strip()
        if use_webhook and not webhook:
            w.toast(self, "Paste a webhook URL from the guild you're claiming.")
            return
        if not use_webhook and not discord_id:
            w.toast(self, "Enter the Discord user ID to DM the code to.")
            return

        self._set_busy(True)
        work.run(
            lambda: api_module.register(
                username, guild_id, password,
                email=self.r_email.get_text().strip() or None,
                verification_webhook_url=webhook or None,
                discord_user_id=discord_id or None,
            ),
            on_success=self._registered,
            on_error=lambda message: w.toast(self, message),
            on_finally=lambda: self._set_busy(False),
        )

    def _registered(self, payload: dict) -> None:
        if api_module.api.token:
            self.on_signed_in(payload)
            return
        # No token yet: the backend is waiting on the verification step.
        w.toast(self, "Account created. Complete verification, then sign in.")
        self._stack.set_visible_child_name("login")


def _secret_backend() -> str:
    from ..core import secrets

    return secrets.backend_name()
