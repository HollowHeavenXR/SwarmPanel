//! Fleet: Dashboard, Controls, Invites.

use super::ui::{self, arr, icon, n, s};
use super::{Ctx, Page, call, render};
use crate::api::fleet as flt;
use crate::db::J;
use crate::http::{R, Reply, Req};
use maud::{Markup, html};
use serde_json::{Value, json};

const ACTIONS: &[&str] = &["PLAY", "SMART_RECOMMEND", "PAUSE", "RESUME", "SKIP", "STOP", "CLEAR", "RESET_QUEUE", "SHUFFLE", "LOOP", "FILTER", "LEAVE", "SET_HOME", "RECOVER", "RESTART"];
const FILTERS: &[(&str, &str)] = &[
    ("none", "None"), ("nightcore", "Nightcore"), ("bassboost", "Bassboost"), ("vaporwave", "Vaporwave"), ("8d", "8D"), ("karaoke", "Karaoke"), ("tremolo", "Tremolo"),
    ("vibrato", "Vibrato"), ("lowpass", "Low Pass"), ("lofi", "Lo-fi"), ("electronic", "Electronic"), ("party", "Party"), ("radio", "Radio"), ("cinema", "Cinema"),
];

fn accent(ctx: &Ctx, key: &str) -> &'static str {
    ctx.app().settings.bot(key).map(|b| b.accent).unwrap_or("#89b4fa")
}

fn offline(bot: &Value) -> bool {
    s(bot, "status").eq_ignore_ascii_case("offline") || bot.f("heartbeat_age_seconds").is_some_and(|a| a > 120.0)
}

fn best_session(bot: &Value) -> Option<&Value> {
    let ss = arr(bot, "sessions");
    ss.iter().find(|x| x.b("is_playing")).or(ss.first())
}

fn badge(bot: &Value, sess: Option<&Value>) -> (&'static str, &'static str) {
    if offline(bot) {
        return ("danger", "Offline");
    }
    if s(bot, "status").eq_ignore_ascii_case("degraded") {
        return ("danger", "Degraded");
    }
    match sess {
        Some(x) if x.b("is_playing") => ("live", "Live"),
        Some(x) if x.b("is_paused") => ("soft", "Paused"),
        _ if bot.s("heartbeat_status").or(bot.s("status")).unwrap_or_default().to_lowercase().contains("stale") => ("danger", "Stale"),
        _ => ("off", "Idle"),
    }
}

fn pct(x: &Value) -> f64 {
    let d = x.f("duration_seconds").unwrap_or(0.0);
    if d > 0.0 { (100.0 * x.f("position_seconds").unwrap_or(0.0) / d).clamp(0.0, 100.0) } else { 0.0 }
}

/// Ticking position readout; with `seek` it is also a drag-to-seek bar.
pub fn playback(x: &Value, seek: Option<(&str, &str)>) -> Markup {
    let dur = x.f("duration_seconds").unwrap_or(0.0);
    let pos = x.f("position_seconds").unwrap_or(0.0);
    let p = pct(x);
    let at = x.s("position_observed_at").map(|t| ui::iso(&t)).unwrap_or_default();
    html! {
        div.playback data-playback data-pos=(pos) data-at=(at) data-dur=(dur) data-playing=(x.b("is_playing")) {
            div.seek.can-seek[seek.is_some() && dur > 0.0] data-seek=[seek.filter(|_| dur > 0.0).map(|(b, _)| b)] data-guild=[seek.map(|(_, g)| g)] {
                div.seek-track { div.seek-fill data-fill style=(format!("width:{p:.1}%")) {} div.seek-thumb data-thumb style=(format!("left:{p:.1}%")) {} }
            }
            div.seek-times { span data-cur { (ui::dur(pos)) } span data-total { @if dur > 0.0 { (ui::dur(dur)) } @else { "--:--" } } }
        }
    }
}

fn bot_card(ctx: &Ctx, bot: &Value) -> Markup {
    let key = s(bot, "key");
    let off = offline(bot);
    let style = format!("--card-accent:{}", accent(ctx, &key));
    if s(bot, "kind") == "orchestrator" {
        let m = bot.get("medic_summary").cloned().unwrap_or(json!({}));
        return html! {
            article.bot-card.orchestrator.is-offline[off] style=(style) {
                header.bot-head {
                    span.bot-dot {}
                    div { h3 { (s(bot, "display_name")) } small { "Autonomous swarm orchestrator" } }
                    (ui::pill(if off { "danger" } else { "live" }, if off { "Offline" } else { "Online" }))
                }
                div.bot-now {
                    div.thumb.empty { (icon("gear")) }
                    div {
                        strong { (ui::plural(n(&m, "pending_repairs"), "pending repair", "pending repairs")) ", " (ui::plural(n(&m, "pending_infra"), "infra task", "infra tasks")) }
                        small { (ui::plural(n(bot, "recent_interaction_count"), "interaction", "interactions")) " · " (n(&m, "critical_health")) " critical, " (n(&m, "recoverable_health")) " recoverable" }
                    }
                }
                div.chips {
                    @if let Some(u) = bot.i("uptime_seconds") { span.chip data-uptime { "up " (ui::uptime(u)) } }
                    @if let Some(kb) = bot.i("memory_kb") { span.chip { (kb / 1024) " MB" } }
                    @if let Some(age) = bot.f("heartbeat_age_seconds") { span.chip data-uptime { "heartbeat " (age as i64) "s ago" } }
                }
            }
        };
    }
    let sess = best_session(bot);
    let (tone, label) = badge(bot, sess);
    let title = sess.and_then(|x| x.s("title")).filter(|t| !t.is_empty()).or_else(|| bot.s("error")).unwrap_or_else(|| "Waiting for live playback.".into());
    let sub = sess.and_then(|x| x.s("media_source_label").or_else(|| x.s("session_state_label"))).unwrap_or_else(|| "Live state fills in automatically.".into());
    let thumb = sess.and_then(|x| x.s("thumbnail")).filter(|t| ui::safe_url(t).is_some());
    html! {
        article.bot-card.is-offline[off] style=(style) data-bot=(key) {
            header.bot-head {
                span.bot-dot {}
                div { h3 { (s(bot, "display_name")) } small { (bot.s("heartbeat_status").or(bot.s("status")).unwrap_or_else(|| "telemetry ready".into())) @if let Some(m) = bot.s("playback_mode") { " · " (m) } } }
                (ui::pill(tone, label))
            }
            div.bot-now {
                @if let Some(t) = thumb { img.thumb src=(t) alt="" loading="lazy"; } @else { div.thumb.empty { (icon("note")) } }
                div.bot-now-copy { strong { (title) } small.detail { (sub) } }
            }
            @if let Some(x) = sess { @if x.f("duration_seconds").unwrap_or(0.0) > 0.0 || x.b("is_playing") { (playback(x, Some((&key, &s(x, "guild_id"))))) } }
            div.chips {
                span.chip { (n(bot, "active_playing_count")) " live" }
                span.chip { (n(bot, "known_guild_count")) " guilds" }
                span.chip data-queue { (n(bot, "queue_depth")) " queued" }
                span.chip data-queue { (n(bot, "backup_queue_depth")) " backup" }
                @if let Some(age) = bot.f("heartbeat_age_seconds") { span.chip data-uptime { "heartbeat " (age as i64) "s ago" } }
            }
        }
    }
}

pub async fn dashboard_frag(ctx: &Ctx) -> Markup {
    let data = ctx.live("dashboard", json!({})).await.unwrap_or_else(|_| json!({ "bots": [] }));
    let bots = arr(&data, "bots");
    let mut sessions: Vec<(String, &Value)> = Vec::new();
    for b in bots {
        for x in arr(b, "sessions") {
            sessions.push((s(b, "display_name"), x));
        }
    }
    let online = bots.iter().filter(|b| !offline(b)).count();
    let live: Vec<&(String, &Value)> = sessions.iter().filter(|(_, x)| x.b("is_playing")).collect();
    let music: Vec<&Value> = bots.iter().filter(|b| s(b, "kind") == "music").collect();
    let total_q: i64 = bots.iter().map(|b| n(b, "queue_depth")).sum();
    let total_b: i64 = bots.iter().map(|b| n(b, "backup_queue_depth")).sum();
    let guilds: i64 = bots.iter().map(|b| n(b, "known_guild_count")).sum();
    let busiest = music.iter().max_by_key(|b| n(b, "queue_depth"));
    let aria = bots.iter().find(|b| s(b, "key") == "aria").map(|b| if offline(b) { "Offline".to_string() } else { b.s("status").unwrap_or_else(|| "Online".into()) });
    let mut leaders: Vec<&&Value> = music.iter().filter(|b| n(b, "queue_depth") > 0).collect();
    leaders.sort_by_key(|b| -n(b, "queue_depth"));
    let shown: Vec<&(String, &Value)> = sessions.iter().filter(|(_, x)| x.b("is_playing") || x.b("is_paused") || x.s("title").is_some_and(|t| !t.is_empty())).collect();
    html! {
        section.overview {
            div.spotlight {
                div.spotlight-head {
                    span.eyebrow { "Fleet overview" }
                    (ui::pill(if live.is_empty() { "off" } else { "live" }, if live.is_empty() { "Idle" } else { "Active" }))
                }
                @if let Some((name, x)) = live.first() {
                    div.now {
                        @if let Some(t) = x.s("thumbnail").filter(|t| ui::safe_url(t).is_some()) { img.now-art src=(t) alt=""; } @else { div.now-art.empty { (icon("note")) } }
                        div.now-copy {
                            strong { (x.s("title").unwrap_or_else(|| "Now playing".into())) }
                            small { (name) @if let Some(c) = x.s("channel_name") { " · #" (c) } }
                            (playback(x, None))
                        }
                    }
                } @else {
                    @match busiest {
                        Some(b) if n(b, "queue_depth") > 0 => { strong.big { (s(b, "display_name")) " has the deepest queue" } p.muted { (n(b, "queue_depth")) " queued" } },
                        _ => { strong.big { "The fleet is quiet right now" } p.muted { "Nothing is playing. Send an order from Controls." } },
                    }
                }
            }
            div.metrics {
                (ui::metric("Bots online", format!("{online} / {}", bots.len()), Some("heartbeat within 120s")))
                (ui::metric("Live sessions", live.len(), Some("currently playing")))
                (ui::metric("Queue depth", total_q, Some(&format!("{total_b} in backup"))))
                (ui::metric("Guilds served", guilds, Some("across the fleet")))
                @if let Some(st) = &aria { (ui::metric("Aria", st, Some("orchestrator"))) }
            }
        }
        @if !leaders.is_empty() {
            div.leaders {
                span.eyebrow { "Queue leaders" }
                @for b in leaders.iter().take(4) { span.leader style=(format!("--card-accent:{}", accent(ctx, &s(b, "key")))) { span.bot-dot {} (s(b, "display_name")) b { (n(b, "queue_depth")) } } }
            }
        }
        (ui::section("Bots", Some(&format!("{} bots · {} online", bots.len(), online)), None))
        div.bot-grid { @for b in bots { (bot_card(ctx, b)) } }
        (ui::section("Live sessions", None, None))
        @if shown.is_empty() { (ui::empty("Nothing playing right now.")) } @else {
            div.table-wrap { table.table {
                thead { tr { th { "Bot" } th { "Track" } th { "State" } th.w-pos { "Position" } th.num { "Queue" } } }
                tbody { @for (name, x) in &shown { tr {
                    td { strong { (name) } @if let Some(c) = x.s("channel_name") { br; small.muted { "#" (c) } } }
                    td { (x.s("title").unwrap_or_else(|| "—".into())) }
                    td { (ui::pill(if x.b("is_playing") { "live" } else if x.b("is_paused") { "soft" } else { "off" }, &s(x, "session_state_label"))) }
                    td { (playback(x, None)) }
                    td.num { (n(x, "queue_count")) }
                } } }
            } }
        }
    }
}

pub fn event_card(e: &Value) -> Markup {
    let level = s(e, "level").to_lowercase();
    let tone = match level.as_str() {
        "error" | "critical" => "error",
        "warning" | "warn" => "warning",
        _ => "info",
    };
    html! {
        article class=(format!("event event-{tone}")) {
            div.event-head { strong { (e.s("title").or(e.s("type")).unwrap_or_else(|| "Event".into())) } (ui::time(e.s("timestamp"), "ago")) }
            @if let Some(d) = e.s("description").filter(|d| !d.is_empty()) { p { (d) } }
            div.event-meta { small { (s(e, "source")) } small { (if level.is_empty() { "info".into() } else { level.clone() }) } }
        }
    }
}

pub async fn events_frag(ctx: &Ctx) -> Markup {
    match ctx.live("events", json!({})).await {
        Ok(v) => {
            let evs = arr(&v, "events");
            let latest: Vec<&Value> = evs.iter().rev().take(8).collect();
            html! { @if latest.is_empty() { (ui::empty("No incidents. The fleet is quiet.")) } @else { div.events { @for e in latest { (event_card(e)) } } } }
        }
        Err(e) => ui::notice("error", &format!("Incident feed unavailable: {e}")),
    }
}

pub async fn dashboard(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let path = if ctx.req.path == "/dashboard" { "/dashboard" } else { "/" };
    let body = html! {
        div #dash data-src="/frag/dashboard" data-live="dashboard" { (dashboard_frag(&ctx).await) }
        @if ctx.a.is_admin() {
            (ui::section("Incidents", Some("Latest bot errors and Aria Medic events."), Some(html! { a.btn href="/intel" { "Open Intel" } })))
            div #dash-events data-src="/frag/events" data-live="events" { (events_frag(&ctx).await) }
        }
    };
    Ok(render(&ctx, Page::new(path, "Dashboard", "Live status across the swarm.", body)))
}

// ---- Controls ------------------------------------------------------------------------------------
fn channel_options(guild: Option<&Value>, types: &[i64], selected: Option<&str>) -> Markup {
    let chans: Vec<&Value> = guild.map(|g| arr(g, "channels").iter().filter(|c| types.contains(&c.i("type").unwrap_or(-1))).collect()).unwrap_or_default();
    html! { @for c in chans { option value=(s(c, "id")) selected[selected == Some(s(c, "id").as_str())] { (c.s("name").unwrap_or_else(|| s(c, "id"))) } } }
}

pub async fn control_state_frag(ctx: &Ctx) -> Markup {
    let (bk, g) = (ctx.q("bot_key").unwrap_or_default(), ctx.q("guild_id").unwrap_or_default());
    if bk.is_empty() || g.is_empty() {
        return ui::empty("Choose a bot and guild.");
    }
    match ctx.live("control_state", json!({ "bot_key": bk, "guild_id": g })).await {
        Ok(st) => {
            let x = st.get("session").cloned().unwrap_or(st.clone());
            let recovering = s(&x, "session_state") == "recovering";
            html! {
                div.state-card.recovering[recovering] {
                    div.state-head {
                        div { strong { (x.s("title").filter(|t| !t.is_empty()).unwrap_or_else(|| "Nothing playing".into())) } small.muted { (s(&x, "session_state_label")) } }
                        (ui::pill(if x.b("is_playing") { "live" } else if recovering { "danger" } else if x.b("is_paused") { "soft" } else { "off" }, if recovering { "Recovery pending" } else if x.b("is_playing") { "Playing" } else if x.b("is_paused") { "Paused" } else { "Idle" }))
                    }
                    @if x.f("duration_seconds").unwrap_or(0.0) > 0.0 { (playback(&x, Some((&bk, &g)))) }
                    dl.kv {
                        dt { "Channel" } dd { (x.s("channel_name").map(|c| format!("#{c}")).or(x.s("channel_id")).unwrap_or_else(|| "Not connected".into())) }
                        dt { "Home" } dd { (x.s("home_channel_name").map(|c| format!("#{c}")).or(x.s("home_channel_id")).unwrap_or_else(|| "—".into())) }
                        dt { "Queue" } dd { (n(&x, "queue_count")) " queued · " (n(&x, "backup_queue_count")) " backup" }
                        dt { "Loop / filter" } dd { (x.s("loop_mode").unwrap_or_else(|| "off".into())) " / " (x.s("filter_mode").unwrap_or_else(|| "none".into())) }
                    }
                    details.raw { summary { "Raw state" } (ui::json(&st)) }
                }
            }
        }
        Err(e) => ui::notice("error", &format!("Couldn't load control state: {e}")),
    }
}

pub async fn queues_frag(ctx: &Ctx) -> Markup {
    let (bk, g) = (ctx.q("bot_key").unwrap_or_default(), ctx.q("guild_id").unwrap_or_default());
    if g.is_empty() {
        return ui::empty("Choose a guild.");
    }
    match ctx.live("queues", json!({ "bot_key": bk, "guild_id": g })).await {
        Ok(v) => {
            let qs = arr(&v, "queues");
            html! {
                @if qs.is_empty() { (ui::empty("No saved queues for this bot and guild.")) } @else {
                    ul.rows { @for q in qs {
                        @let id = s(q, "id");
                        @let count = arr(q, "items").len() as i64;
                        li.row {
                            div.row-main { strong { (s(q, "name")) } small.muted { (ui::plural(count, "track", "tracks")) } }
                            div.row-actions {
                                button.btn.sm.primary type="button" data-post="/controls/load-queue" data-body=(json!({ "queue_id": id, "bot_key": bk, "guild_id": g })) data-done="refresh" disabled[count == 0] { (icon("play")) "Load" }
                                button.btn.sm type="button" data-post=(format!("/api/queues/{id}/rename")) data-body=(json!({ "guild_id": g })) data-prompt="New queue name" data-prompt-key="name" data-prompt-value=(s(q, "name")) data-ok="Renamed." data-done="refresh" { (icon("edit")) "Rename" }
                                button.btn.sm.danger type="button" data-post=(format!("/api/queues/{id}/delete")) data-confirm=(format!("Delete the saved queue \"{}\"?", s(q, "name"))) data-ok="Deleted." data-done="refresh" { (icon("x")) }
                            }
                        }
                    } }
                }
            }
        }
        Err(e) => ui::notice("error", &format!("Couldn't load saved queues: {e}")),
    }
}

pub async fn guild_overview_frag(ctx: &Ctx) -> Markup {
    let g = ctx.q("guild_id").unwrap_or_default();
    if g.is_empty() {
        return ui::empty("Choose a guild first.");
    }
    match ctx.api(flt::control_matrix, &[], &[("guild_id", g.clone())]).await {
        Ok(v) => {
            let bots = arr(&v, "bots");
            let active = bots.iter().filter(|b| b.get("session").is_some_and(|x| x.b("is_playing") || x.b("is_paused"))).count();
            html! {
                p.muted { (active) " of " (bots.len()) " bots active in this guild · loaded " (ui::time(Some(crate::util::iso_now()), "time")) }
                div.table-wrap { table.table {
                    thead { tr { th { "Bot" } th { "State" } th { "Now playing" } th {} } }
                    tbody { @for b in bots {
                        @let x = b.get("session").cloned().unwrap_or(json!({}));
                        @let key = b.s("key").or(b.s("bot_key")).or(x.s("bot_key")).unwrap_or_default();
                        @let name = b.s("display_name").or(b.s("bot_display")).or(x.s("bot_display")).unwrap_or(key.clone());
                        @if let Some(err) = b.s("error") {
                            tr { td { strong { (name) } } td { (ui::pill("danger", "Unavailable")) } td colspan="2" { (err) } }
                        } @else {
                            @let (playing, paused) = (x.b("is_playing"), x.b("is_paused"));
                            tr {
                                td { strong { (name) } br; small.muted { (x.s("channel_name").map(|c| format!("#{c}")).or(x.s("channel_id").map(|c| format!("Channel {c}"))).unwrap_or_else(|| "Not connected".into())) } }
                                td { (ui::pill(if playing { "live" } else if paused { "soft" } else { "off" }, &x.s("session_state_label").unwrap_or_else(|| if playing { "Playing".into() } else { "Idle".into() }))) }
                                td { (x.s("title").unwrap_or_else(|| "—".into())) br; small.muted { (n(&x, "queue_count")) " queued" } }
                                td.actions { @if playing || paused {
                                    button.btn.sm type="button" data-post="/api/bots/control" data-body=(json!({ "bot_key": key, "guild_id": g, "action": if paused { "RESUME" } else { "PAUSE" }, "payload": {} })) data-ok="Order sent." data-done="refresh:#guild-overview" { (if paused { "Resume" } else { "Pause" }) }
                                    button.btn.sm type="button" data-post="/api/bots/control" data-body=(json!({ "bot_key": key, "guild_id": g, "action": "SKIP", "payload": {} })) data-ok="Order sent." data-done="refresh:#guild-overview" { "Skip" }
                                } }
                            }
                        }
                    } }
                } }
            }
        }
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn controls(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let data = crate::fleet::dashboard_payload(ctx.app(), &ctx.a, false).await;
    let bots: Vec<&Value> = arr(&data, "bots").iter().filter(|b| s(b, "kind") == "music").collect();
    let bot_key = ctx.q("bot").filter(|k| bots.iter().any(|b| s(b, "key") == *k)).or_else(|| bots.first().map(|b| s(b, "key"))).unwrap_or_default();
    let bot = bots.iter().find(|b| s(b, "key") == bot_key).copied();
    let own = ctx.own_guild();
    let session_guild = bot.and_then(|b| best_session(b)).and_then(|x| x.s("guild_id"));
    let guild_id = own.clone().or(ctx.q("guild")).or(session_guild).unwrap_or_default();

    // Guild + channel lists come from the bot's Discord inventory (cached server-side).
    let inv = if bot_key.is_empty() { Ok(json!({})) } else { ctx.api(flt::inventory, &[], &[("bot_key", bot_key.clone())]).await };
    let inv_err = inv.as_ref().err().cloned();
    let inv = inv.unwrap_or(json!({}));
    let guilds = arr(&inv, "guilds");
    let guild_id = if guild_id.is_empty() { guilds.first().map(|g| s(g, "id")).unwrap_or_default() } else { guild_id };
    let guild = guilds.iter().find(|g| s(g, "id") == guild_id);
    let guild_name = guild.and_then(|g| g.s("name")).unwrap_or_else(|| if guild_id.is_empty() { "—".into() } else { format!("Guild {guild_id}") });
    let st = if bot_key.is_empty() || guild_id.is_empty() { json!({}) } else { ctx.live("control_state", json!({ "bot_key": bot_key, "guild_id": guild_id })).await.unwrap_or(json!({})) };
    let sess = st.get("session").cloned().unwrap_or(st.clone());
    let voice_sel = sess.s("home_channel_id").or(sess.s("channel_id"));
    let text_sel = sess.s("feedback_channel_id");
    let status = bot.map(|b| s(b, "status").to_lowercase()).unwrap_or_default();
    let cs_params = json!({ "bot_key": bot_key, "guild_id": guild_id });
    let qs = format!("bot_key={}&guild_id={}", urlencoding::encode(&bot_key), urlencoding::encode(&guild_id));
    let base = json!({ "bot_key": bot_key, "guild_id": guild_id });
    let quick = |action: &str| {
        let mut b = base.clone();
        b["action"] = json!(action);
        b["payload"] = json!({});
        b
    };

    let body = html! {
        form.target-bar method="get" action="/controls" data-autosubmit {
            label.field { span { "Bot" } select name="bot" {
                @for b in &bots { option value=(s(b, "key")) selected[s(b, "key") == bot_key] { (s(b, "display_name")) @if let Some(m) = b.s("playback_mode") { " (" (m) ")" } } }
            } }
            label.field { span { "Guild" }
                @if let Some(g) = &own { select name="guild" { option value=(g) selected { (guild_name) } } }
                @else { select name="guild" {
                    @if guilds.is_empty() && !guild_id.is_empty() { option value=(guild_id) selected { (guild_name) } }
                    @for g in guilds { option value=(s(g, "id")) selected[s(g, "id") == guild_id] { (g.s("name").unwrap_or_else(|| s(g, "id"))) } }
                } }
            }
            noscript { button.btn type="submit" { "Switch" } }
        }
        @if let Some(e) = &inv_err { (ui::notice("warning", &format!("Couldn't load this bot's guilds and channels: {e}"))) }
        @if status == "degraded" { (ui::notice("warning", "This bot's voice link is degraded. Orders will queue but may not apply until it recovers.")) }
        @else if status == "offline" || status == "stale" { (ui::notice("warning", "This bot looks offline. Orders will queue and run once it reconnects.")) }

        div.control-grid {
            div.stack {
                section.card {
                    (ui::section("Quick transport", Some(&format!("{} in {}", bot.map(|b| s(b, "display_name")).unwrap_or_default(), guild_name)), None))
                    div.transport {
                        button.btn type="button" data-post="/api/bots/control" data-body=(quick("PAUSE")) data-ok="Paused." data-done="refresh:#control-state" { (icon("pause")) "Pause" }
                        button.btn type="button" data-post="/api/bots/control" data-body=(quick("RESUME")) data-ok="Resumed." data-done="refresh:#control-state" { (icon("play")) "Resume" }
                        button.btn type="button" data-post="/api/bots/control" data-body=(quick("SKIP")) data-ok="Skipped." data-done="refresh:#control-state" { (icon("skip")) "Skip" }
                        button.btn type="button" data-post="/api/bots/control" data-body=(quick("SHUFFLE")) data-ok="Shuffled." data-done="refresh:#control-state" { "Shuffle" }
                        button.btn.danger type="button" data-post="/api/bots/control" data-body=(quick("STOP")) data-confirm="Stop playback and clear the current track?" data-ok="Stopped." data-done="refresh:#control-state" { (icon("stop")) "Stop" }
                    }
                }
                form.card.order-form data-api="/api/bots/control" data-ok="Order sent." data-done="refresh:#control-state" data-omit-empty {
                    (ui::section("Send an order", Some("Fields appear for the action you pick."), None))
                    input type="hidden" name="bot_key" value=(bot_key);
                    input type="hidden" name="guild_id" value=(guild_id);
                    label.field { span { "Action" } select name="action" { @for act in ACTIONS { option value=(act) { (act.replace('_', " ")) } } } }
                    label.field.when-play { span { "Source URL or search" } input type="text" name="payload.source_url" placeholder="https://… or search terms" maxlength="2000"; }
                    label.field.when-voice { span { "Voice channel" } select name="payload.voice_channel_id" { option value="" { "Choose channel" } (channel_options(guild, &[2, 13], voice_sel.as_deref())) } }
                    label.field.when-text { span { "Text channel" } select name="payload.text_channel_id" { option value="" { "None" } (channel_options(guild, &[0, 5, 10, 11, 12], text_sel.as_deref())) } }
                    label.field.when-loop { span { "Loop mode" } select name="payload.loop_mode" { @for m in ["off", "song", "queue"] { option value=(m) selected[sess.s("loop_mode").as_deref() == Some(m)] { (m) } } } }
                    label.field.when-filter { span { "Filter" } select name="payload.filter_mode" { @for (v, l) in FILTERS { option value=(v) selected[sess.s("filter_mode").as_deref() == Some(*v)] { (l) } } } }
                    div.form-foot { button.btn.primary type="submit" { "Send order" } }
                }
                section.card {
                    (ui::section("Saved queues", None, None))
                    div #queues data-src=(format!("/frag/queues?{qs}")) data-live="queues" data-live-params=(cs_params.to_string()) { (queues_frag_for(&ctx, &bot_key, &guild_id).await) }
                }
            }
            div.stack {
                section.card {
                    (ui::section("Control state", None, Some(html! { button.icon-btn type="button" data-refresh="#control-state" title="Refresh" { (icon("refresh")) } })))
                    div #control-state data-src=(format!("/frag/control-state?{qs}")) data-live="control_state" data-live-params=(cs_params.to_string()) { (control_state_render(&ctx, &bot_key, &guild_id).await) }
                }
                section.card {
                    (ui::section("Guild overview", Some("Every bot's state in this guild, with quick transport."), Some(html! { button.btn.sm type="button" data-refresh="#guild-overview" { (icon("refresh")) "Load" } })))
                    div #guild-overview data-src=(format!("/frag/guild-overview?guild_id={}", urlencoding::encode(&guild_id))) data-lazy { (ui::empty("Load the overview to see every bot in this guild.")) }
                }
                @if ctx.a.is_admin() {
                    section.card {
                        (ui::section("Fleet recovery", Some("Sends RECOVER to every session fleet-wide that is sitting in Recovery Pending."), None))
                        button.btn.primary type="button" data-post="/controls/recover-all" data-body="{}" data-result="#recover-result" data-done="refresh" { "Recover all stale sessions" }
                        div #recover-result {}
                    }
                }
                section.card {
                    (ui::section("Stage / voice conversion", Some("Converts every home-channel bot's channel in this guild to a stage channel, or back. Discord can't convert in place, so each channel is recreated with the same name and settings."), None))
                    div.transport {
                        button.btn type="button" data-post=(format!("/api/guilds/{guild_id}/convert-channels")) data-body=(json!({ "direction": "stage" })) data-confirm="Convert every home-channel bot's channel in this guild to a stage channel? Each one is deleted and recreated." data-result="#convert-result" data-done="refresh:#control-state" disabled[guild_id.is_empty()] { "To stage channels" }
                        button.btn type="button" data-post=(format!("/api/guilds/{guild_id}/convert-channels")) data-body=(json!({ "direction": "voice" })) data-confirm="Convert every home-channel bot's channel in this guild back to a voice channel? Each one is deleted and recreated." data-result="#convert-result" data-done="refresh:#control-state" disabled[guild_id.is_empty()] { "To voice channels" }
                    }
                    div #convert-result {}
                }
            }
        }
    };
    Ok(render(&ctx, Page::new("/controls", "Controls", "Send a direct order to any bot in any guild.", body)))
}

async fn control_state_render(ctx: &Ctx, bk: &str, g: &str) -> Markup {
    let mut c = ctx.req.query.clone();
    c.insert("bot_key".into(), bk.into());
    c.insert("guild_id".into(), g.into());
    let sub = Ctx { req: super::with_query(&ctx.req, c), a: ctx.a.clone(), prefs: Value::Null };
    control_state_frag(&sub).await
}

async fn queues_frag_for(ctx: &Ctx, bk: &str, g: &str) -> Markup {
    let mut c = ctx.req.query.clone();
    c.insert("bot_key".into(), bk.into());
    c.insert("guild_id".into(), g.into());
    let sub = Ctx { req: super::with_query(&ctx.req, c), a: ctx.a.clone(), prefs: Value::Null };
    queues_frag(&sub).await
}

/// POST /controls/recover-all: RECOVER every session in "recovering" (admin).
pub async fn recover_all(req: Req) -> Reply {
    let a = match req.require_admin().await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let data = crate::fleet::dashboard_payload(&req.app, &a, false).await;
    let mut targets = Vec::new();
    for b in arr(&data, "bots") {
        for x in arr(b, "sessions") {
            if s(x, "session_state") == "recovering" && x.s("guild_id").is_some() {
                targets.push((s(b, "key"), s(x, "guild_id")));
            }
        }
    }
    if targets.is_empty() {
        return Reply::ok(json!({ "ok": true, "message": "Nothing to recover: no sessions are pending." }));
    }
    let (mut ok, mut bad) = (0, 0);
    for (bk, g) in &targets {
        match call(&req, flt::control, &[], &[], json!({ "bot_key": bk, "guild_id": g, "action": "RECOVER", "payload": {} })).await {
            Ok(_) => ok += 1,
            Err(_) => bad += 1,
        }
    }
    let msg = format!("Recovered {ok}/{} session(s){}.", targets.len(), if bad > 0 { format!(", {bad} failed") } else { String::new() });
    Reply::json(if bad > 0 && ok == 0 { 502 } else { 200 }, json!({ "ok": bad == 0, "message": msg, "detail": msg })).by(&a)
}

/// POST /controls/load-queue {queue_id, bot_key, guild_id}: PLAY every saved track.
pub async fn load_queue(req: Req) -> Reply {
    let a = match req.require_auth().await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let (qid, bk, g) = (req.js("queue_id").unwrap_or_default(), req.js("bot_key").unwrap_or_default(), req.js("guild_id").unwrap_or_default());
    if let Err(r) = req.require_guild_scope(&a, &g) {
        return r;
    }
    let qs = crate::queues::list(&req.app, &g, Some(&bk)).await.unwrap_or_default();
    let Some(q) = qs.iter().find(|q| s(q, "id") == qid) else { return Reply::err(404, "Saved queue not found") };
    let items = arr(q, "items");
    let mut ok = 0;
    for it in items {
        let url = it.s("video_url").or(it.s("url")).unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        if call(&req, flt::control, &[], &[], json!({ "bot_key": bk, "guild_id": g, "action": "PLAY", "payload": { "source_url": url } })).await.is_ok() {
            ok += 1;
        }
    }
    let msg = format!("Queued {ok}/{} track(s) from \"{}\".", items.len(), s(q, "name"));
    Reply::json(if ok == 0 { 502 } else { 200 }, json!({ "ok": ok > 0, "message": msg, "detail": msg })).by(&a)
}

// ---- Invites -------------------------------------------------------------------------------------
pub async fn invites_frag(ctx: &Ctx) -> Markup {
    match ctx.live("invites", json!({})).await {
        Ok(res) => {
            let bots = if arr(&res, "invite_bots").is_empty() { arr(&res, "bots") } else { arr(&res, "invite_bots") };
            html! {
                @if bots.is_empty() { (ui::empty("No bots found.")) } @else {
                    div.invite-grid { @for b in bots {
                        @let name = b.s("name").or(b.s("display_name")).unwrap_or_default();
                        article.invite style=(format!("--card-accent:{}", b.s("accent").unwrap_or_else(|| "#89b4fa".into()))) {
                            div.invite-head {
                                (ui::avatar(b.s("icon_url"), &b.s("identity_name").unwrap_or(name.clone()), None, "md"))
                                div { strong { (name) } small.muted { (s(b, "capability_summary")) } }
                                @if b.b("connected_to_session_guild") { (ui::pill("live", "In your guild")) }
                            }
                            @if let Some(e) = b.s("identity_error") { (ui::notice("error", &e)) }
                            @match b.s("invite_url").as_deref().and_then(ui::safe_url) {
                                Some(u) => { a.btn.primary href=(u) target="_blank" rel="noreferrer" { (icon("ext")) "Invite" } },
                                None => { span.muted { "No invite available" } },
                            }
                        }
                    } }
                }
            }
        }
        Err(e) => ui::notice("error", &format!("Failed to load bots: {e}")),
    }
}

pub async fn invites(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let body = html! { div #invites data-src="/frag/invites" data-live="invites" { (invites_frag(&ctx).await) } };
    Ok(render(&ctx, Page::new("/invites", "Invites", "Invite links for every bot in the swarm.", body)))
}
