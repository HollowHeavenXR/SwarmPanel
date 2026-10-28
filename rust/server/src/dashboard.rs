//! Fleet snapshot (port of dashboard.lua). Every bot's own database is read
//! in parallel; the whole snapshot is cached ~1.5s so many viewers share one
//! build. Lavalink voice pings / Redis node health are gone with Lavalink.

use crate::app::AppRef;
use crate::config::Bot;
use crate::db::{J, P};
use futures_util::future::join_all;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

fn has_chan(v: Option<&str>) -> bool {
    v.is_some_and(|s| !s.is_empty() && s != "0")
}

fn derive_state(playing: bool, paused: bool, has_track: bool, has_channel: bool, queue: i64, has_settings: bool, home: Option<&str>, backup: i64) -> (&'static str, &'static str) {
    let recovery = has_chan(home) && (has_track || queue > 0 || backup > 0);
    if paused && has_track && has_channel {
        return ("paused", "Paused");
    }
    if playing && has_track && has_channel {
        return ("playing", "Playing");
    }
    if has_track && has_channel {
        return ("paused", "Paused");
    }
    if recovery {
        return ("recovering", "Recovery Pending");
    }
    if queue > 0 {
        return ("queued", "Queued");
    }
    if has_settings || has_chan(home) {
        return ("configured", "Configured");
    }
    ("idle", "Idle")
}

const YT_HOSTS: &[&str] = &["youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be", "www.youtu.be"];

fn split_url(u: &str) -> Option<(String, String, String)> {
    let rest = u.trim().strip_prefix("https://").or_else(|| u.trim().strip_prefix("http://"))?;
    let (host, pq) = rest.split_once('/').map(|(h, r)| (h, format!("/{r}"))).unwrap_or((rest, "/".into()));
    let (path, query) = pq.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((pq.clone(), String::new()));
    Some((host.to_lowercase(), path, query))
}

pub fn youtube_id(u: &str) -> Option<String> {
    let (host, path, query) = split_url(u)?;
    if !YT_HOSTS.contains(&host.as_str()) {
        return None;
    }
    if host.ends_with("youtu.be") {
        return path.trim_start_matches('/').split('/').next().filter(|s| !s.is_empty()).map(str::to_string);
    }
    if path == "/watch" {
        return query.split('&').find_map(|kv| kv.strip_prefix("v=")).filter(|v| !v.is_empty()).map(str::to_string);
    }
    let mut parts = path.trim_start_matches('/').split('/');
    let (a, b) = (parts.next()?, parts.next()?);
    (matches!(a, "shorts" | "embed" | "live") && !b.is_empty()).then(|| b.to_string())
}

pub fn thumbnail(u: Option<&str>) -> Option<String> {
    youtube_id(u?).map(|id| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
}

fn media_source(u: Option<&str>, forced_local: bool) -> (&'static str, &'static str) {
    let u = u.unwrap_or("");
    if u.is_empty() {
        return ("unknown", "Unknown");
    }
    match split_url(u) {
        Some((h, _, _)) if YT_HOSTS.contains(&h.as_str()) => {
            if forced_local { ("local_download", "Local Download") } else { ("youtube", "Streaming (YouTube)") }
        }
        Some((h, _, _)) if h.ends_with("soundcloud.com") => ("soundcloud", "Streaming (SoundCloud)"),
        Some(_) => ("link", "Streaming (Direct Link)"),
        None => ("search", "Search"),
    }
}

fn gid(v: &Value) -> Option<String> {
    v.s("guild_id")
}

/// One bot's snapshot (sessions per guild + heartbeat).
pub async fn music_bot_snapshot(app: &AppRef, bot: &Bot) -> Value {
    let db = &app.db;
    let schema = bot.db_schema.as_str();
    let k = bot.key;
    let t = |s: &str| format!("{k}_{s}");
    let names: Vec<String> = ["playback_state", "guild_settings", "queue", "queue_backup", "bot_home_channels", "metrics", "track_intelligence", "smart_recommendations", "playback_events", "error_events"]
        .iter()
        .map(|s| t(s))
        .chain(std::iter::once("swarm_health".to_string()))
        .collect();
    let exists = db.existing_tables(schema, &names).await;
    let has = |s: &str| exists.contains(&t(s));
    let mut known: BTreeSet<String> = BTreeSet::new();
    let bk: P = k.into();

    let (playback, settings, queue, backup, home, metrics, intel, reco) = tokio::join!(
        async { if has("playback_state") { db.all(schema, &format!("SELECT * FROM {} ORDER BY guild_id LIMIT 500", t("playback_state")), &[]).await } else { vec![] } },
        async { if has("guild_settings") { db.all(schema, &format!("SELECT guild_id, filter_mode, loop_mode, transition_mode, fade_seconds, fade_curve FROM {} LIMIT 500", t("guild_settings")), &[]).await } else { vec![] } },
        async { if has("queue") { db.all(schema, &format!("SELECT guild_id, COUNT(*) AS queue_len FROM {} WHERE bot_name = $1 GROUP BY guild_id", t("queue")), &[bk.clone()]).await } else { vec![] } },
        async { if has("queue_backup") { db.all(schema, &format!("SELECT guild_id, COUNT(*) AS backup_len FROM {} WHERE bot_name = $1 GROUP BY guild_id", t("queue_backup")), &[bk.clone()]).await } else { vec![] } },
        async { if has("bot_home_channels") { db.all(schema, &format!("SELECT guild_id, home_vc_id FROM {} WHERE bot_name = $1 LIMIT 500", t("bot_home_channels")), &[bk.clone()]).await } else { vec![] } },
        async { if has("metrics") { db.all(schema, &format!("SELECT *, EXTRACT(EPOCH FROM (NOW() - updated_at))::int AS metric_age_seconds FROM {} WHERE bot_name = $1 LIMIT 500", t("metrics")), &[bk.clone()]).await } else { vec![] } },
        async { if has("track_intelligence") { db.all(schema, &format!("SELECT guild_id, COUNT(*) AS learned_tracks, COALESCE(SUM(play_count), 0) AS plays, COALESCE(SUM(like_count), 0) AS likes, COALESCE(SUM(dislike_count), 0) AS dislikes FROM {} GROUP BY guild_id LIMIT 500", t("track_intelligence")), &[]).await } else { vec![] } },
        async { if has("smart_recommendations") { db.all(schema, &format!("SELECT guild_id, COUNT(*) AS recommendations FROM {} GROUP BY guild_id LIMIT 500", t("smart_recommendations")), &[]).await } else { vec![] } },
    );
    let index = |rows: &Vec<Value>, known: &mut BTreeSet<String>| -> HashMap<String, Value> {
        rows.iter().filter_map(|r| gid(r).map(|g| (g, r.clone()))).inspect(|(g, _)| { known.insert(g.clone()); }).collect()
    };
    let pb = index(&playback, &mut known);
    let st = index(&settings, &mut known);
    let qu = index(&queue, &mut known);
    let bu = index(&backup, &mut known);
    let ho = index(&home, &mut known);
    let me = index(&metrics, &mut known);
    let it = index(&intel, &mut known);
    let re = index(&reco, &mut known);

    let mut forced: HashMap<String, bool> = HashMap::new();
    if has("playback_events") {
        let uids: Vec<String> = pb.values().filter_map(|r| r.s("track_uid")).map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect::<BTreeSet<_>>().into_iter().collect();
        if !uids.is_empty() {
            let list = uids.iter().map(|u| format!("'{}'", u.replace('\'', "''"))).collect::<Vec<_>>().join(", ");
            for r in db.all(schema, &format!("SELECT TRIM(track_uid) AS track_uid, forced_local FROM {} WHERE event_type = 'track_start' AND TRIM(track_uid) IN ({list}) ORDER BY created_at DESC", t("playback_events")), &[]).await {
                if let Some(u) = r.s("track_uid") {
                    forced.entry(u).or_insert(r.b("forced_local"));
                }
            }
        }
    }

    // Numeric-aware sort (shorter ids first), like the Lua.
    let mut gids: Vec<String> = known.into_iter().collect();
    gids.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
    let empty = json!({});
    let mut sessions = Vec::new();
    let mut active = 0;
    for g in &gids {
        let p = pb.get(g).unwrap_or(&empty);
        let m = me.get(g);
        let s = st.get(g);
        let qn = qu.get(g).and_then(|r| r.i("queue_len")).unwrap_or(0);
        let bn = bu.get(g).and_then(|r| r.i("backup_len")).unwrap_or(0);
        let home_id = ho.get(g).and_then(|r| r.s("home_vc_id"));
        let video = p.s("video_url");
        let uid = p.s("track_uid").map(|u| u.trim().to_string());
        let (src_key, src_label) = media_source(video.as_deref(), uid.as_ref().and_then(|u| forced.get(u)).copied().unwrap_or(false));
        let age = m.and_then(|m| m.i("metric_age_seconds")).unwrap_or(999_999);
        let fresh = m.is_some() && age <= 90;
        let mm = m.unwrap_or(&empty);
        let playing = if fresh { mm.b("player_playing") } else { p.b("is_playing") };
        let paused = if fresh { mm.b("player_paused") } else { p.b("is_paused") };
        let chan = if fresh { mm.s("connected_channel_id").or_else(|| p.s("channel_id")) } else { p.s("channel_id") };
        let pos = mm.i("position_seconds").or_else(|| p.i("position_seconds")).unwrap_or(0);
        let dur = mm.i("duration_seconds").or_else(|| mm.i("track_length_seconds")).or_else(|| p.i("duration_seconds")).or_else(|| p.i("length_seconds")).unwrap_or(0);
        let observed = if fresh { mm.get("updated_at").cloned() } else { p.get("updated_at").cloned() };
        let has_track = p.s("title").is_some_and(|t| !t.is_empty()) || video.as_deref().is_some_and(|v| !v.is_empty());
        let (state, label) = derive_state(playing, paused, has_track, chan.is_some(), qn, s.is_some(), home_id.as_deref(), bn);
        if playing {
            active += 1;
        }
        let ss = s.unwrap_or(&empty);
        sessions.push(json!({
            "guild_id": g, "channel_id": chan, "title": p.get("title"), "video_url": video,
            "media_source": src_key, "media_source_label": src_label, "thumbnail": thumbnail(video.as_deref()),
            "position_seconds": pos, "duration_seconds": dur, "position_observed_at": observed,
            "is_playing": playing, "is_paused": paused, "metric_age_seconds": m.map(|_| age),
            "session_state": state, "session_state_label": label,
            "filter_mode": ss.s("filter_mode").unwrap_or_else(|| "none".into()), "loop_mode": ss.s("loop_mode").unwrap_or_else(|| "queue".into()),
            "transition_mode": ss.s("transition_mode").unwrap_or_else(|| "off".into()), "fade_seconds": ss.f("fade_seconds").unwrap_or(5.0),
            "fade_curve": ss.s("fade_curve").unwrap_or_else(|| "linear".into()),
            "queue_count": qn, "backup_queue_count": bn,
            "backup_restore_ready": bn > 0 && matches!(state, "recovering" | "queued" | "configured" | "idle" | "paused"),
            "home_channel_id": home_id, "ping_ms": null,
            "smart": {
                "learned_tracks": it.get(g).and_then(|r| r.i("learned_tracks")).unwrap_or(0), "plays": it.get(g).and_then(|r| r.i("plays")).unwrap_or(0),
                "likes": it.get(g).and_then(|r| r.i("likes")).unwrap_or(0), "dislikes": it.get(g).and_then(|r| r.i("dislikes")).unwrap_or(0),
                "recommendations": re.get(g).and_then(|r| r.i("recommendations")).unwrap_or(0),
            },
        }));
    }
    let (mut hb_age, mut hb_status, mut mode) = (None, "unknown".to_string(), None);
    if exists.contains("swarm_health") {
        if let Some(r) = db.opt(schema, "SELECT status, playback_mode, EXTRACT(EPOCH FROM (NOW() - last_pulse))::int AS heartbeat_age FROM swarm_health WHERE bot_name = $1 LIMIT 1", &[k.into()]).await {
            hb_age = Some(r.i("heartbeat_age").unwrap_or(0));
            hb_status = r.s("status").unwrap_or_else(|| "unknown".into());
            mode = r.s("playback_mode");
        }
    }
    let voice_degraded = has("error_events")
        && db
            .opt(schema, &format!("SELECT COUNT(*) AS n FROM {} WHERE error_type IN ('voice_hard_invalidation', 'voice_recovery_giveup') AND created_at > NOW() - INTERVAL '12 minutes'", t("error_events")), &[])
            .await
            .and_then(|r| r.i("n"))
            .unwrap_or(0)
            > 0;
    let status = match hb_age {
        None => "unknown",
        Some(a) if a > 120 => "stale",
        Some(_) if voice_degraded => "degraded",
        Some(_) => "online",
    };
    let qd: i64 = sessions.iter().map(|s| s.i("queue_count").unwrap_or(0)).sum();
    let bd: i64 = sessions.iter().map(|s| s.i("backup_queue_count").unwrap_or(0)).sum();
    json!({
        "key": k, "display_name": bot.display_name, "kind": bot.kind, "schema": schema, "status": status,
        "heartbeat_age_seconds": hb_age, "heartbeat_status": hb_status, "playback_mode": mode,
        "active_playing_count": active, "known_guild_count": gids.len(), "queue_depth": qd, "backup_queue_depth": bd,
        "sessions": sessions, "known_guild_ids": gids,
    })
}

pub async fn get_bot_control_state(app: &AppRef, bot: &Bot, guild_id: &str) -> Value {
    let snap = music_bot_snapshot(app, bot).await;
    if let Some(s) = snap["sessions"].as_array().and_then(|a| a.iter().find(|s| s.s("guild_id").as_deref() == Some(guild_id))) {
        let mut st = s.clone();
        st["bot_key"] = json!(bot.key);
        st["bot_display"] = json!(bot.display_name);
        for f in ["status", "heartbeat_age_seconds", "heartbeat_status"] {
            st[f] = snap[f].clone();
        }
        return st;
    }
    json!({
        "bot_key": bot.key, "bot_display": bot.display_name, "guild_id": guild_id, "status": snap["status"], "heartbeat_age_seconds": snap["heartbeat_age_seconds"],
        "heartbeat_status": snap["heartbeat_status"], "session_state": "idle", "session_state_label": "Idle", "is_playing": false, "is_paused": false,
        "queue_count": 0, "backup_queue_count": 0, "backup_restore_ready": false, "filter_mode": "none", "loop_mode": "queue", "transition_mode": "off", "fade_seconds": 5.0, "fade_curve": "linear",
    })
}

const SMART: &str = "((finish_count * 3) + (like_count * 5) + play_count - (skip_count * 2) - (dislike_count * 5))";

pub async fn guild_leaderboard(app: &AppRef, bot: &Bot, guild_id: &str, limit: i64) -> Value {
    let lim = limit.clamp(1, 50);
    let (schema, k) = (bot.db_schema.as_str(), bot.key);
    let ex = app.db.existing_tables(schema, &[format!("{k}_track_intelligence"), format!("{k}_user_track_affinity")]).await;
    let tracks = if ex.contains(&format!("{k}_track_intelligence")) {
        app.db
            .all(schema, &format!("SELECT title, video_url, play_count, finish_count, skip_count, like_count, dislike_count, {SMART} AS smart_score FROM {k}_track_intelligence WHERE guild_id = $1 ORDER BY smart_score DESC, play_count DESC LIMIT $2"), &[guild_id.into(), lim.into()])
            .await
    } else {
        vec![]
    };
    let listeners = if ex.contains(&format!("{k}_user_track_affinity")) {
        app.db
            .all(schema, &format!("SELECT user_id, COUNT(*) AS track_count, COALESCE(SUM(play_count), 0) AS play_count, COALESCE(SUM(score), 0) AS taste_score FROM {k}_user_track_affinity WHERE guild_id = $1 GROUP BY user_id ORDER BY taste_score DESC, play_count DESC LIMIT $2"), &[guild_id.into(), lim.into()])
            .await
    } else {
        vec![]
    };
    json!({ "guild_id": guild_id, "bot_key": k, "top_tracks": tracks, "top_listeners": listeners })
}

pub async fn music_intelligence(app: &AppRef, guild_id: Option<&str>, bot_key: Option<&str>, limit: i64) -> Result<Value, String> {
    let lim = limit.clamp(1, 25);
    let bots: Vec<&Bot> = match bot_key.filter(|b| !b.is_empty()) {
        Some(b) => vec![app.settings.music_bot(b).ok_or("Unknown bot key")?],
        None => app.settings.bots.iter().collect(),
    };
    let g = guild_id.filter(|g| !g.is_empty());
    let per = join_all(bots.iter().map(|bot| async move {
        let (schema, k) = (bot.db_schema.as_str(), bot.key);
        let ex = app.db.existing_tables(schema, &[format!("{k}_track_intelligence"), format!("{k}_smart_recommendations")]).await;
        if !ex.contains(&format!("{k}_track_intelligence")) {
            return None;
        }
        let (where_, params): (&str, Vec<P>) = match g {
            Some(g) => ("WHERE guild_id = $1", vec![g.into()]),
            None => ("", vec![]),
        };
        let s = app
            .db
            .opt(schema, &format!("SELECT COUNT(*) AS learned_tracks, COALESCE(SUM(play_count), 0) AS plays, COALESCE(SUM(finish_count), 0) AS finishes, COALESCE(SUM(skip_count), 0) AS skips, COALESCE(SUM(like_count), 0) AS likes, COALESCE(SUM(dislike_count), 0) AS dislikes FROM {k}_track_intelligence {where_}"), &params)
            .await
            .unwrap_or_else(|| json!({}));
        let mut tp = params.clone();
        tp.push(lim.into());
        let top = app
            .db
            .all(schema, &format!("SELECT guild_id::text AS guild_id, title, video_url, play_count, finish_count, skip_count, like_count, dislike_count, {SMART} AS smart_score, updated_at FROM {k}_track_intelligence {where_} ORDER BY smart_score DESC, updated_at DESC LIMIT ${}", tp.len()), &tp)
            .await;
        let recs = if ex.contains(&format!("{k}_smart_recommendations")) {
            app.db.opt(schema, &format!("SELECT COUNT(*) AS recommendations FROM {k}_smart_recommendations {where_}"), &params).await.and_then(|r| r.i("recommendations")).unwrap_or(0)
        } else {
            0
        };
        let n = |f: &str| s.i(f).unwrap_or(0);
        Some(json!({
            "bot_key": k, "bot_display": bot.display_name, "schema": schema, "learned_tracks": n("learned_tracks"), "plays": n("plays"), "finishes": n("finishes"),
            "skips": n("skips"), "likes": n("likes"), "dislikes": n("dislikes"), "recommendations": recs, "top_tracks": top,
        }))
    }))
    .await;
    let rows: Vec<Value> = per.into_iter().flatten().collect();
    let sum = |f: &str| rows.iter().map(|r| r.i(f).unwrap_or(0)).sum::<i64>();
    Ok(json!({
        "totals": { "learned_tracks": sum("learned_tracks"), "plays": sum("plays"), "finishes": sum("finishes"), "skips": sum("skips"), "likes": sum("likes"), "dislikes": sum("dislikes"), "recommendations": sum("recommendations") },
        "bots": rows,
    }))
}

static ARIA_LAST: AtomicU8 = AtomicU8::new(2); // 2 = unknown

async fn aria_card(app: &AppRef) -> Value {
    let db = &app.db;
    let a = "discord_aria";
    let count = |sql: &'static str| async move { db.opt(a, sql, &[]).await.and_then(|r| r.i("total")).unwrap_or(0) };
    let (hb, stats, n, recent, pr, pi, crit, recov, dec, infra, evs) = tokio::join!(
        db.opt(a, "SELECT EXTRACT(EPOCH FROM (NOW() - updated_at))::int AS age FROM aria_process_heartbeat WHERE id = 1", &[]),
        db.opt(a, "SELECT memory_kb, uptime_seconds FROM aria_process_stats WHERE bot_name = 'aria'", &[]),
        count("SELECT COUNT(*) AS total FROM aria_interactions"),
        db.all(a, "SELECT guild_id, channel_id, user_id, user_name, interaction_type, prompt_text, response_text, created_at FROM aria_interactions ORDER BY created_at DESC, id DESC LIMIT 6", &[]),
        count("SELECT COUNT(*) AS total FROM aria_repair_tasks WHERE status='pending'"),
        count("SELECT COUNT(*) AS total FROM aria_infra_tasks WHERE status='pending'"),
        count("SELECT COUNT(*) AS total FROM aria_swarm_health WHERE status_label='critical'"),
        count("SELECT COUNT(*) AS total FROM aria_swarm_health WHERE status_label IN ('recoverable','degraded')"),
        db.all(a, "SELECT issue_type, bot_name, guild_id, priority_score, urgency_label, created_at FROM aria_operator_decisions ORDER BY created_at DESC, id DESC LIMIT 5", &[]),
        db.all(a, "SELECT target_name, action_name, issue_type, success, execution_mode, result_text, created_at FROM aria_infra_history ORDER BY created_at DESC, id DESC LIMIT 5", &[]),
        db.all(a, "SELECT event_type, bot_name, guild_id, severity, created_at FROM aria_swarm_events ORDER BY created_at DESC, id DESC LIMIT 6", &[]),
    );
    let age = hb.and_then(|r| r.i("age"));
    let online = age.is_some_and(|a| a < 120);
    let (mem, up) = (stats.as_ref().and_then(|s| s.i("memory_kb")), stats.as_ref().and_then(|s| s.i("uptime_seconds")));
    let prev = ARIA_LAST.swap(online as u8, Ordering::SeqCst);
    if prev != 2 && prev != online as u8 {
        app.telemetry.record(crate::telemetry::Event::new("aria", if online { "online" } else { "offline" }).num(age.map(|a| a as f64)).meta(json!({ "memory_kb": mem, "uptime_seconds": up })));
    }
    json!({
        "key": "aria", "display_name": "Aria", "kind": "orchestrator", "schema": a, "status": if online { "ONLINE" } else { "OFFLINE" },
        "heartbeat_age_seconds": age, "memory_kb": mem, "uptime_seconds": up, "recent_interactions": recent, "recent_interaction_count": n,
        "medic_summary": { "pending_repairs": pr, "pending_infra": pi, "critical_health": crit, "recoverable_health": recov, "recent_operator_decisions": dec, "recent_infra_history": infra, "recent_swarm_events": evs },
    })
}

static INDEXES: std::sync::Once = std::sync::Once::new();

async fn build(app: &AppRef) -> Value {
    let mut first = false;
    INDEXES.call_once(|| first = true);
    if first {
        let _ = app
            .db
            .batch(
                "discord_aria",
                "CREATE INDEX IF NOT EXISTS aria_swarm_events_created_id_idx ON aria_swarm_events (created_at DESC, id DESC);
                 CREATE INDEX IF NOT EXISTS aria_operator_decisions_created_id_idx ON aria_operator_decisions (created_at DESC, id DESC);
                 CREATE INDEX IF NOT EXISTS aria_infra_history_created_id_idx ON aria_infra_history (created_at DESC, id DESC);",
            )
            .await;
    }
    let (mut bots, aria) = tokio::join!(join_all(app.settings.bots.iter().map(|b| music_bot_snapshot(app, b))), aria_card(app));
    bots.push(aria);
    json!({ "generated_at": crate::util::iso_now(), "bots": bots, "node_health": {} })
}

/// Cached ~1.5s; concurrent callers during a rebuild wait for the same build.
pub async fn get_dashboard_data(app: &AppRef) -> Value {
    if let Some(v) = app.cache.dashboard.get("snap", Duration::from_millis(1500)) {
        return v;
    }
    let _guard = app.cache.build_lock.lock().await;
    if let Some(v) = app.cache.dashboard.get("snap", Duration::from_millis(1500)) {
        return v;
    }
    let v = build(app).await;
    app.cache.dashboard.put("snap", v.clone());
    v
}

pub fn empty_activity() -> Value {
    json!({ "top_tracks": [], "top_bots": [], "active_sessions": [], "total_plays": 0, "learned_tracks": 0, "smart_likes": 0, "smart_dislikes": 0, "smart_recommendations": 0, "top_smart_tracks": [] })
}

/// Per-guild activity summaries, cached 30s per guild (each miss queries every bot's database).
pub async fn music_activity_for_guilds(app: &AppRef, guild_ids: &[String]) -> HashMap<String, Value> {
    let ttl = std::time::Duration::from_secs(30);
    let mut out = HashMap::new();
    let mut missing = Vec::new();
    for g in guild_ids {
        match app.cache.misc.get(&format!("activity:{g}"), ttl) {
            Some(v) => {
                out.insert(g.clone(), v);
            }
            None => missing.push(g.clone()),
        }
    }
    if !missing.is_empty() {
        for (g, v) in music_activity_uncached(app, &missing).await {
            app.cache.misc.put(&format!("activity:{g}"), v.clone());
            out.insert(g, v);
        }
    }
    out
}

async fn music_activity_uncached(app: &AppRef, guild_ids: &[String]) -> HashMap<String, Value> {
    let mut norm: Vec<String> = Vec::new();
    for g in guild_ids {
        if !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()) && !norm.contains(g) {
            norm.push(g.clone());
        }
    }
    let mut out: HashMap<String, Value> = norm.iter().map(|g| (g.clone(), empty_activity())).collect();
    if norm.is_empty() {
        return out;
    }
    let inlist = norm.join(", ");
    struct PerBot {
        key: &'static str,
        display: &'static str,
        history: Vec<Value>,
        active: Vec<Value>,
        intel: Vec<Value>,
        top: Vec<Value>,
        reco: Vec<Value>,
    }
    let per: Vec<PerBot> = join_all(app.settings.bots.iter().map(|bot| {
        let inlist = inlist.clone();
        async move {
            let (s, k) = (bot.db_schema.as_str(), bot.key);
            let ex = app.db.existing_tables(s, &[format!("{k}_history"), format!("{k}_playback_state"), format!("{k}_track_intelligence"), format!("{k}_smart_recommendations")]).await;
            let on = |t: &str| ex.contains(&format!("{k}_{t}"));
            let (history, active, intel, top, reco) = tokio::join!(
                async { if on("history") { app.db.all(s, &format!("SELECT guild_id, title, video_url, COUNT(*) AS plays, MAX(played_at) AS last_played_at FROM {k}_history WHERE guild_id IN ({inlist}) AND title IS NOT NULL AND title != '' GROUP BY guild_id, title, video_url ORDER BY plays DESC, last_played_at DESC LIMIT 300"), &[]).await } else { vec![] } },
                async { if on("playback_state") { app.db.all(s, &format!("SELECT guild_id, title, video_url, is_playing, is_paused FROM {k}_playback_state WHERE guild_id IN ({inlist}) AND (is_playing = TRUE OR is_paused = TRUE)"), &[]).await } else { vec![] } },
                async { if on("track_intelligence") { app.db.all(s, &format!("SELECT guild_id, COUNT(*) AS learned_tracks, COALESCE(SUM(play_count),0) AS plays, COALESCE(SUM(like_count),0) AS likes, COALESCE(SUM(dislike_count),0) AS dislikes FROM {k}_track_intelligence WHERE guild_id IN ({inlist}) GROUP BY guild_id"), &[]).await } else { vec![] } },
                async { if on("track_intelligence") { app.db.all(s, &format!("SELECT guild_id, title, {SMART} AS smart_score FROM {k}_track_intelligence WHERE guild_id IN ({inlist}) ORDER BY smart_score DESC LIMIT 20"), &[]).await } else { vec![] } },
                async { if on("smart_recommendations") { app.db.all(s, &format!("SELECT guild_id, COUNT(*) AS recommendations FROM {k}_smart_recommendations WHERE guild_id IN ({inlist}) GROUP BY guild_id"), &[]).await } else { vec![] } },
            );
            PerBot { key: k, display: bot.display_name, history, active, intel, top, reco }
        }
    }))
    .await;
    let mut tracks: HashMap<String, Vec<(String, Value, i64)>> = HashMap::new();
    let mut bot_plays: HashMap<String, HashMap<&'static str, i64>> = HashMap::new();
    let mut active: HashMap<String, Vec<Value>> = HashMap::new();
    let mut smart: HashMap<String, Vec<Value>> = HashMap::new();
    for b in &per {
        for r in &b.history {
            let Some(g) = gid(r) else { continue };
            let title = r.s("title").unwrap_or_else(|| "Unknown Track".into()).trim().to_string();
            let plays = r.i("plays").unwrap_or(0);
            let list = tracks.entry(g.clone()).or_default();
            match list.iter_mut().find(|(t, _, _)| t.eq_ignore_ascii_case(&title)) {
                Some(e) => e.2 += plays,
                None => list.push((title, r.get("video_url").cloned().unwrap_or(Value::Null), plays)),
            }
            *bot_plays.entry(g).or_default().entry(b.key).or_default() += plays;
        }
        for r in &b.active {
            if let Some(g) = gid(r) {
                active.entry(g).or_default().push(json!({ "bot_key": b.key, "bot_display": b.display, "title": r.s("title").unwrap_or_else(|| "Unknown Track".into()), "video_url": r.get("video_url"), "is_playing": r.b("is_playing"), "is_paused": r.b("is_paused") }));
            }
        }
        for r in &b.intel {
            if let Some(s) = gid(r).and_then(|g| out.get_mut(&g)) {
                s["learned_tracks"] = json!(s.i("learned_tracks").unwrap_or(0) + r.i("learned_tracks").unwrap_or(0));
                s["smart_likes"] = json!(s.i("smart_likes").unwrap_or(0) + r.i("likes").unwrap_or(0));
                s["smart_dislikes"] = json!(s.i("smart_dislikes").unwrap_or(0) + r.i("dislikes").unwrap_or(0));
            }
        }
        for r in &b.top {
            if let Some(g) = gid(r) {
                smart.entry(g).or_default().push(json!({ "bot_key": b.key, "bot_display": b.display, "title": r.get("title"), "smart_score": r.f("smart_score").unwrap_or(0.0) }));
            }
        }
        for r in &b.reco {
            if let Some(s) = gid(r).and_then(|g| out.get_mut(&g)) {
                s["smart_recommendations"] = json!(s.i("smart_recommendations").unwrap_or(0) + r.i("recommendations").unwrap_or(0));
            }
        }
    }
    for g in &norm {
        let s = out.get_mut(g).unwrap();
        let mut tl = tracks.remove(g).unwrap_or_default();
        tl.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        let mut bl: Vec<(&str, i64)> = bot_plays.remove(g).unwrap_or_default().into_iter().collect();
        bl.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        s["total_plays"] = json!(bl.iter().map(|b| b.1).sum::<i64>());
        s["top_tracks"] = json!(tl.iter().take(3).map(|(t, u, p)| json!({ "title": t, "video_url": u, "plays": p })).collect::<Vec<_>>());
        s["top_bots"] = json!(bl.iter().take(3).map(|(k, p)| json!({ "bot_key": k, "bot_display": app.settings.bot(k).map(|b| b.display_name).unwrap_or(k), "plays": p })).collect::<Vec<_>>());
        s["active_sessions"] = json!(active.remove(g).unwrap_or_default().into_iter().take(4).collect::<Vec<_>>());
        let mut sm = smart.remove(g).unwrap_or_default();
        sm.sort_by(|a, b| b.f("smart_score").unwrap_or(0.0).total_cmp(&a.f("smart_score").unwrap_or(0.0)));
        sm.truncate(3);
        s["top_smart_tracks"] = json!(sm);
    }
    out
}

pub async fn swarm_leaderboard(app: &AppRef, days: Option<String>, limit: Option<String>) -> Value {
    let days = days.and_then(|d| d.parse::<f64>().ok()).map(|d| d as i64).unwrap_or(7).clamp(1, 365);
    let limit = limit.and_then(|d| d.parse::<f64>().ok()).map(|d| d as i64).unwrap_or(20).clamp(1, 100);
    let per_bot = limit.min(50);
    let results = join_all(app.settings.bots.iter().map(|bot| async move {
        let (s, k) = (bot.db_schema.as_str(), bot.key);
        if !app.db.table_exists(s, &format!("{k}_track_intelligence")).await {
            return None;
        }
        Some(
            app.db
                .query(
                    s,
                    &format!("SELECT title, video_url, play_count, finish_count, skip_count, like_count, dislike_count, last_played, {SMART} AS smart_score FROM {k}_track_intelligence WHERE last_played IS NOT NULL AND last_played >= NOW() - make_interval(days => $1::int) ORDER BY play_count DESC LIMIT $2"),
                    &[days.into(), per_bot.into()],
                )
                .await
                .map(|rows| {
                    rows.into_iter()
                        .map(|mut r| {
                            r["bot_key"] = json!(k);
                            r["bot_display"] = json!(bot.display_name);
                            for f in ["play_count", "finish_count", "skip_count", "like_count", "dislike_count", "smart_score"] {
                                r[f] = json!(r.i(f).unwrap_or(0));
                            }
                            r
                        })
                        .collect::<Vec<_>>()
                }),
        )
    }))
    .await;
    let (mut q, mut e, mut rows) = (0, 0, Vec::new());
    for r in results.into_iter().flatten() {
        q += 1;
        match r {
            Ok(v) => rows.extend(v),
            Err(_) => e += 1,
        }
    }
    rows.sort_by(|a, b| b.i("play_count").cmp(&a.i("play_count")).then(b.i("smart_score").cmp(&a.i("smart_score"))));
    rows.truncate(limit as usize);
    json!({ "window_days": days, "generated_at": crate::util::iso_now(), "bots_queried": q, "bots_errored": e, "tracks": rows })
}
