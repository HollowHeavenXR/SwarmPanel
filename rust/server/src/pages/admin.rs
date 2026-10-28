//! Admin: Overview, Diagnostics, Intel, Accounts, Databases, Gallery, Lumisound, Audit Log.

use super::fleet::event_card;
use super::ui::{self, arr, icon, n, s};
use super::{Ctx, Page, SECTIONS, allowed, can_see, denied, render};
use crate::api::{accounts_admin as acc, admin as adm, community as com};
use crate::db::J;
use crate::http::{R, Reply, Req};
use maud::{Markup, html};
use serde_json::{Value, json};

async fn gate(req: Req, path: &str, title: &str) -> R<Result<Ctx, Reply>> {
    let ctx = Ctx::new(req).await?;
    if allowed(&ctx.a, path) {
        Ok(Ok(ctx))
    } else {
        let msg = if path == "/admin" && ctx.a.site_owner { "Turn admin mode on in the top bar to open the admin tools." } else { "You don't have access to this page." };
        Ok(Err(denied(&ctx, path, title, msg)))
    }
}

macro_rules! gated {
    ($req:expr, $path:expr, $title:expr) => {
        match gate($req, $path, $title).await? {
            Ok(c) => c,
            Err(r) => return Ok(r),
        }
    };
}

fn qs(ctx: &Ctx) -> String {
    ctx.req.query.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&")
}

// ---- Overview ------------------------------------------------------------------------------------
pub async fn stats_frag(ctx: &Ctx) -> Markup {
    match ctx.api(com::admin_overview, &[], &[]).await {
        Ok(v) => html! {
            div.metrics {
                @if v.get("bots_total").is_some() { (ui::metric_warn("Music bots online", format!("{} / {}", n(&v, "bots_online"), n(&v, "bots_total")), n(&v, "bots_online") < n(&v, "bots_total"))) }
                (ui::metric("Alert rules on", n(&v, "alert_rules_enabled"), None))
                (ui::metric("Audit entries (24h)", n(&v, "audit_entries_recent"), None))
                @if v.get("open_reports").is_some() { (ui::metric_warn("Open gallery reports", n(&v, "open_reports"), n(&v, "open_reports") > 0)) }
            }
        },
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn overview(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/admin", "Admin");
    let admin = SECTIONS.iter().find(|s| s.key == "admin").map(|s| s.items).unwrap_or(&[]);
    let body = html! {
        div #admin-stats data-src="/frag/admin-stats" data-poll="30" { (stats_frag(&ctx).await) }
        div.hub {
            @for i in admin.iter().filter(|i| i.to != "/admin" && can_see(&ctx.a, i.when)) {
                a.hub-card href=(i.to) { span.hub-icon { (icon(i.icon)) } span { strong { (i.label) } small { (i.blurb) } } }
            }
        }
    };
    Ok(render(&ctx, Page::new("/admin", "Overview", "Every admin and moderation tool you can use, in one place.", body)))
}

// ---- Diagnostics -----------------------------------------------------------------------------------
fn metrics_view(m: &Value) -> Markup {
    let t = m.get("totals").cloned().unwrap_or(json!({}));
    html! {
        div.metrics {
            @for (k, l) in [("guilds", "Guilds"), ("voice_connected", "Voice connected"), ("playing", "Playing"), ("paused", "Paused"), ("queued_tracks", "Queued"), ("backup_tracks", "Backup"), ("recovering", "Recovering"), ("lavalink_ready", "Audio ready"), ("stale_metrics", "Stale metrics")] {
                (ui::metric_warn(l, n(&t, k), (k == "recovering" || k == "stale_metrics") && n(&t, k) > 0))
            }
        }
        div.table-wrap { table.table {
            thead { tr { th { "Bot" } th.num { "Guilds" } th.num { "Playing" } th.num { "Queued" } th.num { "Stale" } th { "Status" } } }
            tbody { @for b in arr(m, "bots") {
                @let ms = arr(b, "metrics");
                tr {
                    td { strong { (s(b, "display_name")) } }
                    td.num { (ms.len()) }
                    td.num { (ms.iter().filter(|x| x.b("player_playing")).count()) }
                    td.num { (ms.iter().map(|x| n(x, "queue_count")).sum::<i64>()) }
                    td.num { (ms.iter().filter(|x| x.b("stale")).count()) }
                    td { @if let Some(e) = b.s("error") { (ui::pill("danger", "Error")) " " small.muted { (e) } } @else { (s(b, "status")) } }
                }
            } }
        } }
        details.raw { summary { "Raw metrics" } (ui::json(m)) }
    }
}

pub async fn metrics_frag(ctx: &Ctx) -> Markup {
    match ctx.live("metrics_snapshot", json!({})).await {
        Ok(m) => metrics_view(&m),
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn stability_frag(ctx: &Ctx) -> Markup {
    match ctx.live("stability", json!({})).await {
        Ok(v) => html! {
            div.split {
                div {
                    h3 { "Recovery cooldowns" }
                    @let cd = arr(&v, "cooldowns");
                    @if let Some(e) = v.s("cooldowns_error") { (ui::notice("error", &e)) }
                    @else if cd.is_empty() { (ui::empty("No active cooldowns.")) } @else {
                        div.table-wrap { table.table { thead { tr { th { "Scope" } th { "Bot" } th { "Reason" } th.num { "Remaining" } } }
                            tbody { @for c in cd { tr { td.mono { (s(c, "scope_key")) } td { (s(c, "bot_name")) } td { (s(c, "reason")) } td.num { (ui::uptime(n(c, "remaining_seconds"))) } } } } } }
                    }
                }
                div {
                    h3 { "Recent repairs" }
                    @let rp = arr(&v, "recent_repairs");
                    @if let Some(e) = v.s("repairs_error") { (ui::notice("error", &e)) }
                    @else if rp.is_empty() { (ui::empty("No repairs recorded.")) } @else {
                        div.table-wrap { table.table { thead { tr { th { "When" } th { "Issue" } th { "Action" } th { "Result" } } }
                            tbody { @for r in rp.iter().take(15) { tr {
                                td.nowrap { (ui::time(r.s("created_at"), "ago")) }
                                td { (s(r, "issue_type")) } td { (s(r, "repair_action")) }
                                td { (if r.b("success") { ui::pill("live", "ok") } else { ui::pill("danger", "failed") }) @if let Some(e) = r.s("error_text") { " " small.muted { (e) } } }
                            } } } } }
                    }
                }
            }
        },
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn alert_rules_frag(ctx: &Ctx) -> Markup {
    match ctx.live("alert_rules", json!({})).await {
        Ok(v) => html! {
            @if arr(&v, "rules").is_empty() { (ui::empty("No alert rules.")) }
            ul.rows { @for r in arr(&v, "rules") {
                @let id = s(r, "id");
                li.row {
                    div.row-main {
                        strong { (s(r, "rule_type").replace('_', " ")) }
                        small.muted { "after " (n(r, "threshold_minutes")) "m" @if r.i("escalation_minutes").is_some() { ", escalate after " (n(r, "escalation_minutes")) "m" } @if r.b("escalate_email") { " (email)" } }
                    }
                    div.row-actions {
                        button.btn.sm.on[r.b("enabled")] type="button" data-post=(format!("/api/alert-rules/{id}/update")) data-body=(json!({ "enabled": !r.b("enabled") })) data-done="refresh:#alert-rules" { (if r.b("enabled") { "Enabled" } else { "Disabled" }) }
                        button.btn.sm.danger type="button" data-post=(format!("/api/alert-rules/{id}/delete")) data-confirm="Delete this alert rule?" data-done="refresh:#alert-rules" { (icon("x")) }
                    }
                }
            } }
        },
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn exports_frag(ctx: &Ctx) -> Markup {
    match ctx.live("exports", json!({})).await {
        Ok(v) => html! {
            @if !v.b("enabled") { p.muted.small { "Scheduled exports are turned off on this server." } }
            @let snaps = arr(&v, "snapshots");
            @if snaps.is_empty() { (ui::empty("No exports yet.")) }
            @for snap in snaps.iter().take(14) {
                div.export-day {
                    strong { (s(snap, "date")) }
                    div.chips { @for f in arr(snap, "files") {
                        a.chip href=(format!("/api/exports/{}/{}", s(snap, "date"), s(f, "name"))) { (icon("download")) (s(f, "name")) " · " (ui::bytes(f.f("size_bytes").unwrap_or(0.0))) }
                    } }
                }
            }
        },
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn diagnostics(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/diagnostics", "Diagnostics");
    let body = html! {
        section.card { (ui::section("Stability", Some("Aria's recovery cooldowns and repair journal."), None))
            div #stability data-src="/frag/stability" data-live="stability" { (stability_frag(&ctx).await) } }
        section.card { (ui::section("Live metrics", None, None))
            div #metrics data-src="/frag/metrics" data-live="metrics_snapshot" { (metrics_frag(&ctx).await) } }
        div.split {
            section.card.stack {
                (ui::section("Alert rules", None, None))
                form.stack data-api="/api/alert-rules" data-ok="Alert rule created." data-done="refresh:#alert-rules" data-reset data-omit-empty {
                    div.two-col {
                        label.field { span { "Rule" } select name="rule_type" required {
                            option value="bot_offline" { "Bot offline" } option value="queue_stuck" { "Queue stuck" } option value="stale_metrics" { "Stale metrics" } option value="recovery_pending" { "Recovery pending" }
                        } }
                        label.field { span { "Threshold (minutes)" } input type="number" name="threshold_minutes" data-t="int" min="1" max="1440" value="5" required; }
                        label.field { span { "Escalate after (minutes)" } input type="number" name="escalation_minutes" data-t="int" min="1" max="10080" placeholder="optional"; }
                    }
                    div.inline { label.check { input type="checkbox" name="enabled" checked; span { "Enabled" } } label.check { input type="checkbox" name="escalate_email"; span { "Escalate by email" } } }
                    div.form-foot { button.btn.primary type="submit" { "Add rule" } }
                }
                div #alert-rules data-src="/frag/alert-rules" data-live="alert_rules" { (alert_rules_frag(&ctx).await) }
            }
            section.card { (ui::section("Exports", Some("Daily CSV snapshots."), None))
                div #exports data-src="/frag/exports" data-live="exports" { (exports_frag(&ctx).await) } }
        }
    };
    let actions = html! { button.btn type="button" data-refresh="all" { (icon("refresh")) "Refresh now" } };
    Ok(render(&ctx, Page::new("/diagnostics", "Diagnostics", "Stability, metrics, alert rules, and exports.", body).actions(actions)))
}

// ---- Intel ----------------------------------------------------------------------------------------
pub async fn trends_frag(ctx: &Ctx) -> Markup {
    match ctx.live("trends", json!({})).await {
        Ok(v) => {
            let series = [("queued_tracks", "Queued tracks (24h)", "var(--accent)"), ("active_bots", "Active bots (24h)", "var(--ok)")];
            let flagged: usize = series.iter().map(|(k, _, _)| arr(&v[*k], "anomalies").len()).sum();
            html! {
                @if flagged > 0 { (ui::notice("error", &format!("{flagged} {} flagged in the last 24h. Red points deviate sharply from the window average.", if flagged == 1 { "anomaly" } else { "anomalies" }))) }
                div.chart-grid { @for (k, l, c) in series { (ui::trend_chart(l, arr(&v[k], "points"), arr(&v[k], "anomalies"), c)) } }
            }
        }
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn intel_events_frag(ctx: &Ctx) -> Markup {
    match ctx.live("events", json!({})).await {
        Ok(v) => {
            let evs: Vec<&Value> = arr(&v, "events").iter().rev().collect();
            html! { @if evs.is_empty() { (ui::empty("No events.")) } div.events { @for e in evs { (event_card(e)) } } }
        }
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn intel(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/intel", "Intel");
    let body = html! {
        section.card { (ui::section("24h trends", Some("Hover a point for its reading."), None)) div #trends data-src="/frag/trends" data-live="trends" { (trends_frag(&ctx).await) } }
        div.split.wide-left {
            section.card { (ui::section("Events", Some("Bot errors and Aria Medic events, newest first."), None)) div #intel-events data-src="/frag/intel-events" data-live="events" { (intel_events_frag(&ctx).await) } }
            div.stack {
                section.card { (ui::section("Metrics", None, None)) div #intel-metrics data-src="/frag/metrics" data-live="metrics_snapshot" { (metrics_frag(&ctx).await) } }
                section.card { (ui::section("Stability", None, None)) div #intel-stability data-src="/frag/stability" data-live="stability" { (stability_frag(&ctx).await) } }
            }
        }
    };
    Ok(render(&ctx, Page::new("/intel", "Intel", "Errors, trends, anomalies, and raw events.", body)))
}

// ---- Accounts --------------------------------------------------------------------------------------
pub async fn accounts_frag(ctx: &Ctx) -> Markup {
    let q = ctx.q("query").unwrap_or_default();
    let v = match ctx.api(acc::admin_list, &[("query", q), ("limit", "100".into())], &[]).await {
        Ok(v) => v,
        Err(e) => return ui::notice("error", &format!("Failed to load accounts: {e}")),
    };
    let d = v.get("data").cloned().unwrap_or(json!({}));
    let users = arr(&d, "users");
    let verified = |u: &Value| u.b("email_verified") || u.b("webhook_verified") || u.b("discord_dm_verified");
    let vn = users.iter().filter(|u| verified(u)).count();
    let mods = users.iter().filter(|u| s(u, "panel_role") == "moderator").count();
    html! {
        p.muted { (users.len()) " shown · " (vn) " verified · " (users.len() - vn) " unverified · " (mods) " moderators" }
        div.bulk data-bulk-for="#accounts-tbl" hidden {
            span { "With selected:" }
            button.btn.sm type="button" data-post="/api/swarm-accounts/bulk-verify" data-body=(json!({ "verified": true })) data-ids="#accounts-tbl" data-done="refresh:#accounts" { "Verify" }
            button.btn.sm.danger type="button" data-post="/api/swarm-accounts/bulk-delete" data-body="{}" data-ids="#accounts-tbl" data-confirm="Delete the selected accounts? This cannot be undone." data-done="refresh:#accounts" { "Delete" }
        }
        @if users.is_empty() { (ui::empty("No accounts.")) } @else {
            div.table-wrap { table.table #accounts-tbl {
                thead { tr { th.sel { input type="checkbox" data-select-all aria-label="Select all"; } th { "Account" } th { "Guild" } th { "Verified" } th { "Role" } th { "Last seen" } th { "Actions" } } }
                tbody { @for u in users {
                    @let id = n(u, "id");
                    @let is_mod = s(u, "panel_role") == "moderator";
                    tr {
                        td.sel { input type="checkbox" data-select-row value=(id) aria-label="Select"; }
                        td { strong { (s(u, "username")) } @if let Some(e) = u.s("email").filter(|e| !e.is_empty()) { br; small.muted { (e) } } }
                        td.mono { (s(u, "guild_id")) }
                        td { (if verified(u) { ui::pill("live", "verified") } else { ui::pill("off", "unverified") }) }
                        td { (if is_mod { ui::pill("soft", "moderator") } else { ui::pill("off", "user") }) }
                        td.nowrap { (ui::time(u.s("last_seen_at"), "ago")) }
                        td.actions {
                            details.menu {
                                summary.btn.sm { "Manage" }
                                div.menu-body.stack {
                                    form.stack data-api="/api/swarm-accounts/update" data-ok="Account updated." data-done="refresh:#accounts" {
                                        input type="hidden" name="account_id" value=(id) data-t="int";
                                        div.two-col {
                                            label.field { span { "Username" } input name="username" value=(s(u, "username")); }
                                            label.field { span { "Display name" } input name="display_name" value=(s(u, "display_name")); }
                                            label.field { span { "Email" } input type="email" name="email" value=(s(u, "email")); }
                                            label.field { span { "Guild ID" } input name="guild_id" value=(s(u, "guild_id")); }
                                            label.field { span { "Server name" } input name="server_name" value=(s(u, "server_name")); }
                                        }
                                        label.check { input type="checkbox" name="public_profile" checked[u.b("public_profile")]; span { "Public profile" } }
                                        button.btn.sm.primary type="submit" { "Save" }
                                    }
                                    div.inline {
                                        button.btn.sm type="button" data-post="/api/swarm-accounts/email-verified" data-body=(json!({ "account_id": id, "verified": !verified(u) })) data-done="refresh:#accounts" { (if verified(u) { "Mark unverified" } else { "Mark verified" }) }
                                        button.btn.sm type="button" data-post="/api/swarm-accounts/resend-verification" data-body=(json!({ "account_id": id })) data-ok="Verification code sent (if a webhook is set)." { "Resend code" }
                                        button.btn.sm type="button" data-post="/api/swarm-accounts/moderator" data-body=(json!({ "account_id": id, "moderator": !is_mod })) data-done="refresh:#accounts" { (if is_mod { "Remove moderator" } else { "Make moderator" }) }
                                        button.btn.sm type="button" data-post="/api/swarm-accounts/reset-password" data-body=(json!({ "account_id": id })) data-prompt="New password (8+ characters)" data-prompt-key="new_password" data-prompt-secret data-ok="Password reset." { "Reset password" }
                                        button.btn.sm.danger type="button" data-post="/api/swarm-accounts/delete" data-body=(json!({ "account_id": id })) data-confirm=(format!("Delete the account {}? This cannot be undone.", s(u, "username"))) data-done="refresh:#accounts" { "Delete" }
                                    }
                                }
                            }
                        }
                    }
                } }
            } }
        }
    }
}

pub async fn accounts(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/accounts", "Accounts");
    let body = html! {
        form.filter-bar method="get" action="/accounts" data-get="#accounts" {
            label.search { (icon("search")) input type="search" name="query" value=(ctx.q("query").unwrap_or_default()) placeholder="Search username, guild, email…" autocomplete="off" aria-label="Search accounts"; }
        }
        div #accounts data-src=(format!("/frag/accounts?{}", qs(&ctx))) { (accounts_frag(&ctx).await) }
    };
    Ok(render(&ctx, Page::new("/accounts", "Accounts", "Recover and manage swarm accounts.", body)))
}

// ---- Databases -------------------------------------------------------------------------------------
pub async fn databases(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/databases", "Databases");
    let dbs = ctx.api(adm::databases, &[("include_tables", "true".into())], &[]).await;
    let schemas = dbs.as_ref().map(|v| arr(v, "schemas").to_vec()).unwrap_or_default();
    let schema = ctx.q("schema").filter(|q| schemas.iter().any(|x| s(x, "schema") == *q)).or_else(|| schemas.first().map(|x| s(x, "schema"))).unwrap_or_default();
    let tables: Vec<Value> = schemas.iter().find(|x| s(x, "schema") == schema).map(|x| arr(x, "tables").to_vec()).unwrap_or_default();
    let table = ctx.q("table").filter(|t| tables.iter().any(|x| s(x, "table_name") == *t)).or_else(|| tables.first().map(|x| s(x, "table_name"))).unwrap_or_default();
    let data = if table.is_empty() { Ok(json!({ "rows": [] })) } else { ctx.api(adm::database_data, &[("schema_name", schema.clone()), ("table_name", table.clone()), ("limit", "100".into())], &[]).await };
    let csv = format!("/api/database/data?schema_name={}&table_name={}&format=csv", urlencoding::encode(&schema), urlencoding::encode(&table));
    let truncate = |kind: &str| {
        let expected = if kind == "table" { format!("TRUNCATE {schema}.{table}") } else { format!("TRUNCATE ALL {schema}") };
        let path = if kind == "table" { "/api/database/truncate-table" } else { "/api/database/truncate-schema" };
        html! {
            details.danger-zone {
                summary.btn.sm.danger { (if kind == "table" { "Truncate table" } else { "Truncate schema" }) }
                form.stack data-api=(path) data-done="reload" data-ok="Truncated." {
                    (ui::notice("error", &if kind == "table" { format!("This permanently deletes every row in {schema}.{table}.") } else { format!("This permanently deletes every table's rows in the {schema} schema.") }))
                    input type="hidden" name="schema_name" value=(schema);
                    @if kind == "table" { input type="hidden" name="table_name" value=(table); }
                    label.field { span { "Type exactly: " code { (expected) } } input type="text" name="confirm_text" required autocomplete="off"; }
                    label.field { span { "Owner confirmation phrase (if this server has one)" } input type="text" name="owner_confirm_text" autocomplete="off"; }
                    button.btn.danger type="submit" { "Confirm truncate" }
                }
            }
        }
    };
    let body = html! {
        @if let Err(e) = &dbs { (ui::notice("error", e)) }
        form.filter-bar method="get" action="/databases" data-autosubmit {
            label.field { span { "Schema" } select name="schema" { @for x in &schemas { option value=(s(x, "schema")) selected[s(x, "schema") == schema] { (s(x, "schema")) } } } }
            label.field { span { "Table" } select name="table" { @for t in &tables { option value=(s(t, "table_name")) selected[s(t, "table_name") == table] { (s(t, "table_name")) " (" (n(t, "estimated_rows")) ")" } } } }
            div.inline { a.btn href=(csv) download { (icon("download")) "Export CSV" } (truncate("table")) (truncate("schema")) }
        }
        @match &data {
            Ok(v) => { p.muted.small { "Showing up to 100 rows, first 9 columns." } (ui::raw_table(arr(v, "rows"), 9, None, |_| None)) },
            Err(e) => { (ui::notice("error", e)) },
        }
    };
    Ok(render(&ctx, Page::new("/databases", "Databases", "Browse raw schema tables.", body)))
}

// ---- Gallery ---------------------------------------------------------------------------------------
fn gallery_delete(table: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match table {
        "users" => Some(("user_id", "/api/image-gallery/users/delete", "/api/image-gallery/admin/users/bulk-delete")),
        "media_items" => Some(("media_id", "/api/image-gallery/media/delete", "/api/image-gallery/admin/media/bulk-delete")),
        "media_comments" => Some(("comment_id", "/api/image-gallery/comments/delete", "/api/image-gallery/admin/comments/bulk-delete")),
        _ => None,
    }
}

fn gallery_user_manage(u: &Value) -> Markup {
    let id = s(u, "id");
    let email_ok = u.get("email_verified_at").is_some_and(|v| !v.is_null());
    let age_ok = u.get("age_verified_at").is_some_and(|v| !v.is_null());
    html! {
        details.menu {
            summary.btn.sm { "Manage" }
            div.menu-body.stack {
                form.stack data-api="/api/image-gallery/users/update" data-ok="User updated." data-done="refresh:#gallery-table" {
                    input type="hidden" name="user_id" value=(id);
                    label.field { span { "Username" } input name="username" value=(s(u, "username")); }
                    label.field { span { "Display name" } input name="display_name" value=(s(u, "display_name")); }
                    label.field { span { "Email" } input type="email" name="email" value=(s(u, "email")); }
                    label.check { input type="checkbox" name="public_profile" checked[u.b("public_profile")]; span { "Public profile" } }
                    button.btn.sm.primary type="submit" { "Save details" }
                }
                div.inline {
                    button.btn.sm type="button" data-post="/api/image-gallery/users/email-verified" data-body=(json!({ "user_id": id, "verified": !email_ok })) data-done="refresh:#gallery-table" { (if email_ok { "Mark email unverified" } else { "Mark email verified" }) }
                    button.btn.sm type="button" data-post="/api/image-gallery/users/age-verified" data-body=(json!({ "user_id": id, "verified": !age_ok })) data-done="refresh:#gallery-table" { (if age_ok { "Mark age unverified" } else { "Mark age verified" }) }
                    button.btn.sm type="button" disabled[email_ok || s(u, "email").is_empty()] data-post="/api/image-gallery/users/resend-verification" data-body=(json!({ "user_id": id })) data-ok="Verification email sent." { "Resend email" }
                    button.btn.sm.danger type="button" data-post="/api/image-gallery/users/reset-password" data-body=(json!({ "user_id": id })) data-prompt="New password (8+ characters)" data-prompt-key="new_password" data-prompt-secret data-ok="Password reset." { "Reset password" }
                }
            }
        }
    }
}

pub async fn gallery_table_frag(ctx: &Ctx) -> Markup {
    let table = ctx.q("table").unwrap_or_else(|| "users".into());
    let v = match ctx.api(adm::gallery_table_data, &[("table_name", table.clone()), ("limit", "100".into())], &[]).await {
        Ok(v) => v,
        Err(e) => return ui::notice("error", &e),
    };
    let rows = arr(&v, "rows");
    let del = gallery_delete(&table);
    let sel_id = del.map(|_| "gallery-tbl");
    html! {
        @if let Some((_, _, bulk)) = del {
            div.bulk data-bulk-for="#gallery-tbl" hidden {
                span { "With selected:" }
                button.btn.sm.danger type="button" data-post=(bulk) data-body="{}" data-ids="#gallery-tbl" data-confirm="Delete the selected rows? This cannot be undone." data-done="refresh:#gallery-table" { "Delete" }
            }
        }
        (ui::raw_table(rows, 9, sel_id, |r| {
            let id = s(r, "id");
            if table == "media_reports" {
                return Some(html! {
                    form.inline data-api="/api/image-gallery/reports/status" data-ok="Report updated." data-done="refresh:#gallery-table" {
                        input type="hidden" name="report_id" value=(id);
                        select name="status" aria-label="Status" { @for st in ["open", "reviewed", "dismissed"] { option value=(st) selected[s(r, "status") == st] { (st) } } }
                        button.btn.sm type="submit" { "Save" }
                    }
                });
            }
            del.map(|(key, single, _)| html! {
                @if table == "users" { (gallery_user_manage(r)) }
                button.btn.sm.danger type="button" data-post=(single) data-body=(json!({ key: id })) data-confirm="Delete this row? This cannot be undone." data-ok="Deleted." data-done="refresh:#gallery-table" { (icon("x")) }
            })
        }))
    }
}

pub async fn gallery(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/gallery-admin", "Gallery");
    let (summary, tables) = tokio::join!(ctx.api(adm::gallery_admin, &[], &[]), ctx.api(adm::gallery_tables, &[], &[]));
    let tables = tables.map(|v| arr(&v, "tables").to_vec()).unwrap_or_default();
    let table = ctx.q("table").unwrap_or_else(|| "users".into());
    let body = html! {
        @match &summary {
            Ok(v) => {
                @let su = v.get("data").and_then(|d| d.get("summary")).or(v.get("summary")).cloned().unwrap_or(json!({}));
                div.metrics {
                    (ui::metric("Users", n(&su, "users"), None)) (ui::metric("Media", n(&su, "media"), None)) (ui::metric("Comments", n(&su, "comments"), None))
                    (ui::metric("Collections", n(&su, "collections"), None)) (ui::metric_warn("Open reports", n(&su, "reports_open"), n(&su, "reports_open") > 0))
                }
            },
            Err(e) => { (ui::notice("error", e)) },
        }
        form.filter-bar method="get" action="/gallery-admin" data-autosubmit {
            label.field { span { "Table" } select name="table" { @for t in &tables { option value=(s(t, "table_name")) selected[s(t, "table_name") == table] { (s(t, "table_name")) " (" (n(t, "estimated_rows")) ")" } } } }
            div.inline {
                a.btn href="/api/image-gallery/admin/media/export.csv" download { (icon("download")) "Media CSV" }
                a.btn href="/api/image-gallery/admin/users/export.csv" download { (icon("download")) "Users CSV" }
            }
        }
        div #gallery-table data-src=(format!("/frag/gallery-table?table={}", urlencoding::encode(&table))) { (gallery_table_frag(&ctx).await) }
    };
    Ok(render(&ctx, Page::new("/gallery-admin", "Gallery", "Image Gallery users, media, comments, and reports.", body)))
}

// ---- Lumisound -------------------------------------------------------------------------------------
fn simple_table(rows: &[Value], cols: &[(&str, &str)], fmt: impl Fn(&str, &Value) -> Markup, actions: impl Fn(&Value) -> Option<Markup>) -> Markup {
    if rows.is_empty() {
        return ui::empty("No rows.");
    }
    let has_actions = rows.iter().any(|r| actions(r).is_some());
    html! {
        div.table-wrap { table.table {
            thead { tr { @for (_, l) in cols { th { (l) } } @if has_actions { th {} } } }
            tbody { @for r in rows { tr { @for (k, _) in cols { td { (fmt(k, r)) } } @if has_actions { td.actions { @if let Some(a) = actions(r) { (a) } } } } } }
        } }
    }
}

fn ls_fmt(k: &str, r: &Value) -> Markup {
    match k {
        "file_size_bytes" => html! { (ui::bytes(r.f(k).unwrap_or(0.0))) },
        "created_at" | "uploaded_at" | "played_at" | "updated_at" => ui::time(r.s(k), "ago"),
        "is_playing" => if r.b(k) { ui::pill("live", "playing") } else { ui::pill("off", "paused") },
        "position" => html! { (ui::dur(r.f("position_seconds").unwrap_or(0.0))) " / " (ui::dur(r.f("duration_seconds").unwrap_or(0.0))) },
        _ => html! { (r.s(k).unwrap_or_else(|| "—".into())) },
    }
}

pub async fn lumisound_frag(ctx: &Ctx) -> Markup {
    let d = match ctx.live("lumisound_admin", json!({})).await {
        Ok(v) => v.get("data").cloned().unwrap_or(json!({})),
        Err(e) => return ui::notice("error", &format!("Failed to load Lumisound data: {e}")),
    };
    let su = d.get("summary").cloned().unwrap_or(json!({}));
    let post = |path: &str, body: Value, label: &str, danger: bool, confirm: Option<String>| {
        html! { button.btn.sm.danger[danger] type="button" data-post=(path) data-body=(body) data-confirm=[confirm] data-done="refresh:#lumisound" { (label) } }
    };
    html! {
        div.metrics {
            (ui::metric("Users", n(&su, "users"), None)) (ui::metric("Active users", n(&su, "active_users"), None)) (ui::metric("Listening now", n(&su, "listening_now"), None))
            (ui::metric("Plays (24h)", n(&su, "plays_24h"), None)) (ui::metric("Uploads", n(&su, "uploads"), None)) (ui::metric("Upload storage", ui::bytes(su.f("uploads_storage_bytes").unwrap_or(0.0)), None))
            (ui::metric("Playlists", n(&su, "playlists"), None)) (ui::metric("Plays logged", n(&su, "play_history"), None))
            (ui::metric_warn("Open bug reports", n(&su, "bug_reports_open"), n(&su, "bug_reports_open") > 0)) (ui::metric("Listen rooms", n(&su, "listen_rooms_active"), None))
        }
        div.split {
            section.card { (ui::section("Listening now", None, None)) (simple_table(arr(&d, "now_playing"), &[("username", "User"), ("title", "Title"), ("artist", "Artist"), ("source", "Source"), ("position", "Position")], ls_fmt, |_| None)) }
            section.card { (ui::section("Recent plays", None, None)) (simple_table(arr(&d, "recent_plays"), &[("username", "User"), ("title", "Title"), ("artist", "Artist"), ("played_at", "Played"), ("listen_seconds", "Listened (s)")], ls_fmt, |_| None)) }
        }
        section.card { (ui::section("Users", None, None)) (simple_table(arr(&d, "users"), &[("username", "Username"), ("display_name", "Display name"), ("email", "Email"), ("created_at", "Created"), ("playlist_count", "Playlists"), ("upload_count", "Uploads")], ls_fmt, |r| {
            let id = s(r, "id");
            let active = !matches!(r.get("is_active"), Some(Value::Bool(false))) && r.i("is_active") != Some(0);
            Some(if active { post("/api/lumisound/users/active", json!({ "user_id": id, "active": false }), "Suspend", true, Some(format!("Suspend Lumisound user \"{}\"?", s(r, "username")))) }
                 else { post("/api/lumisound/users/active", json!({ "user_id": id, "active": true }), "Reinstate", false, None) })
        })) }
        section.card { (ui::section("Recent uploads", None, None)) (simple_table(arr(&d, "uploads"), &[("title", "Title"), ("filename", "File"), ("artist", "Artist"), ("username", "User"), ("file_size_bytes", "Size"), ("uploaded_at", "Uploaded")], ls_fmt, |r| {
            Some(post("/api/lumisound/uploads/delete", json!({ "upload_id": s(r, "id") }), "Delete", true, Some(format!("Delete upload \"{}\"?", r.s("title").or(r.s("filename")).unwrap_or_default()))))
        })) }
        div.split {
            section.card { (ui::section("Bug reports", None, None)) (simple_table(arr(&d, "bug_reports"), &[("username", "User"), ("category", "Category"), ("description", "Description"), ("status", "Status"), ("created_at", "Created")], ls_fmt, |r| {
                let resolved = s(r, "status") == "resolved";
                Some(post("/api/lumisound/bug-reports/status", json!({ "report_id": s(r, "id"), "status": if resolved { "open" } else { "resolved" } }), if resolved { "Reopen" } else { "Resolve" }, false, None))
            })) }
            section.card { (ui::section("Active listen rooms", None, None)) (simple_table(arr(&d, "listen_rooms"), &[("room_code", "Code"), ("title", "Title"), ("artist", "Artist"), ("host_username", "Host"), ("is_playing", "State"), ("updated_at", "Updated")], ls_fmt, |_| None)) }
        }
    }
}

pub async fn lumisound(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/lumisound-admin", "Lumisound");
    let body = html! { div #lumisound data-src="/frag/lumisound" data-live="lumisound_admin" { (lumisound_frag(&ctx).await) } };
    Ok(render(&ctx, Page::new("/lumisound-admin", "Lumisound", "Accounts, library, and activity for the Lumisound iOS app.", body)))
}

// ---- Audit log -------------------------------------------------------------------------------------
fn short(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "∅".into(),
        Some(Value::String(s)) => s.clone(),
        Some(o) => o.to_string(),
    }
}

pub async fn audit_frag(ctx: &Ctx) -> Markup {
    let v = match ctx.live("audit_log", json!({})).await {
        Ok(v) => v,
        Err(e) => return ui::notice("error", &format!("Failed to load audit log: {e}")),
    };
    let d = v.get("data").cloned().unwrap_or(json!({}));
    let can_revert = ctx.a.is_admin();
    let entries = arr(&d, "entries");
    html! {
        p.muted.small { (n(&d, "total")) " entries in total; showing the latest " (entries.len()) "." }
        @if entries.is_empty() { (ui::empty("No audit entries.")) }
        ol.audit { @for e in entries {
            @let details = s(e, "details");
            @let parsed = serde_json::from_str::<Value>(&details).ok();
            li {
                div.audit-head {
                    strong { (s(e, "action").replace('_', " ")) }
                    span.muted { "by " (s(e, "actor_username")) @if let Some(t) = e.s("target_type") { " · " (t) " " (s(e, "target_id")) } }
                    (ui::time(e.s("created_at"), "datetime"))
                    @if can_revert && (s(e, "action") == "swarm_account_update" || s(e, "action") == "alert_rule_update") {
                        button.btn.sm type="button" data-post=(format!("/api/audit-log/{}/revert", s(e, "id"))) data-body="{}" data-confirm="Revert this change?" data-ok="Reverted." data-done="refresh:#audit" { "Revert" }
                    }
                }
                @match &parsed {
                    Some(p) if p.get("before").is_some() || p.get("after").is_some() => {
                        @let (b, a) = (p.get("before").cloned().unwrap_or(json!({})), p.get("after").cloned().unwrap_or(json!({})));
                        dl.diff { @for k in diff_keys(&b, &a) { dt { code { (k) } } dd { span.before { (short(b.get(&k))) } " → " span.after { (short(a.get(&k))) } } } }
                    },
                    Some(p) if !p.is_null() => { (ui::json(p)) },
                    _ => { @if !details.is_empty() { p.small { (details) } } },
                }
            }
        } }
    }
}

fn diff_keys(b: &Value, a: &Value) -> Vec<String> {
    let mut v: Vec<String> = [b, a].iter().filter_map(|x| x.as_object()).flat_map(|m| m.keys().cloned()).collect();
    v.sort();
    v.dedup();
    v
}

pub async fn audit_log(req: Req) -> R<Reply> {
    let ctx = gated!(req, "/audit-log", "Audit Log");
    let body = html! { div #audit data-src="/frag/audit" data-live="audit_log" { (audit_frag(&ctx).await) } };
    Ok(render(&ctx, Page::new("/audit-log", "Audit Log", "Every recorded admin action.", body)))
}
