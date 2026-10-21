"""Account section -- Profile, Appearance, Other Projects.

Ported from Features/Profile. Appearance writes the same
`/api/users/preferences` the web panel does, so a change here shows up on
the web UI, the iPhone app and the Apple TV's background/accent live via
the `account` feed -- which is also why this screen listens to that feed
instead of only trusting its own last write.
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
from ...core.config import settings
from .. import theme
from .. import widgets as w
from .base import Screen

VISIBILITIES = ["public", "friends", "private"]


class ProfileScreen(Screen):
    """How you appear across the panel -- GET/POST /api/users/me."""

    fallback_interval = 0.0

    def load(self) -> Any:
        return api.get("/api/users/me")

    def render(self, payload: Any) -> Gtk.Widget:
        profile = fields(payload, "profile", "user", "account", default=payload)
        box = self.body()

        group = w.section("Profile", "Shown on your public profile page.")
        self._display_name = Adw.EntryRow(title="Display name")
        self._display_name.set_text(str(field(profile, "displayName", "")))
        self._bio = Adw.EntryRow(title="Bio")
        self._bio.set_text(str(field(profile, "bio", "")))
        self._avatar = Adw.EntryRow(title="Avatar URL")
        self._avatar.set_text(str(field(profile, "avatarUrl", "")))
        self._color = Adw.EntryRow(title="Profile colour")
        self._color.set_text(str(fields(profile, "profileColor", "color", default="")))
        for row in (self._display_name, self._bio, self._avatar, self._color):
            group.add(row)

        self._visibility = Adw.ComboRow(title="Visibility")
        model = Gtk.StringList()
        for label in ("Public", "Friends only", "Private"):
            model.append(label)
        self._visibility.set_model(model)
        current = str(field(profile, "visibility", "public"))
        self._visibility.set_selected(
            VISIBILITIES.index(current) if current in VISIBILITIES else 0
        )
        group.add(self._visibility)

        save = Gtk.Button(label="Save profile")
        save.add_css_class("suggested-action")
        save.add_css_class("pill")
        save.set_halign(Gtk.Align.START)
        save.connect("clicked", lambda _b: self._save_profile())
        group.add(save)
        box.append(group)

        box.append(self._account_group(profile))
        box.append(self._security_group(profile))
        return w.clamp_of(box)

    def _account_group(self, profile: Any) -> Gtk.Widget:
        group = w.section("Account")
        for title, key in (
            ("Username", "username"), ("Role", "role"),
            ("Guild", "guildName"), ("Guild ID", "guildId"),
            ("Email", "email"), ("Joined", "createdAt"),
        ):
            value = fields(profile, key, default=None) or field(self.window.session, key)
            if value:
                group.add(w.key_value(
                    title,
                    w.format_absolute(value) if key == "createdAt" else value,
                    copyable=key in ("guildId", "email"),
                ))
        verified = fields(profile, "emailVerified", "verified", default=None)
        if verified is not None:
            row = Adw.ActionRow(title="Email verified")
            row.add_suffix(w.pill("yes" if verified else "no",
                                  css="swarm-ok" if verified else "swarm-warn"))
            if not verified:
                resend = Gtk.Button(label="Resend")
                resend.add_css_class("flat")
                resend.set_valign(Gtk.Align.CENTER)
                resend.connect("clicked", lambda _b: self._resend_verification())
                row.add_suffix(resend)
            group.add(row)
        return group

    def _security_group(self, profile: Any) -> Gtk.Widget:
        group = w.section("Security")

        email_row = Adw.ActionRow(
            title="Change email",
            subtitle="Owner elevation requires a verified email matching the "
                     "site owner address.",
        )
        email_row.set_subtitle_lines(2)
        email_button = Gtk.Button(label="Change…")
        email_button.add_css_class("flat")
        email_button.set_valign(Gtk.Align.CENTER)
        email_button.connect("clicked", lambda _b: self._change_email())
        email_row.add_suffix(email_button)
        group.add(email_row)

        password_row = Adw.ActionRow(title="Change password")
        password_button = Gtk.Button(label="Change…")
        password_button.add_css_class("flat")
        password_button.set_valign(Gtk.Align.CENTER)
        password_button.connect("clicked", lambda _b: self._change_password())
        password_row.add_suffix(password_button)
        group.add(password_row)
        return group

    def _save_profile(self) -> None:
        body = {
            "displayName": self._display_name.get_text().strip(),
            "bio": self._bio.get_text().strip(),
            "avatarUrl": self._avatar.get_text().strip(),
            "profileColor": self._color.get_text().strip(),
            "visibility": VISIBILITIES[self._visibility.get_selected()],
        }
        self.act(lambda: api.post("/api/users/me", body), success="Profile saved.")

    def _resend_verification(self) -> None:
        self.act(
            lambda: api.post("/api/session/resend-verification"),
            success="Verification email sent.",
            then=lambda _r: None,
        )

    def _change_email(self) -> None:
        w.prompt(
            self, "Change email",
            "You'll need to verify the new address before it takes effect.",
            "you@example.com", "Save",
            lambda value: self.act(
                lambda: api.post("/api/session/email", {"email": value}),
                success="Check the new address to verify it.",
            ),
        )

    def _change_password(self) -> None:
        dialog = Adw.AlertDialog(
            heading="Change password",
            body="Enter your current password, then the new one.",
        )
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        current = Gtk.PasswordEntry(show_peek_icon=True)
        current.set_property("placeholder-text", "Current password")
        new = Gtk.PasswordEntry(show_peek_icon=True)
        new.set_property("placeholder-text", "New password")
        confirm = Gtk.PasswordEntry(show_peek_icon=True)
        confirm.set_property("placeholder-text", "Repeat new password")
        for entry in (current, new, confirm):
            box.append(entry)
        dialog.set_extra_child(box)
        dialog.add_response("cancel", "Cancel")
        dialog.add_response("go", "Change password")
        dialog.set_response_appearance("go", Adw.ResponseAppearance.SUGGESTED)
        dialog.set_close_response("cancel")

        def responded(_dialog, response):
            if response != "go":
                return
            if new.get_text() != confirm.get_text():
                w.toast(self.window, "The new passwords don't match.")
                return
            if not (current.get_text() and new.get_text()):
                w.toast(self.window, "Fill in both password fields.")
                return
            self.act(
                lambda: api.post("/api/session/password", {
                    "currentPassword": current.get_text(),
                    "newPassword": new.get_text(),
                }),
                success="Password changed.",
                then=lambda _r: None,
            )

        dialog.connect("response", responded)
        dialog.present(self.get_root())


# Panel preference enums, copied from profiles.lua's PANEL_* sets. The server
# validates every one of these and silently substitutes its default for an
# unknown value, so the client offering exactly these choices is what keeps a
# saved setting from quietly becoming something else.
PANEL_CHOICES: dict[str, tuple[str, list[str], list[str]]] = {
    "theme_mode": ("Theme", ["dark", "light", "system"],
                   ["Dark", "Light", "Follow system"]),
    "background_mode": ("Background",
                        ["default", "midnight", "aurora", "ember",
                         "custom_color", "custom_image"],
                        ["Default", "Midnight", "Aurora", "Ember",
                         "Custom colour", "Custom image"]),
    "layout_mode": ("Layout", ["standard", "focused", "wide"],
                    ["Standard", "Focused", "Wide"]),
    "density": ("Density", ["comfortable", "compact"],
                ["Comfortable", "Compact"]),
    "card_shape": ("Card shape", ["soft", "crisp"], ["Soft", "Crisp"]),
    "font_scale": ("Font scale", ["normal", "large", "dense"],
                   ["Normal", "Large", "Dense"]),
    "motion": ("Motion", ["standard", "reduced"], ["Standard", "Reduced"]),
    "operator_layout": ("Operator layout", ["command", "console", "compact"],
                        ["Command", "Console", "Compact"]),
    "roster_layout": ("Roster layout", ["cards", "signals", "ledger"],
                      ["Cards", "Signals", "Ledger"]),
    "tab_style": ("Tab style", ["rail", "underline", "minimal"],
                  ["Rail", "Underline", "Minimal"]),
    "stream_card_style": ("Bot card style", ["telemetry", "compact", "cinematic"],
                          ["Telemetry", "Compact", "Cinematic"]),
    "dashboard_density": ("Dashboard density", ["command", "dense"],
                          ["Command", "Dense"]),
}


class AppearanceScreen(Screen):
    """Theme, layout and motion -- GET/POST /api/users/preferences.

    These are *account* preferences, shared with the web panel, the iPhone
    app and the Apple TV, so the field names here are exactly the ones
    profiles.lua validates. The server also owns a set of ready-made presets
    (/api/appearance/presets), which is what the panel's "looks" are -- each
    one is a pre-validated bundle of these same choices.
    """

    fallback_interval = 0.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.presets: dict[str, list] = {"panel": [], "profile": []}
        self.preferences: dict = {}
        self._rows: dict[str, Adw.ComboRow] = {}
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        preferences = fields(api.get("/api/users/preferences"),
                             "preferences", "panelPreferences", default={}) or {}
        presets = {"panel": [], "profile": []}
        try:
            payload = api.get("/api/appearance/presets")
            presets = {
                "panel": field(payload, "panel", []) or [],
                "profile": field(payload, "profile", []) or [],
            }
        except Exception:
            # Presets are a convenience; the per-field controls below still
            # work without them.
            pass
        return {"preferences": preferences, "presets": presets}

    def apply(self, payload: Any) -> None:
        if isinstance(payload, dict) and "preferences" in payload:
            self.preferences = payload["preferences"] or {}
            self.presets = payload.get("presets") or {"panel": [], "profile": []}
            theme.apply_preferences(self.window, self.preferences)
        super().apply(payload)

    def on_account(self, account: Any) -> None:
        """The `account` feed is authoritative: another client may have just
        changed these, so adopt them rather than trusting our last write."""
        preferences = fields(account, "preferences", "panelPreferences",
                             default=None)
        if preferences:
            self.apply({"preferences": preferences, "presets": self.presets})

    # -- rendering ---------------------------------------------------------

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()
        box.append(self._local_group())
        if self.presets.get("panel"):
            box.append(self._preset_group("Panel looks", self.presets["panel"]))
        box.append(self._panel_group())
        box.append(self._numbers_group())
        if self.presets.get("profile"):
            box.append(self._preset_group("Profile looks", self.presets["profile"]))
        box.append(self._preview_group())
        return w.clamp_of(box)

    def _local_group(self) -> Gtk.Widget:
        group = w.section(
            "This computer",
            "Local to the desktop app — not shared with your other devices.",
        )
        scheme_row = Adw.ComboRow(title="Window theme")
        schemes = ["follow", "panel", "light", "dark"]
        model = Gtk.StringList()
        for label in ("Follow the system", "Follow my panel theme",
                      "Always light", "Always dark"):
            model.append(label)
        scheme_row.set_model(model)
        current = settings.color_scheme
        scheme_row.set_selected(schemes.index(current) if current in schemes else 0)
        scheme_row.connect("notify::selected",
                           lambda row, _p: self._set_scheme(schemes, row.get_selected()))
        group.add(scheme_row)

        secret_row = Adw.ActionRow(title="Token storage")
        from ...core import secrets

        if secrets.backend_name() == "Secret Service":
            secret_row.set_subtitle("Secret Service (encrypted)")
        else:
            secret_row.set_subtitle(
                "Local file (0600) — no secret service is available. Enable "
                "gnome-keyring for encrypted storage."
            )
            secret_row.set_subtitle_lines(2)
            secret_row.add_suffix(w.pill("weaker", css="swarm-warn"))
        group.add(secret_row)
        return group

    def _preset_group(self, title: str, presets: list) -> Gtk.Widget:
        group = w.section(
            title,
            "Ready-made bundles of the settings below. Applying one saves "
            "immediately.",
        )
        for preset in presets:
            name = str(fields(preset, "title", "id", default="Preset"))
            row = Adw.ActionRow(title=name)
            note = field(preset, "note")
            if note:
                row.set_subtitle(str(note))
                row.set_subtitle_lines(2)
            patch = field(preset, "patch", {}) or {}
            button = Gtk.Button(label="Apply")
            button.add_css_class("pill")
            button.set_valign(Gtk.Align.CENTER)
            button.set_sensitive(bool(patch))
            button.connect("clicked",
                           lambda _b, pa=patch, n=name: self._apply_preset(pa, n))
            row.add_suffix(button)
            group.add(row)
        return group

    def _panel_group(self) -> Gtk.Widget:
        group = w.section(
            "Panel appearance",
            "Shared with the web panel, the iPhone app and the Apple TV.",
        )
        self._rows = {}
        for key, (label, values, labels) in PANEL_CHOICES.items():
            row = Adw.ComboRow(title=label)
            model = Gtk.StringList()
            for text in labels:
                model.append(text)
            row.set_model(model)
            current = str(field(self.preferences, key, "") or "")
            if current in values:
                row.set_selected(values.index(current))
            row.connect("notify::selected",
                        lambda r, _p, k=key, v=values: self._set_choice(k, v, r.get_selected()))
            self._rows[key] = row
            group.add(row)

        self._accent = Adw.EntryRow(title="Accent colour (hex)")
        self._accent.set_text(str(field(self.preferences, "accentColor", "") or ""))
        self._accent.set_show_apply_button(True)
        self._accent.connect(
            "apply",
            lambda row: self._save({"accent_color": row.get_text().strip()}),
        )
        group.add(self._accent)

        self._background_color = Adw.EntryRow(title="Background colour (hex)")
        self._background_color.set_text(
            str(field(self.preferences, "backgroundColor", "") or ""))
        self._background_color.set_show_apply_button(True)
        self._background_color.connect(
            "apply",
            lambda row: self._save({"background_color": row.get_text().strip()}),
        )
        group.add(self._background_color)
        return group

    def _numbers_group(self) -> Gtk.Widget:
        group = w.section("Surfaces")
        # Ranges match profiles.lua's clamps exactly, so the control cannot
        # offer a value the server would silently pull back into range.
        self._opacity = Adw.SpinRow.new_with_range(0.35, 1.0, 0.01)
        self._opacity.set_title("Surface opacity")
        self._opacity.set_digits(2)
        self._opacity.set_value(_as_float(field(self.preferences, "surfaceOpacity"), 0.92))
        self._opacity.connect(
            "notify::value",
            lambda row, _p: self._save({"surface_opacity": round(row.get_value(), 2)}),
        )
        group.add(self._opacity)

        self._blur = Adw.SpinRow.new_with_range(0, 36, 1)
        self._blur.set_title("Surface blur")
        self._blur.set_value(_as_float(field(self.preferences, "surfaceBlur"), 18))
        self._blur.connect(
            "notify::value",
            lambda row, _p: self._save({"surface_blur": int(row.get_value())}),
        )
        group.add(self._blur)
        return group

    def _preview_group(self) -> Gtk.Widget:
        group = w.section("Preview", "How panel sections look with these settings.")
        group.add(w.stat_row([
            w.stat_tile("Bots", "12"),
            w.stat_tile("Playing", "4"),
            w.stat_tile("Queued", "137"),
        ]))
        row = Adw.ActionRow(title="Aria — Night Drive",
                            subtitle="Witch Knot · #music")
        row.add_prefix(w.artwork(None, size=44))
        bar = w.progress(134, 242)
        if bar is not None:
            bar.set_size_request(120, -1)
            row.add_suffix(bar)
        row.add_suffix(w.pill("playing", css="swarm-ok"))
        group.add(row)
        warn = Adw.ActionRow(title="node-7", subtitle="heartbeat stale for 3m")
        warn.add_suffix(w.pill("stale", css="swarm-warn"))
        group.add(warn)
        return group

    # -- saving ------------------------------------------------------------

    def _set_scheme(self, schemes: list[str], index: int) -> None:
        if not (0 <= index < len(schemes)):
            return
        settings.color_scheme = schemes[index]
        theme.apply_color_scheme()

    def _set_choice(self, key: str, values: list[str], index: int) -> None:
        if not (0 <= index < len(values)):
            return
        if str(field(self.preferences, key, "") or "") == values[index]:
            return
        self._save({key: values[index]})

    def _apply_preset(self, patch: Any, name: str) -> None:
        if not patch:
            return
        self._save(dict(patch), success=f"Applied “{name}”.")

    def _save(self, patch: dict, success: str = "Appearance saved.") -> None:
        """Send only the changed fields.

        profiles.lua merges the body onto the stored preferences key by key,
        so a partial patch is the correct shape -- sending the whole object
        would overwrite settings this screen does not expose.
        """
        def done(result: Any) -> None:
            preferences = fields(result, "preferences", "panelPreferences",
                                 default=None)
            if preferences:
                self.preferences = preferences
                theme.apply_preferences(self.window, preferences)
                self.apply({"preferences": preferences, "presets": self.presets})
            else:
                self.refresh(show_spinner=False)
            w.toast(self.window, success)

        self.act(lambda: api.post("/api/users/preferences", patch), then=done)


def _as_float(value: Any, default: float) -> float:
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


class OtherProjectsScreen(Screen):
    """Other things I've built -- the panel's Other Projects page."""

    fallback_interval = 0.0

    def load(self) -> Any:
        # There is no dedicated projects endpoint; the panel renders a fixed
        # list, and only Lumisound has a live download behind the API.
        return {}

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()

        group = w.section("Projects")
        lumisound = Adw.ActionRow(
            title="Lumisound",
            subtitle="iOS app with its own accounts and audio library.",
        )
        download = Gtk.Button(label="Download")
        download.add_css_class("pill")
        download.set_valign(Gtk.Align.CENTER)
        download.connect("clicked", lambda _b: self._download_lumisound())
        lumisound.add_suffix(download)
        group.add(lumisound)

        gallery = Adw.ActionRow(
            title="Image Gallery",
            subtitle="Media sharing with moderation, reports and age gating.",
        )
        group.add(gallery)

        panel = Adw.ActionRow(
            title="SwarmPanel",
            subtitle="This panel — web, iPhone, Apple TV and now Linux.",
        )
        open_web = Gtk.Button(icon_name="adw-external-link-symbolic")
        open_web.add_css_class("flat")
        open_web.set_valign(Gtk.Align.CENTER)
        open_web.set_tooltip_text("Open the web panel")
        open_web.connect(
            "clicked",
            lambda _b: Gtk.UriLauncher(uri=api.url_for("/")).launch(),
        )
        panel.add_suffix(open_web)
        group.add(panel)
        box.append(group)

        contact = w.section("Contact")
        discord = Adw.ActionRow(title="Discord", subtitle="HeavenlyXenusVR")
        discord_button = Gtk.Button(icon_name="adw-external-link-symbolic")
        discord_button.add_css_class("flat")
        discord_button.set_valign(Gtk.Align.CENTER)
        discord_button.connect(
            "clicked",
            lambda _b: Gtk.UriLauncher(
                uri="https://discord.com/users/1304564041863266347"
            ).launch(),
        )
        discord.add_suffix(discord_button)
        contact.add(discord)
        box.append(contact)
        return w.clamp_of(box)

    def _download_lumisound(self) -> None:
        """The download is an authenticated endpoint, so it can't just be
        handed to the browser -- fetch it with the bearer token, then save."""

        def fetch() -> bytes:
            return api.download_raw("/api/projects/lumisound/download")

        def save(data: bytes) -> None:
            dialog = Gtk.FileDialog()
            dialog.set_initial_name("Lumisound.ipa")

            def chosen(dlg, result):
                try:
                    file = dlg.save_finish(result)
                except Exception:
                    return  # the user cancelled
                try:
                    with open(file.get_path(), "wb") as handle:
                        handle.write(data)
                except OSError as error:
                    w.toast(self.window, f"Couldn't save: {error}")
                    return
                w.toast(self.window, f"Saved to {file.get_path()}")

            dialog.save(self.get_root(), None, chosen)

        w.toast(self.window, "Downloading…", timeout=2)
        work.run(fetch, on_success=save,
                 on_error=lambda message: w.toast(self.window, message))
