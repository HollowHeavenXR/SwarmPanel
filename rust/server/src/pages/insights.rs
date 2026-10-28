//! Insights: Leaderboard, Learning, plus Admin's Bot Telemetry.

use super::ui::{self, arr, n, s};
use super::{Ctx, Page, allowed, denied, render};
use crate::api::fleet as flt;
use crate::db::J;
use crate::http::{R, Reply, Req};
use maud::{Markup, html};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn bot_select(ctx: &Ctx, name: &str, selected: &str, all: Option<&str>) -> Markup {
    html! {
        select name=(name) {
            @if let Some(l) = all { option value="" { (l) } }
            @for b in ctx.app().settings.bots.iter().filter(|b| b.is_music()) { option value=(b.key) selected[b.key == selected] { (b.display_name) } }
        }
    }
}

fn track_table(rows: &[Value], with_bot: bool) -> Markup {
    if rows.is_empty() {
        return ui::empty("No data yet.");
    }
    html! {
        div.table-wrap { table.table {
            thead { tr { th.num { "#" } th { "Track" } @if with_bot { th { "Bot" } } th.num { "Plays" } th.num { "Finishes" } th.num { "Skips" } th.num { "Likes" } th.num { "Score" } } }
            tbody { @for (i, t) in rows.iter().enumerate() { tr {
                td.num { (i + 1) }
                td { @match t.s("video_url").as_deref().and_then(ui::safe_url) { Some(u) => { a href=(u) target="_blank" rel="noreferrer" { (t.s("title").unwrap_or_else(|| "Unknown".into())) } }, None => { (t.s("title").unwrap_or_else(|| "Unknown".into())) } } }
                @if with_bot { td { (s(t, "bot_display")) } }
                td.num { (t.i("play_count").or(t.i("plays")).unwrap_or(0)) }
                td.num { (n(t, "finish_count")) } td.num { (n(t, "skip_count")) } td.num { (n(t, "like_count")) } td.num { (n(t, "smart_score")) }
            } } }
        } }
    }
}

fn default_guild(ctx: &Ctx, data: &Value) -> String {
    ctx.own_guild()
        .or_else(|| ctx.q("guild_id"))
        .or_else(|| arr(data, "bots").iter().flat_map(|b| arr(b, "sessions")).find_map(|x| x.s("guild_id")))
        .unwrap_or_default()
}

pub async fn leaderboard(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let data = crate::fleet::dashboard_payload(ctx.app(), &ctx.a, false).await;
    let bot = ctx.q("bot_key").or_else(|| ctx.app().settings.bots.iter().find(|b| b.is_music()).map(|b| b.key.to_string())).unwrap_or_default();
    let guild = default_guild(&ctx, &data);
    let lb = if guild.is_empty() { Err("Enter a guild ID.".to_string()) } else { ctx.api(flt::guild_leaderboard, &[("bot_key", bot.clone()), ("limit", "20".into())], &[("guild_id", guild.clone())]).await };
    let swarm = if ctx.a.is_admin() { Some(ctx.api(flt::swarm_leaderboard, &[("days", "7".into()), ("limit", "20".into())], &[]).await) } else { None };
    let body = html! {
        form.filter-bar method="get" action="/leaderboard" data-autosubmit {
            label.field { span { "Bot" } (bot_select(&ctx, "bot_key", &bot, None)) }
            label.field { span { "Guild ID" } input type="text" name="guild_id" value=(guild) readonly[ctx.own_guild().is_some()] inputmode="numeric"; }
            button.btn.primary type="submit" { "Load" }
        }
        @match &lb {
            Ok(v) => {
                @let d = v.get("data").cloned().unwrap_or(json!({}));
                div.split {
                    section.card { (ui::section("Top tracks", Some("Ranked by smart score in this guild."), None)) (track_table(arr(&d, "top_tracks"), false)) }
                    section.card {
                        (ui::section("Top listeners", None, None))
                        @let ls = arr(&d, "top_listeners");
                        @if ls.is_empty() { (ui::empty("No listener data yet.")) } @else {
                            div.table-wrap { table.table { thead { tr { th { "User ID" } th.num { "Tracks" } th.num { "Plays" } th.num { "Taste" } } }
                                tbody { @for l in ls { tr { td.mono { (s(l, "user_id")) } td.num { (n(l, "track_count")) } td.num { (n(l, "play_count")) } td.num { (n(l, "taste_score")) } } } } } }
                        }
                    }
                }
            },
            Err(e) => { (ui::notice("info", e)) },
        }
        @if let Some(sw) = swarm {
            section.card {
                (ui::section("Swarm-wide, last 7 days", Some("Admin only: aggregated across every bot's database."), None))
                @match sw { Ok(v) => { (track_table(arr(&v, "tracks"), true)) }, Err(e) => { (ui::notice("error", &e)) } }
            }
        }
    };
    Ok(render(&ctx, Page::new("/leaderboard", "Leaderboard", "Top tracks and listeners.", body)))
}

pub async fn learning(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let bot = ctx.q("bot_key").unwrap_or_default();
    let guild = ctx.own_guild().or_else(|| ctx.q("guild_id")).unwrap_or_default();
    let res = ctx.api(flt::music_intelligence, &[("bot_key", bot.clone()), ("guild_id", guild.clone())], &[]).await;
    let body = html! {
        form.filter-bar method="get" action="/learning" data-autosubmit {
            label.field { span { "Bot" } (bot_select(&ctx, "bot_key", &bot, Some("All bots"))) }
            label.field { span { @if ctx.own_guild().is_some() { "Guild ID" } @else { "Guild ID (blank = fleet-wide)" } } input type="text" name="guild_id" value=(guild) readonly[ctx.own_guild().is_some()] inputmode="numeric"; }
            button.btn.primary type="submit" { "Load" }
        }
        @match &res {
            Ok(v) => {
                @let d = v.get("data").cloned().unwrap_or(json!({}));
                @let t = d.get("totals").cloned().unwrap_or(json!({}));
                div.metrics {
                    @for (k, l) in [("learned_tracks", "Learned tracks"), ("plays", "Plays"), ("finishes", "Finishes"), ("skips", "Skips"), ("likes", "Likes"), ("dislikes", "Dislikes"), ("recommendations", "Smart recs")] {
                        (ui::metric(l, n(&t, k), None))
                    }
                }
                @let bots = arr(&d, "bots");
                @if bots.is_empty() { (ui::empty("No music intelligence data yet.")) }
                @for b in bots {
                    section.card {
                        (ui::section(&b.s("bot_display").unwrap_or(s(b, "bot_key")), Some(&format!("{} learned tracks · {} smart recommendations", n(b, "learned_tracks"), n(b, "recommendations"))), None))
                        (track_table(arr(b, "top_tracks"), false))
                    }
                }
            },
            Err(e) => { (ui::notice("error", e)) },
        }
    };
    Ok(render(&ctx, Page::new("/learning", "Learning", "What the smart-recommendation engine has learned.", body)))
}

pub async fn telemetry(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    if !allowed(&ctx.a, "/bot-telemetry") {
        return Ok(denied(&ctx, "/bot-telemetry", "Bot Telemetry", "Admin access required."));
    }
    let bot = ctx.q("bot_key").or_else(|| ctx.app().settings.bots.iter().find(|b| b.is_music()).map(|b| b.key.to_string())).unwrap_or_default();
    let cat = ctx.q("category").unwrap_or_default();
    let limit = ctx.q("limit").unwrap_or_else(|| "100".into());
    let res = ctx.api(flt::bot_telemetry, &[("bot_key", bot.clone()), ("category", cat.clone()), ("limit", limit.clone())], &[]).await;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    if let Ok(v) = &res {
        for r in arr(v, "events") {
            *counts.entry(s(r, "event_name")).or_default() += 1;
        }
    }
    let mut top: Vec<(String, i64)> = counts.into_iter().collect();
    top.sort_by_key(|(_, c)| -c);
    let body = html! {
        form.filter-bar method="get" action="/bot-telemetry" {
            label.field { span { "Bot" } (bot_select(&ctx, "bot_key", &bot, None)) }
            label.field { span { "Category" } input type="text" name="category" value=(cat) placeholder="e.g. runtime"; }
            label.field.narrow { span { "Limit" } input type="number" name="limit" value=(limit) min="1" max="1000"; }
            button.btn.primary type="submit" { "Load" }
        }
        @match &res {
            Ok(v) => {
                @let rows = arr(v, "events");
                @if !top.is_empty() { div.metrics { @for (name, c) in top.iter().take(6) { (ui::metric(name, c, None)) } } }
                (ui::section("Recent events", Some(&ui::plural(rows.len() as i64, "event", "events")), None))
                @if rows.is_empty() { (ui::empty("No telemetry events recorded yet for this bot.")) } @else {
                    div.table-wrap { table.table {
                        thead { tr { th { "Time" } th { "Category" } th { "Event" } th { "Guild" } th.num { "Value" } th { "Detail" } } }
                        tbody { @for r in rows { tr {
                            td.nowrap { (ui::time(r.s("created_at"), "datetime")) }
                            td { (s(r, "event_category")) } td { (s(r, "event_name")) } td.mono { (s(r, "guild_id")) }
                            td.num { @if let Some(f) = r.f("numeric_value") { (format!("{f:.1}")) } }
                            td { (s(r, "text_value")) }
                        } } }
                    } }
                }
            },
            Err(e) => { (ui::notice("error", e)) },
        }
    };
    Ok(render(&ctx, Page::new("/bot-telemetry", "Bot Telemetry", "Raw diagnostic events straight from one bot's own database.", body)))
}
