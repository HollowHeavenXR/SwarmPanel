"""Admin section -- Overview, Diagnostics, Intel, Audit Log, Accounts,
Databases, Gallery, Lumisound.

Ported from Features/Admin. Two guardrails are carried over deliberately:

* Every destructive action is behind a confirmation, and the database
  truncates behind a *type-the-phrase* confirmation plus a second owner
  phrase. Both phrases are validated server-side regardless of what this
  client sends, so the local check is a mis-click guard, not the authority.

* Screens whose payload shape isn't worth hard-coding (system diagnostics,
  stability, metrics) render the parts they recognise and fall back to a
  readable raw payload, rather than guessing at field names that could
  silently drift. Same choice the web panel's JsonPanel makes.
"""

from __future__ import annotations

from typing import Any

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")

from gi.repository import Adw, Gtk

from ...core import nav, work
from ...core.api import api
from ...core.casing import field, fields, unwrap
from .. import widgets as w
from .base import Screen


def _as_int(value: Any) -> int:
    try:
        return int(float(value))
    except (TypeError, ValueError):
        return 0


def _generic_sections(box: Gtk.Box, payload: Any, skip: set[str]) -> bool:
    """Render a free-form payload's lists and objects. Returns whether
    anything was shown."""
    rendered = False
    for key, value in (payload.items() if isinstance(payload, dict) else []):
        if key in skip or key == "ok":
            continue
        title = str(key).replace("_", " ").title()
        if isinstance(value, list) and value and isinstance(value[0], dict):
            box.append(w.heading(title, level=3))
            box.append(w.rows_to_table(value[:200]))
            rendered = True
        elif isinstance(value, dict) and value:
            group = w.section(title)
            for inner_key, inner_value in list(value.items())[:30]:
                group.add(w.key_value(str(inner_key).replace("_", " ").title(),
                                      inner_value))
            box.append(group)
            rendered = True
        elif isinstance(value, (str, int, float, bool)):
            group = w.section(title)
            group.add(w.key_value(title, value))
            box.append(group)
            rendered = True
    return rendered


class OverviewScreen(Screen):
    """Admin hub -- every admin tool in one place, plus a live summary."""

    fallback_interval = 30.0

    def load(self) -> Any:
        return api.get("/api/admin/overview")

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()

        summary = fields(payload, "summary", "counts", "totals", default=None)
        if isinstance(summary, dict) and summary:
            box.append(w.stat_row([
                w.stat_tile(str(key).replace("_", " ").title(), w.cell_text(value))
                for key, value in list(summary.items())[:5]
            ]))

        # The tool grid comes from the same nav definition the sidebar uses,
        # so a new admin screen appears here automatically.
        admin_section = next(
            (s for s in nav.visible_sections(self.window.session) if s.key == "admin"),
            None,
        )
        if admin_section is not None:
            group = w.section("Tools", admin_section.blurb)
            for item in admin_section.items:
                if item.to == "/admin":
                    continue
                row = Adw.ActionRow(title=item.label, subtitle=item.blurb)
                row.add_prefix(Gtk.Image(icon_name=item.icon))
                row.set_activatable(True)
                row.connect("activated", lambda _r, to=item.to: self.window.navigate(to))
                row.add_suffix(Gtk.Image(icon_name="go-next-symbolic"))
                group.add(row)
            box.append(group)

        _generic_sections(box, payload, skip={"summary", "counts", "totals"})
        return w.clamp_of(box)


class DiagnosticsScreen(Screen):
    """Stability, metrics, alert rules and exports -- the `stability` and
    `metrics_snapshot` watch keys plus their REST endpoints."""

    watch_key = "stability"
    fallback_interval = 20.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.stability: Any = None
        self.metrics: Any = None
        self.diagnostics: Any = None
        self.alert_rules: list[Any] = []
        self.exports: Any = None
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        def safe(path: str) -> Any:
            # One failing sub-panel must not blank the whole screen.
            try:
                return api.get(path)
            except Exception:
                return None

        return {
            "stability": safe("/api/stability"),
            "metrics": safe("/api/metrics"),
            "diagnostics": safe("/api/system-diagnostics"),
            "alertRules": safe("/api/alert-rules"),
            "exports": safe("/api/exports"),
        }

    def apply(self, payload: Any) -> None:
        if isinstance(payload, dict) and "stability" in payload and "metrics" in payload:
            self.stability = payload["stability"]
            self.metrics = payload["metrics"]
            self.diagnostics = payload["diagnostics"]
            self.alert_rules = field(payload["alertRules"], "rules", []) or []
            self.exports = payload["exports"]
        super().apply(payload)

    def on_live_snapshot(self, payload: Any) -> None:
        self.stability = payload
        super().apply(self.payload)

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()

        box.append(w.heading("Stability", level=3))
        if self.stability:
            stability_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
            if not _generic_sections(stability_box, self.stability, skip=set()):
                stability_box.append(w.monospace(w.pretty_json(self.stability)))
            box.append(stability_box)
        else:
            box.append(w.dim("Stability data unavailable."))

        box.append(w.heading("Metrics", level=3))
        if self.metrics:
            metrics_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
            if not _generic_sections(metrics_box, self.metrics, skip=set()):
                metrics_box.append(w.monospace(w.pretty_json(self.metrics)))
            box.append(metrics_box)
        else:
            box.append(w.dim("Metrics unavailable."))

        box.append(self._alert_rules_group())
        box.append(self._exports_group())

        if self.diagnostics:
            box.append(w.heading("System diagnostics", level=3))
            box.append(w.monospace(w.pretty_json(self.diagnostics)))
        return w.clamp_of(box)

    def _alert_rules_group(self) -> Gtk.Widget:
        group = w.section("Alert rules",
                          "Scoped Telegram operator alerts for panel health.")
        if not self.alert_rules:
            group.add(Adw.ActionRow(title="No alert rules configured"))
            return group
        for rule in self.alert_rules:
            name = str(fields(rule, "name", "metric", "id", default="rule"))
            row = Adw.ActionRow(title=name)
            bits = []
            for key in ("metric", "comparison", "threshold", "window", "scope"):
                value = field(rule, key)
                if value is not None:
                    bits.append(f"{key} {value}")
            row.set_subtitle(" · ".join(bits) or w.DASH)
            row.set_subtitle_lines(2)
            enabled = field(rule, "enabled")
            if enabled is not None:
                toggle = Gtk.Switch()
                toggle.set_valign(Gtk.Align.CENTER)
                toggle.set_active(bool(enabled))
                toggle.connect(
                    "state-set",
                    lambda _s, state, r=rule: self._set_rule_enabled(r, state),
                )
                row.add_suffix(toggle)
            if field(rule, "id") is not None:
                delete = Gtk.Button(icon_name="user-trash-symbolic")
                delete.add_css_class("flat")
                delete.set_valign(Gtk.Align.CENTER)
                delete.set_tooltip_text("Delete this rule")
                delete.connect("clicked", lambda _b, r=rule: self._delete_rule(r))
                row.add_suffix(delete)
            group.add(row)
        return group

    def _set_rule_enabled(self, rule: Any, enabled: bool) -> bool:
        # Addressed by path. POST /api/alert-rules is *create*; updating an
        # existing rule goes to /api/alert-rules/:rule_id/update, which
        # applies whichever fields the body carries.
        rule_id = field(rule, "id")
        if rule_id is None:
            w.toast(self.window, "That rule has no id, so it can't be changed.")
            return False
        self.act(
            lambda: api.post(f"/api/alert-rules/{rule_id}/update",
                             {"enabled": bool(enabled)}),
            success=f"Rule {'enabled' if enabled else 'disabled'}.",
            then=lambda _r: None,
        )
        return False  # let the switch adopt the state it was set to

    def _delete_rule(self, rule: Any) -> None:
        rule_id = field(rule, "id")
        if rule_id is None:
            return
        name = str(fields(rule, "name", "metric", default="this rule"))
        w.confirm(
            self, "Delete alert rule?", f"“{name}” will stop firing.", "Delete",
            lambda: self.act(
                lambda: api.post(f"/api/alert-rules/{rule_id}/delete"),
                success="Rule deleted.",
            ),
        )

    def _exports_group(self) -> Gtk.Widget:
        group = w.section("Exports", "Scheduled CSV snapshots.")
        enabled = field(self.exports, "enabled")
        if enabled is not None:
            row = Adw.ActionRow(title="Scheduled exports")
            row.add_suffix(w.pill("on" if enabled else "off",
                                  css="swarm-ok" if enabled else ""))
            group.add(row)
        snapshots = field(self.exports, "snapshots", []) or []
        if not snapshots:
            group.add(Adw.ActionRow(title="No snapshots yet"))
            return group
        for snapshot in snapshots[:30]:
            files = field(snapshot, "files", []) or []
            row = Adw.ExpanderRow(title=str(field(snapshot, "date", w.DASH)))
            row.set_subtitle(f"{len(files)} file(s)")
            for name in files:
                group_row = Adw.ActionRow(title=str(name))
                row.add_row(group_row)
            group.add(row)
        return group


class IntelScreen(Screen):
    """Trends, anomalies and raw events -- the `events` watch key plus
    /api/metrics/history and /api/metrics/anomalies."""

    watch_key = "events"
    fallback_interval = 20.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.events: list[Any] = []
        self.history: Any = None
        self.anomalies: list[Any] = []
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        def safe(path: str, query: dict | None = None) -> Any:
            try:
                return api.get(path, query)
            except Exception:
                return None

        return {
            "events": safe("/api/events", {"limit": "200"}),
            "history": safe("/api/metrics/history"),
            "anomalies": safe("/api/metrics/anomalies"),
        }

    def apply(self, payload: Any) -> None:
        if isinstance(payload, dict) and "events" in payload and "history" in payload:
            self.events = field(payload["events"], "events", []) or []
            self.history = payload["history"]
            self.anomalies = field(payload["anomalies"], "anomalies", []) or []
        super().apply(payload)

    def on_live_snapshot(self, payload: Any) -> None:
        self.events = field(payload, "events", []) or []
        super().apply(self.payload)

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()

        if self.anomalies:
            box.append(w.heading("Anomalies", level=3))
            box.append(w.rows_to_table(self.anomalies[:100]))
        else:
            box.append(w.heading("Anomalies", level=3))
            box.append(w.dim("Nothing anomalous right now."))

        if self.history:
            # The payload is {metric, hours, points:[{metric_value,
            # captured_at}]} -- a bare table of numbers says nothing without
            # naming the metric and its window, so lead with those.
            metric = str(field(self.history, "metric", "") or "metric")
            hours = _as_int(field(self.history, "hours", 0))
            points = fields(self.history, "points", "series", "metrics",
                            default=[]) or []
            box.append(w.heading(
                f"{metric.replace('_', ' ').title()} trend", level=3))
            values = []
            for point in points:
                try:
                    values.append(float(fields(point, "metricValue", "value",
                                               default=None)))
                except (TypeError, ValueError):
                    pass
            if values:
                box.append(w.stat_row([
                    w.stat_tile("Latest", w.format_count(values[-1])),
                    w.stat_tile("Low", w.format_count(min(values))),
                    w.stat_tile("High", w.format_count(max(values))),
                    w.stat_tile("Mean", w.format_count(sum(values) / len(values))),
                    w.stat_tile("Samples", w.format_count(len(values)),
                                f"over {hours}h" if hours else ""),
                ]))
            if isinstance(points, list) and points and isinstance(points[0], dict):
                box.append(w.rows_to_table(points[-60:]))
            else:
                box.append(w.monospace(w.pretty_json(self.history)))

        box.append(w.heading("Events", level=3))
        if self.events:
            box.append(w.rows_to_table(self.events[:300]))
        else:
            box.append(w.dim("No recent events."))
        return w.clamp_of(box)


class AuditLogScreen(Screen):
    """Every recorded admin action -- GET /api/audit-log."""

    fallback_interval = 30.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.query = ""
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        # Enveloped: {"ok": true, "data": {"entries": [...], "total": n}}
        return unwrap(api.get("/api/audit-log",
                              {"limit": "300", "q": self.query or None}))

    def render(self, payload: Any) -> Gtk.Widget:
        entries = fields(payload, "entries", "log", "rows", default=[]) or []
        box = self.body()

        search = Gtk.SearchEntry()
        search.set_placeholder_text("Filter the audit log…")
        search.set_text(self.query)
        search.connect("search-changed", self._on_search)
        group = Adw.PreferencesGroup()
        group.add(search)
        box.append(group)

        if not entries:
            box.append(w.empty_state("Nothing logged",
                                     "No admin actions match this filter."
                                     if self.query else "No admin actions recorded.",
                                     "view-list-symbolic"))
            return w.clamp_of(box)

        group = w.section("Entries", f"{len(entries)} recorded action(s)")
        for entry in entries:
            group.add(self._entry_row(entry))
        box.append(group)
        return w.clamp_of(box)

    def _entry_row(self, entry: Any) -> Gtk.Widget:
        action = str(fields(entry, "action", "kind", "event", default=w.DASH))
        row = Adw.ExpanderRow(title=action)
        when = fields(entry, "createdAt", "at", "timestamp", default=None)
        # The column is `actor_username`; the shorter spellings are kept as
        # fallbacks for other payloads that carry the same idea.
        actor = fields(entry, "actorUsername", "actor", "username",
                       "accountId", default=w.DASH)
        row.set_subtitle(f"{actor} · {w.format_relative(when)}")

        target = fields(entry, "targetId", "target", "subject", default=None)
        if target:
            row.add_row(w.key_value("Target", target, copyable=True))
        target_type = field(entry, "targetType")
        if target_type:
            row.add_row(w.key_value("Target type", target_type))
        if when:
            row.add_row(w.key_value("When", w.format_absolute(when)))
        details = fields(entry, "details", "detail", "payload", "message",
                         default=None)
        if details:
            row.add_row(w.key_value("Details", details, copyable=True))

        # Only some actions are revertible, and only when a before-state was
        # recorded; the server is the authority, so the button is always
        # offered and its refusal is surfaced as the message it gives.
        entry_id = fields(entry, "id", "entryId", default=None)
        if entry_id is not None:
            revert_row = Adw.ActionRow(
                title="Revert this action",
                subtitle="Only possible for revertible actions that recorded "
                         "a before-state.",
            )
            revert_row.set_subtitle_lines(2)
            button = Gtk.Button(label="Revert")
            button.add_css_class("destructive-action")
            button.set_valign(Gtk.Align.CENTER)
            button.connect("clicked",
                           lambda _b, e=entry_id, a=action: self._revert(e, a))
            revert_row.add_suffix(button)
            row.add_row(revert_row)
        return row

    def _revert(self, entry_id: Any, action: str) -> None:
        w.confirm(
            self, "Revert this action?",
            f"The panel will try to undo “{action}” by restoring the "
            f"before-state recorded with it.",
            "Revert",
            lambda: self.act(
                lambda: api.post(f"/api/audit-log/{entry_id}/revert"),
                success="Action reverted.",
            ),
        )

    def _on_search(self, entry: Gtk.SearchEntry) -> None:
        self.query = entry.get_text().strip()
        self.refresh(show_spinner=False)


class AccountsScreen(Screen):
    """Recover and manage swarm accounts -- /api/swarm-accounts/*.

    Every mutation here is confirmed first; the bulk ones name the count so
    a mis-click on "delete" can't quietly take out a page of accounts.
    """

    fallback_interval = 0.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.query = ""
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        # Enveloped: {"ok": true, "data": {"users": [...], "summary": {...}}}
        return unwrap(api.get("/api/swarm-accounts/admin",
                              {"q": self.query or None, "limit": "200"}))

    def render(self, payload: Any) -> Gtk.Widget:
        accounts = fields(payload, "users", "accounts", "rows", default=[]) or []
        box = self.body()

        search = Gtk.SearchEntry()
        search.set_placeholder_text("Search accounts…")
        search.set_text(self.query)
        search.connect("search-changed", self._on_search)
        group = Adw.PreferencesGroup()
        group.add(search)
        box.append(group)

        if not accounts:
            box.append(w.empty_state("No accounts found", "", "system-users-symbolic"))
            return w.clamp_of(box)

        listing = w.section("Accounts", f"{len(accounts)} account(s)")
        for account in accounts:
            listing.add(self._account_row(account))
        box.append(listing)
        return w.clamp_of(box)

    def _account_row(self, account: Any) -> Gtk.Widget:
        account_id = fields(account, "id", "accountId", default=None)
        username = str(fields(account, "username", "displayName", default=w.DASH))
        row = Adw.ExpanderRow(title=username)
        bits = []
        for key in ("email", "role", "guildId"):
            value = field(account, key)
            if value:
                bits.append(str(value))
        row.set_subtitle(" · ".join(bits) or w.DASH)

        verified = fields(account, "emailVerified", "verified", default=None)
        if verified is not None:
            row.add_suffix(w.pill("verified" if verified else "unverified",
                                  css="swarm-ok" if verified else "swarm-warn"))
        if field(account, "moderator"):
            row.add_suffix(w.pill("mod", css="swarm-ok"))

        if account_id is None:
            row.add_row(Adw.ActionRow(
                title="No account id",
                subtitle="This row can't be acted on.",
            ))
            return row

        actions = [
            ("Toggle moderator", "/api/swarm-accounts/moderator",
             {"accountId": account_id, "moderator": not bool(field(account, "moderator"))},
             "Moderator flag updated.", False),
            ("Mark email verified", "/api/swarm-accounts/email-verified",
             {"accountId": account_id, "verified": True},
             "Marked verified.", False),
            ("Resend verification", "/api/swarm-accounts/resend-verification",
             {"accountId": account_id}, "Verification email sent.", False),
            ("Reset password", "/api/swarm-accounts/reset-password",
             {"accountId": account_id}, "Password reset issued.", True),
            ("Delete account", "/api/swarm-accounts/delete",
             {"accountId": account_id}, "Account deleted.", True),
        ]
        for label, path, body, success, destructive in actions:
            action_row = Adw.ActionRow(title=label)
            button = Gtk.Button(label=label.split()[0])
            button.add_css_class("flat")
            if destructive:
                button.add_css_class("destructive-action")
            button.set_valign(Gtk.Align.CENTER)
            button.connect(
                "clicked",
                lambda _b, p=path, b=body, s=success, d=destructive, l=label:
                    self._run(l, p, b, s, d, username),
            )
            action_row.add_suffix(button)
            row.add_row(action_row)

        edit_row = Adw.ActionRow(title="Change username")
        edit_button = Gtk.Button(label="Edit")
        edit_button.add_css_class("flat")
        edit_button.set_valign(Gtk.Align.CENTER)
        edit_button.connect(
            "clicked",
            lambda _b, a=account_id, u=username: w.prompt(
                self, "Change username", "", u, "Save",
                lambda value: self.act(
                    lambda: api.post("/api/swarm-accounts/update",
                                     {"accountId": a, "username": value}),
                    success="Account updated.",
                ),
                initial=u,
            ),
        )
        edit_row.add_suffix(edit_button)
        row.add_row(edit_row)
        return row

    def _run(self, label: str, path: str, body: dict, success: str,
             destructive: bool, username: str) -> None:
        def fire() -> Any:
            return api.post(path, body)

        if destructive:
            w.confirm(
                self, f"{label}?", f"This applies to {username}.", label,
                lambda: self.act(fire, success=success),
            )
            return
        self.act(fire, success=success)

    def _on_search(self, entry: Gtk.SearchEntry) -> None:
        self.query = entry.get_text().strip()
        self.refresh(show_spinner=False)


class DatabasesScreen(Screen):
    """Browse raw schema tables, and the guarded truncate paths.

    Truncates require the exact phrase the backend expects --
    `TRUNCATE {schema}.{table}`, or `TRUNCATE ALL {schema}` for a whole
    schema -- plus a second owner phrase. Both are re-checked server-side.
    """

    fallback_interval = 0.0

    def __init__(self, window: Any, **kwargs) -> None:
        self.schemas: list[Any] = []
        self.schema = ""
        self.table = ""
        self.rows: list[Any] = []
        self.row_count: int | None = None
        super().__init__(window, **kwargs)

    def load(self) -> Any:
        schemas = field(api.get("/api/databases", {"include_tables": "true"}),
                        "schemas", []) or []
        rows: list[Any] = []
        count = None
        if self.schema and self.table:
            data = api.get("/api/database/data",
                           {"schema_name": self.schema, "table_name": self.table,
                            "limit": "200"})
            rows = field(data, "rows", []) or []
            count = field(data, "count")
        return {"schemas": schemas, "rows": rows, "count": count}

    def apply(self, payload: Any) -> None:
        if isinstance(payload, dict) and "schemas" in payload:
            self.schemas = payload["schemas"]
            self.rows = payload["rows"]
            self.row_count = payload["count"]
        super().apply(payload)

    @staticmethod
    def _table_names(schema_entry: Any) -> list[str]:
        """Table entries aren't a fixed shape server-side -- mirrors the
        web's tableName() helper checking the same key names, falling back
        to a bare string."""
        names: list[str] = []
        for entry in (field(schema_entry, "tables", []) or []):
            if isinstance(entry, str):
                name = entry
            elif isinstance(entry, dict):
                name = (entry.get("name") or entry.get("table_name")
                        or entry.get("TABLE_NAME") or "")
            else:
                name = ""
            if name:
                names.append(str(name))
        return names

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()

        warning = Adw.Banner(title="Destructive actions here are irreversible.")
        warning.set_revealed(True)
        box.append(warning)

        picker = w.section("Browse")
        schema_names = [str(field(s, "schema", "")) for s in self.schemas]
        schema_row = Adw.ComboRow(title="Schema")
        schema_model = Gtk.StringList()
        for name in schema_names:
            schema_model.append(name)
        schema_row.set_model(schema_model)
        if self.schema in schema_names:
            schema_row.set_selected(schema_names.index(self.schema))
        schema_row.connect("notify::selected",
                           lambda row, _p: self._choose_schema(schema_names,
                                                               row.get_selected()))
        picker.add(schema_row)

        tables = []
        for entry in self.schemas:
            if str(field(entry, "schema", "")) == self.schema:
                tables = self._table_names(entry)
                break
        if tables:
            table_row = Adw.ComboRow(title="Table")
            table_model = Gtk.StringList()
            for name in tables:
                table_model.append(name)
            table_row.set_model(table_model)
            if self.table in tables:
                table_row.set_selected(tables.index(self.table))
            table_row.connect("notify::selected",
                              lambda row, _p: self._choose_table(tables,
                                                                 row.get_selected()))
            picker.add(table_row)
        box.append(picker)

        if not (self.schema and self.table):
            box.append(w.empty_state("Pick a schema and table",
                                     "Choose above to browse rows.",
                                     "drive-harddisk-symbolic"))
            return w.clamp_of(box)

        header = f"{self.schema}.{self.table}"
        if self.row_count is not None:
            header += f" — {w.format_count(self.row_count)} row(s)"
        box.append(w.heading(header, level=3))
        box.append(w.rows_to_table(self.rows))

        danger = w.section(
            "Destructive",
            "Both of these require typing an exact phrase, and are re-checked "
            "by the server.",
        )
        truncate_table = Adw.ActionRow(
            title=f"Truncate {self.schema}.{self.table}",
            subtitle="Deletes every row in this table.",
        )
        table_button = Gtk.Button(label="Truncate table")
        table_button.add_css_class("destructive-action")
        table_button.set_valign(Gtk.Align.CENTER)
        table_button.connect("clicked", lambda _b: self._truncate_table())
        truncate_table.add_suffix(table_button)
        danger.add(truncate_table)

        truncate_schema = Adw.ActionRow(
            title=f"Truncate every table in {self.schema}",
            subtitle="Deletes every row in every table of this schema.",
        )
        schema_button = Gtk.Button(label="Truncate schema")
        schema_button.add_css_class("destructive-action")
        schema_button.set_valign(Gtk.Align.CENTER)
        schema_button.connect("clicked", lambda _b: self._truncate_schema())
        truncate_schema.add_suffix(schema_button)
        danger.add(truncate_schema)
        box.append(danger)
        return w.clamp_of(box)

    def _choose_schema(self, names: list[str], index: int) -> None:
        if 0 <= index < len(names) and names[index] != self.schema:
            self.schema = names[index]
            self.table = ""
            self.rows = []
            self.row_count = None
            self.refresh(show_spinner=True)

    def _choose_table(self, names: list[str], index: int) -> None:
        if 0 <= index < len(names) and names[index] != self.table:
            self.table = names[index]
            self.refresh(show_spinner=True)

    def _truncate_table(self) -> None:
        phrase = f"TRUNCATE {self.schema}.{self.table}"
        w.confirm_phrase(
            self, "Truncate table?",
            f"Every row in {self.schema}.{self.table} will be deleted. This "
            f"cannot be undone.",
            phrase, "Truncate table",
            lambda: None,
            extra_label="Owner confirmation phrase",
            on_confirm_extra=lambda owner: self.act(
                lambda: api.post("/api/database/truncate-table", {
                    "schemaName": self.schema, "tableName": self.table,
                    "confirmText": phrase, "ownerConfirmText": owner,
                }),
                success="Table truncated.",
            ),
        )

    def _truncate_schema(self) -> None:
        phrase = f"TRUNCATE ALL {self.schema}"
        w.confirm_phrase(
            self, "Truncate every table?",
            f"Every row in every table of {self.schema} will be deleted. This "
            f"cannot be undone.",
            phrase, "Truncate schema",
            lambda: None,
            extra_label="Owner confirmation phrase",
            on_confirm_extra=lambda owner: self.act(
                lambda: api.post("/api/database/truncate-schema", {
                    "schemaName": self.schema,
                    "confirmText": phrase, "ownerConfirmText": owner,
                }),
                success="Schema truncated.",
            ),
        )


class GalleryScreen(Screen):
    """Image Gallery users, media and reports -- /api/image-gallery/admin."""

    fallback_interval = 0.0

    def load(self) -> Any:
        # Enveloped, and carries its own summary counts.
        return unwrap(api.get("/api/image-gallery/admin", {"limit": "200"}))

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()
        users = fields(payload, "users", default=[]) or []
        media = fields(payload, "media", default=[]) or []
        reports = fields(payload, "reports", default=[]) or []
        comments = fields(payload, "comments", default=[]) or []

        box.append(w.stat_row([
            w.stat_tile("Users", w.format_count(len(users))),
            w.stat_tile("Media", w.format_count(len(media))),
            w.stat_tile("Reports", w.format_count(len(reports))),
            w.stat_tile("Comments", w.format_count(len(comments))),
        ]))

        exports = w.section("Exports")
        for label, path, filename in (
            ("Export users CSV", "/api/image-gallery/admin/users/export.csv",
             "gallery-users.csv"),
            ("Export media CSV", "/api/image-gallery/admin/media/export.csv",
             "gallery-media.csv"),
        ):
            row = Adw.ActionRow(title=label)
            button = Gtk.Button(label="Download")
            button.add_css_class("flat")
            button.set_valign(Gtk.Align.CENTER)
            button.connect("clicked",
                           lambda _b, p=path, f=filename: self._export(p, f))
            row.add_suffix(button)
            exports.add(row)
        box.append(exports)

        if reports:
            box.append(w.heading("Reports", level=3))
            group = w.section("")
            for report in reports[:100]:
                report_id = fields(report, "id", "reportId", default=None)
                row = Adw.ActionRow(
                    title=str(fields(report, "reason", "kind", default="Report")),
                )
                row.set_subtitle(w.cell_text(
                    fields(report, "detail", "note", "mediaId", default="")))
                status = field(report, "status")
                if status:
                    row.add_suffix(w.pill(str(status), css=w.status_css(status)))
                if report_id is not None:
                    for label, value in (("Resolve", "resolved"),
                                         ("Dismiss", "dismissed")):
                        button = Gtk.Button(label=label)
                        button.add_css_class("flat")
                        button.set_valign(Gtk.Align.CENTER)
                        button.connect(
                            "clicked",
                            lambda _b, r=report_id, v=value: self.act(
                                lambda: api.post("/api/image-gallery/reports/status",
                                                 {"reportId": r, "status": v}),
                                success=f"Report {v}.",
                            ),
                        )
                        row.add_suffix(button)
                group.add(row)
            box.append(group)

        if users:
            box.append(w.heading("Users", level=3))
            group = w.section("")
            for user in users[:100]:
                group.add(self._user_row(user))
            box.append(group)

        if media:
            box.append(w.heading("Media", level=3))
            group = w.section("")
            for item in media[:100]:
                media_id = fields(item, "id", "mediaId", default=None)
                row = Adw.ActionRow(
                    title=str(fields(item, "title", "filename", "id",
                                     default=w.DASH)),
                    subtitle=w.cell_text(fields(item, "username", "ownerUsername",
                                                "userId", default="")),
                )
                if media_id is not None:
                    delete = Gtk.Button(label="Delete")
                    delete.add_css_class("flat")
                    delete.add_css_class("destructive-action")
                    delete.set_valign(Gtk.Align.CENTER)
                    delete.connect(
                        "clicked",
                        lambda _b, m=media_id: w.confirm(
                            self, "Delete media?", "This cannot be undone.",
                            "Delete",
                            lambda: self.act(
                                lambda: api.post("/api/image-gallery/media/delete",
                                                 {"mediaId": m}),
                                success="Media deleted.",
                            ),
                        ),
                    )
                    row.add_suffix(delete)
                group.add(row)
            box.append(group)

        if not (users or media or reports or comments):
            box.append(w.empty_state("Nothing to moderate",
                                     "The gallery has no users or media yet.",
                                     "image-x-generic-symbolic"))
        return w.clamp_of(box)

    def _user_row(self, user: Any) -> Gtk.Widget:
        user_id = fields(user, "id", "userId", default=None)
        row = Adw.ExpanderRow(
            title=str(fields(user, "username", "email", default=w.DASH)))
        row.set_subtitle(w.cell_text(field(user, "email", "")))
        if field(user, "ageVerified"):
            row.add_suffix(w.pill("age ok", css="swarm-ok"))
        if field(user, "emailVerified"):
            row.add_suffix(w.pill("email ok", css="swarm-ok"))
        if user_id is None:
            return row
        for label, path, body, success, destructive in (
            ("Mark age verified", "/api/image-gallery/users/age-verified",
             {"userId": user_id, "verified": True}, "Marked age verified.", False),
            ("Mark email verified", "/api/image-gallery/users/email-verified",
             {"userId": user_id, "verified": True}, "Marked email verified.", False),
            ("Resend verification", "/api/image-gallery/users/resend-verification",
             {"userId": user_id}, "Verification sent.", False),
            ("Reset password", "/api/image-gallery/users/reset-password",
             {"userId": user_id}, "Password reset issued.", True),
            ("Delete user", "/api/image-gallery/users/delete",
             {"userId": user_id}, "User deleted.", True),
        ):
            action_row = Adw.ActionRow(title=label)
            button = Gtk.Button(label=label.split()[0])
            button.add_css_class("flat")
            if destructive:
                button.add_css_class("destructive-action")
            button.set_valign(Gtk.Align.CENTER)
            button.connect(
                "clicked",
                lambda _b, p=path, b=body, s=success, d=destructive, l=label:
                    self._maybe_confirm(l, p, b, s, d),
            )
            action_row.add_suffix(button)
            row.add_row(action_row)
        return row

    def _maybe_confirm(self, label: str, path: str, body: dict, success: str,
                       destructive: bool) -> None:
        def fire() -> Any:
            return api.post(path, body)

        if destructive:
            w.confirm(self, f"{label}?", "This cannot be undone.", label,
                      lambda: self.act(fire, success=success))
            return
        self.act(fire, success=success)

    def _export(self, path: str, filename: str) -> None:
        def save(data: bytes) -> None:
            dialog = Gtk.FileDialog()
            dialog.set_initial_name(filename)

            def chosen(dlg, result):
                try:
                    file = dlg.save_finish(result)
                except Exception:
                    return
                try:
                    with open(file.get_path(), "wb") as handle:
                        handle.write(data)
                except OSError as error:
                    w.toast(self.window, f"Couldn't save: {error}")
                    return
                w.toast(self.window, f"Saved to {file.get_path()}")

            dialog.save(self.get_root(), None, chosen)

        work.run(lambda: api.download_raw(path), on_success=save,
                 on_error=lambda message: w.toast(self.window, message))


class LumisoundScreen(Screen):
    """Lumisound accounts, uploads and bug reports -- the `lumisound_admin`
    watch key plus /api/lumisound/admin."""

    watch_key = "lumisound_admin"
    fallback_interval = 30.0

    def load(self) -> Any:
        return unwrap(api.get("/api/lumisound/admin", {"limit": "200"}))

    def render(self, payload: Any) -> Gtk.Widget:
        box = self.body()
        users = fields(payload, "users", "accounts", default=[]) or []
        uploads = fields(payload, "uploads", "library", default=[]) or []
        bugs = fields(payload, "bugReports", "bugs", default=[]) or []
        summary = field(payload, "summary")

        box.append(w.stat_row([
            w.stat_tile("Users", w.format_count(len(users))),
            w.stat_tile("Uploads", w.format_count(len(uploads))),
            w.stat_tile("Bug reports", w.format_count(len(bugs))),
        ]))

        if bugs:
            box.append(w.heading("Bug reports", level=3))
            group = w.section("")
            for bug in bugs[:100]:
                bug_id = fields(bug, "id", "reportId", default=None)
                row = Adw.ActionRow(
                    title=str(fields(bug, "title", "summary", "id", default=w.DASH)))
                row.set_subtitle(w.cell_text(
                    fields(bug, "body", "detail", "description", default="")))
                row.set_subtitle_lines(3)
                status = field(bug, "status")
                if status:
                    row.add_suffix(w.pill(str(status), css=w.status_css(status)))
                if bug_id is not None:
                    for label, value in (("Resolve", "resolved"),
                                         ("Reopen", "open")):
                        button = Gtk.Button(label=label)
                        button.add_css_class("flat")
                        button.set_valign(Gtk.Align.CENTER)
                        button.connect(
                            "clicked",
                            lambda _b, r=bug_id, v=value: self.act(
                                lambda: api.post("/api/lumisound/bug-reports/status",
                                                 {"reportId": r, "status": v}),
                                success=f"Report marked {v}.",
                            ),
                        )
                        row.add_suffix(button)
                group.add(row)
            box.append(group)

        if users:
            box.append(w.heading("Accounts", level=3))
            group = w.section("")
            for user in users[:100]:
                user_id = fields(user, "id", "userId", default=None)
                row = Adw.ActionRow(
                    title=str(fields(user, "username", "email", default=w.DASH)),
                    subtitle=w.cell_text(field(user, "email", "")),
                )
                active = fields(user, "isActive", "active", default=None)
                if active is not None:
                    toggle = Gtk.Switch()
                    toggle.set_valign(Gtk.Align.CENTER)
                    toggle.set_active(bool(active))
                    toggle.set_tooltip_text("Account active")
                    if user_id is not None:
                        toggle.connect(
                            "state-set",
                            lambda _s, state, u=user_id: self._set_active(u, state),
                        )
                    row.add_suffix(toggle)
                group.add(row)
            box.append(group)

        if uploads:
            box.append(w.heading("Library", level=3))
            group = w.section("")
            for upload in uploads[:100]:
                upload_id = fields(upload, "id", "uploadId", default=None)
                row = Adw.ActionRow(
                    title=str(fields(upload, "title", "filename", "id",
                                     default=w.DASH)),
                    subtitle=w.cell_text(" · ".join(str(x) for x in (
                        fields(upload, "artist", default=""),
                        fields(upload, "username", "ownerUsername", default=""),
                    ) if x)),
                )
                if upload_id is not None:
                    delete = Gtk.Button(label="Delete")
                    delete.add_css_class("flat")
                    delete.add_css_class("destructive-action")
                    delete.set_valign(Gtk.Align.CENTER)
                    delete.connect(
                        "clicked",
                        lambda _b, u=upload_id: w.confirm(
                            self, "Delete upload?", "This cannot be undone.",
                            "Delete",
                            lambda: self.act(
                                lambda: api.post("/api/lumisound/uploads/delete",
                                                 {"uploadId": u}),
                                success="Upload deleted.",
                            ),
                        ),
                    )
                    row.add_suffix(delete)
                group.add(row)
            box.append(group)

        if not (users or uploads or bugs):
            box.append(w.empty_state("Nothing here yet",
                                     "Lumisound has no accounts or uploads.",
                                     "audio-x-generic-symbolic"))
        return w.clamp_of(box)

    def _set_active(self, user_id: Any, active: bool) -> bool:
        self.act(
            lambda: api.post("/api/lumisound/users/active",
                             {"userId": user_id, "active": bool(active)}),
            success=f"Account {'activated' if active else 'deactivated'}.",
            then=lambda _r: None,
        )
        return False
