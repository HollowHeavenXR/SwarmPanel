//! Fleet metrics, stability, event feed, history + anomalies (port of metrics.lua).

use crate::app::AppRef;
use crate::db::{J, P};
use futures_util::future::join_all;
use serde_json::{Value, json};

pub async fn snapshot(app: &AppRef) -> Value {
    let per = join_all(app.settings.bots.iter().map(|bot| async move {
        let (s, k) = (bot.db_schema.as_str(), bot.key);
        let ex = app.db.existing_tables(s, &[format!("{k}_metrics"), format!("{k}_voice_state")]).await;
        let mut err = None;
        let mut metrics = Vec::new();
        if ex.contains(&format!("{k}_metrics")) {
            let voice = ex.contains(&format!("{k}_voice_state"));
            let (vcols, vjoin) = if voice {
                (
                    ", v.last_channel_id, v.connected_channel_id AS voice_state_connected_channel_id, v.desired_connected, v.reconnect_attempts, v.last_error AS voice_last_error, EXTRACT(EPOCH FROM (NOW() - v.last_seen_at))::int AS voice_age_seconds",
                    format!("LEFT JOIN {k}_voice_state v ON v.guild_id = m.guild_id AND v.bot_name = m.bot_name"),
                )
            } else {
                ("", String::new())
            };
            match app
                .db
                .query(
                    s,
                    &format!(
                        "SELECT m.guild_id, m.voice_connected, m.connected_channel_id, m.player_connected, m.player_playing, m.player_paused, m.queue_count, m.backup_queue_count, m.is_playing_db, m.is_paused_db, \
                         m.position_seconds, m.recovery_pending, m.lavalink_ready, m.last_error, EXTRACT(EPOCH FROM (NOW() - m.updated_at))::int AS metrics_age_seconds {vcols} \
                         FROM {k}_metrics m {vjoin} WHERE m.bot_name = $1 ORDER BY m.updated_at DESC LIMIT 200"
                    ),
                    &[k.into()],
                )
                .await
            {
                Err(_) => err = Some("metrics query failed"),
                Ok(rows) => {
                    for r in rows {
                        let age = r.i("metrics_age_seconds").unwrap_or(0);
                        metrics.push(json!({
                            "guild_id": r.s("guild_id"), "voice_connected": r.b("voice_connected"),
                            "connected_channel_id": r.s("connected_channel_id").or_else(|| r.s("voice_state_connected_channel_id")).unwrap_or_default(),
                            "last_channel_id": r.s("last_channel_id").unwrap_or_default(), "desired_connected": r.b("desired_connected"),
                            "player_connected": r.b("player_connected"), "player_playing": r.b("player_playing"), "player_paused": r.b("player_paused"),
                            "queue_count": r.i("queue_count").unwrap_or(0), "backup_queue_count": r.i("backup_queue_count").unwrap_or(0),
                            "is_playing_db": r.b("is_playing_db"), "is_paused_db": r.b("is_paused_db"), "position_seconds": r.i("position_seconds").unwrap_or(0),
                            "recovery_pending": r.b("recovery_pending"), "lavalink_ready": r.b("lavalink_ready"), "reconnect_attempts": r.i("reconnect_attempts").unwrap_or(0),
                            "metrics_age_seconds": age, "voice_age_seconds": r.i("voice_age_seconds").unwrap_or(0), "stale": age > 45,
                            "last_error": r.s("last_error").or_else(|| r.s("voice_last_error")),
                        }));
                    }
                }
            }
        }
        let status = if err.is_some() { "error" } else if metrics.iter().any(|m| m.b("stale")) { "stale" } else { "ok" };
        json!({ "key": k, "display_name": bot.display_name, "schema": s, "metrics": metrics, "error": err, "status": status })
    }))
    .await;
    let mut t = json!({ "bots": per.len(), "guilds": 0, "voice_connected": 0, "playing": 0, "paused": 0, "queued_tracks": 0, "backup_tracks": 0, "recovering": 0, "lavalink_ready": 0, "stale_metrics": 0, "total_position_seconds": 0 });
    let add = |t: &mut Value, k: &str, n: i64| t[k] = json!(t.i(k).unwrap_or(0) + n);
    for b in &per {
        for m in b["metrics"].as_array().cloned().unwrap_or_default() {
            add(&mut t, "guilds", 1);
            add(&mut t, "voice_connected", m.b("voice_connected") as i64);
            if m.b("player_playing") {
                add(&mut t, "playing", 1);
                add(&mut t, "total_position_seconds", m.i("position_seconds").unwrap_or(0));
            }
            add(&mut t, "paused", m.b("player_paused") as i64);
            add(&mut t, "queued_tracks", m.i("queue_count").unwrap_or(0));
            add(&mut t, "backup_tracks", m.i("backup_queue_count").unwrap_or(0));
            add(&mut t, "recovering", m.b("recovery_pending") as i64);
            add(&mut t, "lavalink_ready", m.b("lavalink_ready") as i64);
            add(&mut t, "stale_metrics", m.b("stale") as i64);
        }
    }
    json!({ "generated_at": crate::util::iso_now(), "totals": t, "bots": per })
}

pub async fn ensure_history_schema(app: &AppRef) {
    let _ = app.db.batch("accountlogins", "CREATE INDEX IF NOT EXISTS swarm_metrics_history_key_time_idx ON swarm_metrics_history (metric_key, captured_at)").await;
}

pub async fn maybe_cleanup_history(app: &AppRef) {
    if rand::random_range(0..12) == 0 {
        let _ = app.db.exec("accountlogins", "DELETE FROM swarm_metrics_history WHERE captured_at < now() - interval '45 days'", &[]).await;
    }
}

async fn sample(app: &AppRef, key: &str, v: f64) {
    let _ = app.db.exec("accountlogins", "INSERT INTO swarm_metrics_history (captured_at, metric_key, metric_value, guild_id) VALUES (NOW(), $1, $2, NULL)", &[crate::util::cut(key, 60).into(), v.into()]).await;
}

/// Every 5 minutes: fleet totals + Aria into swarm_metrics_history.
pub async fn capture(app: &AppRef) {
    let s = snapshot(app).await;
    let t = &s["totals"];
    for (k, src) in [("active_bots", "bots"), ("voice_connected", "voice_connected"), ("playing", "playing"), ("queued_tracks", "queued_tracks"), ("backup_tracks", "backup_tracks"), ("connected_guilds", "guilds"), ("stale_metrics", "stale_metrics"), ("total_position_seconds", "total_position_seconds")] {
        if let Some(v) = t.f(src) {
            sample(app, k, v).await;
        }
    }
    let hb = app.db.opt("discord_aria", "SELECT EXTRACT(EPOCH FROM (NOW() - updated_at))::int AS age FROM aria_process_heartbeat WHERE id = 1", &[]).await.and_then(|r| r.i("age"));
    sample(app, "aria_online", if hb.is_some_and(|a| a < 120) { 1.0 } else { 0.0 }).await;
    if let Some(st) = app.db.opt("discord_aria", "SELECT memory_kb, uptime_seconds FROM aria_process_stats WHERE bot_name = 'aria'", &[]).await {
        if let Some(m) = st.f("memory_kb") {
            sample(app, "aria_memory_kb", m).await;
        }
        if let Some(u) = st.f("uptime_seconds") {
            sample(app, "aria_uptime_seconds", u).await;
        }
    }
}

pub async fn stability(app: &AppRef) -> Value {
    let (m, cd, rep) = tokio::join!(
        snapshot(app),
        app.db.query("discord_aria", "SELECT scope_key, scope_type, guild_id, bot_name, reason, EXTRACT(EPOCH FROM (cooldown_until - NOW()))::int AS remaining_seconds, cooldown_until, updated_at FROM aria_swarm_recovery_cooldowns WHERE cooldown_until > NOW() ORDER BY cooldown_until DESC LIMIT 50", &[]),
        app.db.query("discord_aria", "SELECT issue_type, repair_action, repair_scope, success, confidence, details, error_text, created_at FROM aria_repair_journal ORDER BY created_at DESC LIMIT 50", &[]),
    );
    let mut s = json!({ "cooldowns": [], "recent_repairs": [], "metrics": m, "status": "ok" });
    match cd {
        Ok(c) => s["cooldowns"] = json!(c),
        Err(e) => s["cooldowns_error"] = json!(e),
    }
    match rep {
        Ok(r) => s["recent_repairs"] = json!(r),
        Err(e) => s["repairs_error"] = json!(e),
    }
    s
}

pub async fn aria_medic_events(app: &AppRef, limit: i64) -> Vec<Value> {
    let b = limit.clamp(1, 50);
    let mut ev = Vec::new();
    for r in app.db.all("discord_aria", "SELECT event_type, bot_name, guild_id, severity, created_at FROM aria_swarm_events ORDER BY created_at DESC, id DESC LIMIT $1", &[b.into()]).await {
        let et = r.s("event_type").unwrap_or_else(|| "aria_event".into());
        let bot = r.s("bot_name").unwrap_or_else(|| "aria".into());
        let sev = r.s("severity").unwrap_or_else(|| "info".into()).to_lowercase();
        let level = match sev.as_str() {
            "critical" | "error" => "error",
            "warning" | "recoverable" | "degraded" => "warning",
            _ => "info",
        };
        let mut d = et.replace('_', " ");
        if !bot.is_empty() {
            d.push_str(&format!(" | bot={bot}"));
        }
        if let Some(g) = r.s("guild_id").filter(|g| !g.is_empty() && g != "0") {
            d.push_str(&format!(" | guild={g}"));
        }
        ev.push(json!({ "type": "aria_medic_event", "level": level, "title": "Aria Medic Event", "description": d, "source": "aria", "timestamp": r.s("created_at").unwrap_or_else(crate::util::iso_now) }));
    }
    let n = (b / 2 + 1).clamp(3, 10);
    for r in app.db.all("discord_aria", "SELECT target_name, action_name, issue_type, success, execution_mode, created_at FROM aria_infra_history ORDER BY created_at DESC, id DESC LIMIT $1", &[n.into()]).await {
        let mode = r.s("execution_mode").unwrap_or_else(|| "unknown".into());
        let level = if r.b("success") { "info" } else if mode == "planned" { "warning" } else { "error" };
        ev.push(json!({
            "type": "aria_infra_event", "level": level, "title": "Aria Infra Action",
            "description": format!("{} -> {} | {} | mode={mode}", r.s("action_name").unwrap_or_else(|| "action".into()), r.s("target_name").unwrap_or_else(|| "target".into()), r.s("issue_type").unwrap_or_else(|| "issue".into())),
            "source": "aria", "timestamp": r.s("created_at").unwrap_or_else(crate::util::iso_now),
        }));
    }
    ev.sort_by(|a, b| a.s("timestamp").cmp(&b.s("timestamp")));
    let start = ev.len().saturating_sub(b as usize);
    ev.split_off(start)
}

pub async fn bot_error_events(app: &AppRef, limit: i64) -> Vec<Value> {
    let n = app.settings.bots.len().max(1) as i64;
    let per = (limit / n + 2).clamp(3, 25);
    let lists = join_all(app.settings.bots.iter().map(|bot| async move {
        let (s, k) = (bot.db_schema.as_str(), bot.key);
        if !app.db.table_exists(s, &format!("{k}_error_events")).await {
            return vec![];
        }
        app.db
            .all(s, &format!("SELECT id, bot_name, guild_id, error_level, error_type, title, description, traceback_text, created_at FROM {k}_error_events ORDER BY created_at DESC, id DESC LIMIT $1"), &[per.into()])
            .await
            .into_iter()
            .map(|r| {
                let mut d = r.s("description").unwrap_or_default().trim().to_string();
                let tb = r.s("traceback_text").unwrap_or_default().trim().to_string();
                if !tb.is_empty() {
                    d = format!("{d}\n\n{tb}").trim().to_string();
                }
                json!({
                    "type": "bot_error", "level": r.s("error_level").unwrap_or_else(|| "error".into()).to_lowercase(), "title": r.s("title").unwrap_or_else(|| format!("{} Error", bot.display_name)),
                    "description": d, "source": k, "timestamp": r.s("created_at").unwrap_or_else(crate::util::iso_now), "bot_key": k, "guild_id": r.s("guild_id"), "error_type": r.s("error_type").unwrap_or_else(|| "runtime".into()),
                })
            })
            .collect::<Vec<_>>()
    }))
    .await;
    let mut ev: Vec<Value> = lists.into_iter().flatten().collect();
    ev.sort_by(|a, b| a.s("timestamp").cmp(&b.s("timestamp")));
    let cap = limit.clamp(1, 100) as usize;
    let start = ev.len().saturating_sub(cap);
    ev.split_off(start)
}

/// /api/events and the "events" live key: merged, sorted, deduped, trimmed.
pub async fn events(app: &AppRef, limit: i64) -> Vec<Value> {
    let (a, b) = tokio::join!(bot_error_events(app, limit), aria_medic_events(app, (limit / 2).max(5)));
    let mut all: Vec<Value> = a.into_iter().chain(b).collect();
    all.sort_by(|x, y| x.s("timestamp").cmp(&y.s("timestamp")));
    let mut seen = std::collections::HashSet::new();
    all.retain(|e| {
        seen.insert(format!(
            "{}\0{}\0{}\0{}\0{}",
            e.s("timestamp").unwrap_or_default(),
            e.s("source").unwrap_or_default(),
            e.s("title").unwrap_or_default(),
            e.s("description").unwrap_or_default(),
            e.s("type").unwrap_or_else(|| "feed_event".into())
        ))
    });
    let start = all.len().saturating_sub(limit as usize);
    all.split_off(start)
}

pub async fn history(app: &AppRef, metric: &str, hours: i64) -> Result<Vec<Value>, String> {
    app.db
        .query(
            "accountlogins",
            "SELECT captured_at, metric_value, guild_id FROM swarm_metrics_history WHERE metric_key = $1 AND captured_at > (NOW() AT TIME ZONE 'UTC' - make_interval(hours => $2::int)) ORDER BY captured_at ASC",
            &[crate::util::cut(metric, 60).into(), P::Int(hours.clamp(1, 720))],
        )
        .await
}

pub async fn anomalies(app: &AppRef, metric: &str, hours: i64) -> Result<Vec<Value>, String> {
    let pts = history(app, metric, hours).await?;
    let vals: Vec<f64> = pts.iter().map(|p| p.f("metric_value").unwrap_or(0.0)).collect();
    if vals.len() < 5 {
        return Ok(vec![]);
    }
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let sd = (vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64).sqrt();
    if sd == 0.0 {
        return Ok(vec![]);
    }
    Ok(pts
        .into_iter()
        .filter_map(|mut p| {
            let z = (p.f("metric_value").unwrap_or(0.0) - mean).abs() / sd;
            (z > 2.5).then(|| {
                p["mean"] = json!((mean * 100.0).round() / 100.0);
                p["z_score"] = json!((z * 100.0).round() / 100.0);
                p
            })
        })
        .collect())
}
