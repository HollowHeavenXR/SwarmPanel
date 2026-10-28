//! Panel -> bot orders (port of control.lua): writes the same
//! <bot>_swarm_overrides / <bot>_swarm_direct_orders rows the Rust music bots
//! poll, plus a few direct DB edits (loop/filter/home/queue).

use crate::app::AppRef;
use crate::config::Bot;
use crate::db::{J, P};
use crate::telemetry::Event;
use serde_json::{Value, json};
use std::time::Instant;

pub const VALID_ACTIONS: &[&str] = &["PAUSE", "RESUME", "SKIP", "STOP", "RESTART", "CLEAR", "RESET_QUEUE", "LOOP", "FILTER", "SHUFFLE", "SMART_RECOMMEND", "PLAY", "RECOVER", "LEAVE", "SET_HOME", "SEEK"];
const LOOP_MODES: &[&str] = &["off", "song", "queue"];
pub const FILTER_MODES: &[&str] = &["none", "nightcore", "vaporwave", "bassboost", "8d", "karaoke", "tremolo", "vibrato", "lowpass", "lofi", "electronic", "party", "radio", "cinema"];

struct Ctx<'a> {
    app: &'a AppRef,
    schema: &'a str,
    k: &'a str,
}

impl Ctx<'_> {
    async fn x(&self, sql: &str, params: &[P]) -> Result<u64, String> {
        self.app.db.exec(self.schema, &sql.replace("{p}", self.k), params).await
    }
    async fn quiet(&self, sql: &str, params: &[P]) {
        let _ = self.x(sql, params).await;
    }
    async fn one(&self, sql: &str, params: &[P]) -> Option<Value> {
        self.app.db.opt(self.schema, &sql.replace("{p}", self.k), params).await
    }
    async fn ensure_overrides(&self) {
        let _ = self
            .app
            .db
            .batch(
                self.schema,
                &"CREATE TABLE IF NOT EXISTS {p}_swarm_overrides (guild_id BIGINT, bot_name VARCHAR(50), command VARCHAR(20), attempts INT NOT NULL DEFAULT 0, last_error TEXT NULL, PRIMARY KEY(guild_id, bot_name));
                  ALTER TABLE {p}_swarm_overrides ADD COLUMN IF NOT EXISTS attempts INT NOT NULL DEFAULT 0;
                  ALTER TABLE {p}_swarm_overrides ADD COLUMN IF NOT EXISTS last_error TEXT NULL;
                  ALTER TABLE {p}_swarm_overrides ADD COLUMN IF NOT EXISTS issued_by TEXT;"
                    .replace("{p}", self.k),
            )
            .await;
    }
    async fn ensure_orders(&self) {
        let _ = self
            .app
            .db
            .batch(
                self.schema,
                &"CREATE TABLE IF NOT EXISTS {p}_swarm_direct_orders (id SERIAL PRIMARY KEY, bot_name VARCHAR(50), guild_id BIGINT, vc_id BIGINT, text_channel_id BIGINT, command VARCHAR(50), data TEXT, attempts INT NOT NULL DEFAULT 0, last_error TEXT NULL, created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP);
                  ALTER TABLE {p}_swarm_direct_orders ADD COLUMN IF NOT EXISTS attempts INT NOT NULL DEFAULT 0;
                  ALTER TABLE {p}_swarm_direct_orders ADD COLUMN IF NOT EXISTS last_error TEXT NULL;
                  ALTER TABLE {p}_swarm_direct_orders ADD COLUMN IF NOT EXISTS issued_by TEXT;"
                    .replace("{p}", self.k),
            )
            .await;
    }
    async fn set_override(&self, gid: &str, cmd: &str, actor: &str) -> Result<(), String> {
        self.ensure_overrides().await;
        self.x(
            "INSERT INTO {p}_swarm_overrides (guild_id, bot_name, command, attempts, issued_by) VALUES ($1, $2, $3, 0, $4) ON CONFLICT (guild_id, bot_name) DO UPDATE SET command = EXCLUDED.command, issued_by = EXCLUDED.issued_by",
            &[gid.into(), self.k.into(), cmd.into(), actor.into()],
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("failed to write swarm override: {e}"))
    }
    async fn clear_pending(&self, gid: &str) {
        self.quiet("DELETE FROM {p}_swarm_overrides WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), self.k.into()]).await;
        self.quiet("DELETE FROM {p}_swarm_direct_orders WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), self.k.into()]).await;
    }
    async fn order(&self, gid: &str, vc: &str, text: &str, cmd: &str, data: &str, actor: &str) -> Result<(), String> {
        self.ensure_orders().await;
        self.x(
            "INSERT INTO {p}_swarm_direct_orders (bot_name, guild_id, vc_id, text_channel_id, command, data, issued_by) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[self.k.into(), gid.into(), vc.into(), text.into(), cmd.into(), data.into(), actor.into()],
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("failed to write swarm direct order: {e}"))
    }
    /// Shuffle everything after the first queued track (and mirror to the backup).
    async fn shuffle(&self, gid: &str) -> usize {
        self.quiet("CREATE TABLE IF NOT EXISTS {p}_queue (id SERIAL PRIMARY KEY, guild_id BIGINT, bot_name VARCHAR(50), video_url TEXT, title TEXT, requester_id BIGINT DEFAULT NULL)", &[]).await;
        let mut rows = self.app.db.all(self.schema, &format!("SELECT * FROM {}_queue WHERE guild_id = $1 AND bot_name = $2 ORDER BY id ASC", self.k), &[gid.into(), self.k.into()]).await;
        if rows.len() <= 1 {
            return rows.len();
        }
        let first = rows.remove(0);
        {
            use rand::seq::SliceRandom;
            rows.shuffle(&mut rand::rng());
        }
        rows.insert(0, first);
        let mut stmts: Vec<(String, Vec<P>)> = vec![(format!("DELETE FROM {}_queue WHERE guild_id = $1 AND bot_name = $2", self.k), vec![gid.into(), self.k.into()])];
        for r in &rows {
            stmts.push((
                format!("INSERT INTO {}_queue (guild_id, bot_name, video_url, title, requester_id, track_uid) VALUES ($1, $2, $3, $4, $5, $6)", self.k),
                vec![r.s("guild_id").into(), r.s("bot_name").into(), r.s("video_url").into(), r.s("title").into(), r.s("requester_id").into(), r.s("track_uid").into()],
            ));
        }
        let refs: Vec<(&str, Vec<P>)> = stmts.iter().map(|(s, p)| (s.as_str(), p.clone())).collect();
        let _ = self.app.db.transaction(self.schema, &refs).await;
        self.quiet("CREATE TABLE IF NOT EXISTS {p}_queue_backup (id SERIAL PRIMARY KEY, guild_id BIGINT, bot_name VARCHAR(50), video_url TEXT, title TEXT, requester_id BIGINT DEFAULT NULL)", &[]).await;
        let mut bstmts: Vec<(String, Vec<P>)> = vec![(format!("DELETE FROM {}_queue_backup WHERE guild_id = $1 AND bot_name = $2", self.k), vec![gid.into(), self.k.into()])];
        for r in &rows {
            bstmts.push((
                format!("INSERT INTO {}_queue_backup (guild_id, bot_name, video_url, title, requester_id) VALUES ($1, $2, $3, $4, $5)", self.k),
                vec![r.s("guild_id").into(), r.s("bot_name").into(), r.s("video_url").into(), r.s("title").into(), r.s("requester_id").into()],
            ));
        }
        let refs: Vec<(&str, Vec<P>)> = bstmts.iter().map(|(s, p)| (s.as_str(), p.clone())).collect();
        let _ = self.app.db.transaction(self.schema, &refs).await;
        rows.len()
    }
    async fn prime_defaults(&self, gid: &str) -> usize {
        self.quiet("CREATE TABLE IF NOT EXISTS {p}_guild_settings (guild_id BIGINT PRIMARY KEY)", &[]).await;
        self.quiet("INSERT INTO {p}_guild_settings (guild_id, loop_mode) VALUES ($1, 'queue') ON CONFLICT (guild_id) DO UPDATE SET loop_mode = EXCLUDED.loop_mode", &[gid.into()]).await;
        self.shuffle(gid).await
    }
}

async fn relay(app: &AppRef, bot: &Bot, gid: &str, action: &str, actor: &str) -> Result<(), String> {
    let thread = app.db.opt(&bot.db_schema, &format!("SELECT thread_id FROM {}_command_thread WHERE guild_id = $1", bot.key), &[gid.into()]).await.and_then(|r| r.s("thread_id"));
    let Some(thread) = thread else {
        return Err(format!("{}'s GWS Commands channel isn't set up in this server yet -- try again shortly.", bot.display_name));
    };
    let token = app.settings.token(bot.key);
    if token.is_empty() {
        return Err(format!("No Discord token configured for {}.", bot.display_name));
    }
    let t0 = Instant::now();
    let body = json!({ "embeds": [{ "title": format!("▶️ {action}"), "description": format!("**Command:** `{action}`\n**Guild:** `{gid}`\n**Issued by:** {actor}"), "color": 0x5865F2 }] });
    let r = crate::discord::request(app, &token, reqwest::Method::POST, &format!("/channels/{thread}/messages"), Some(&body)).await;
    app.telemetry.record(Event::new("discord_api", "relay_command_thread").guild(Some(gid)).user(Some(actor)).num(Some(t0.elapsed().as_secs_f64() * 1000.0)).meta(json!({ "bot_key": bot.key, "action": action, "success": r.is_ok() })));
    r.map(|_| ()).map_err(|e| format!("Could not post to {}'s GWS Commands channel: {e}", bot.display_name))
}

fn chan_or_zero(v: Option<String>) -> String {
    v.filter(|s| !s.is_empty()).unwrap_or_else(|| "0".into())
}

fn ps(p: &Value, k: &str) -> Option<String> {
    p.s(k).filter(|s| !s.is_empty())
}

pub async fn control_bot(app: &AppRef, bot: &Bot, gid: &str, action: &str, payload: &Value, actor: &str) -> Result<Value, String> {
    let t0 = Instant::now();
    let action = action.to_uppercase();
    let c = Ctx { app, schema: &bot.db_schema, k: bot.key };
    let name = bot.display_name;
    let mut r = json!({ "action": action, "command": action });
    let res: Result<(), String> = async {
        match action.as_str() {
            "PAUSE" | "RESUME" | "SKIP" | "STOP" => {
                c.clear_pending(gid).await;
                c.set_override(gid, &action, actor).await?;
                c.quiet("ALTER TABLE {p}_playback_state ADD COLUMN IF NOT EXISTS is_paused BOOLEAN DEFAULT FALSE", &[]).await;
                let sql = match action.as_str() {
                    "PAUSE" => "UPDATE {p}_playback_state SET is_paused = TRUE, is_playing = FALSE WHERE guild_id = $1 AND bot_name = $2",
                    "RESUME" => "UPDATE {p}_playback_state SET is_paused = FALSE, is_playing = TRUE WHERE guild_id = $1 AND bot_name = $2",
                    _ => "UPDATE {p}_playback_state SET is_paused = FALSE WHERE guild_id = $1 AND bot_name = $2",
                };
                c.x(sql, &[gid.into(), bot.key.into()]).await?;
                r["message"] = json!(format!("{name} will {} in guild {gid}.", action.to_lowercase()));
            }
            "RESTART" => {
                c.clear_pending("0").await;
                c.set_override("0", "RESTART", actor).await?;
                c.quiet("UPDATE {p}_playback_state SET is_playing = FALSE, is_paused = FALSE WHERE bot_name = $1", &[bot.key.into()]).await;
                r["message"] = json!(format!("Restart signal queued for {name}."));
            }
            "CLEAR" => {
                relay(app, bot, gid, &action, actor).await?;
                c.clear_pending(gid).await;
                c.set_override(gid, "STOP", actor).await?;
                c.x("DELETE FROM {p}_queue WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await?;
                c.quiet("DELETE FROM {p}_queue_backup WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await;
                c.quiet("UPDATE {p}_playback_state SET title = NULL, video_url = NULL, position_seconds = 0, is_playing = FALSE, is_paused = FALSE WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await;
                r["message"] = json!(format!("Cleared the queue and current playback for guild {gid} on {name}."));
            }
            "RESET_QUEUE" => {
                relay(app, bot, gid, &action, actor).await?;
                let pl = c.one("SELECT playlist_url, requester_id, channel_id FROM {p}_active_playlists WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await;
                c.x("DELETE FROM {p}_queue WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await?;
                c.x("DELETE FROM {p}_queue_backup WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await.map_err(|e| format!("failed to clear backup queue: {e}"))?;
                c.quiet("UPDATE {p}_playback_state SET title = NULL, video_url = NULL, position_seconds = 0, is_playing = FALSE, is_paused = FALSE WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await;
                match pl.as_ref().and_then(|p| ps(p, "playlist_url")) {
                    Some(url) => {
                        let home = c.one("SELECT home_vc_id FROM {p}_bot_home_channels WHERE guild_id = $1 AND bot_name = $2", &[gid.into(), bot.key.into()]).await.and_then(|h| h.s("home_vc_id"));
                        let chan = pl.as_ref().and_then(|p| p.s("channel_id"));
                        c.order(gid, &chan_or_zero(home.or(chan.clone())), &chan_or_zero(chan), "PLAY", &url, actor).await?;
                        r["message"] = json!(format!("Cleared the queue, backup queue, and current playback for guild {gid} on {name}, and queued a fresh resync from its saved playlist."));
                    }
                    None => r["message"] = json!(format!("Cleared the queue, backup queue, and current playback for guild {gid} on {name} -- no saved playlist to resync from.")),
                }
            }
            "LOOP" => {
                let mode = ps(payload, "loop_mode").or_else(|| payload.as_str().map(str::to_string)).unwrap_or_default().to_lowercase();
                if !LOOP_MODES.contains(&mode.as_str()) {
                    return Err(format!("Invalid loop mode: {mode}"));
                }
                c.quiet("CREATE TABLE IF NOT EXISTS {p}_guild_settings (guild_id BIGINT PRIMARY KEY)", &[]).await;
                c.x("INSERT INTO {p}_guild_settings (guild_id, loop_mode) VALUES ($1, $2) ON CONFLICT (guild_id) DO UPDATE SET loop_mode = EXCLUDED.loop_mode", &[gid.into(), mode.clone().into()]).await?;
                r["message"] = json!(format!("Loop mode set to {mode} for guild {gid} on {name}."));
                r["loop_mode"] = json!(mode);
            }
            "FILTER" => {
                let mode: String = ps(payload, "filter_mode").or_else(|| payload.as_str().map(str::to_string)).unwrap_or_default().to_lowercase().split_whitespace().collect();
                if !FILTER_MODES.contains(&mode.as_str()) {
                    return Err(format!("Invalid filter mode: {mode}"));
                }
                c.quiet("CREATE TABLE IF NOT EXISTS {p}_guild_settings (guild_id BIGINT PRIMARY KEY)", &[]).await;
                c.x("INSERT INTO {p}_guild_settings (guild_id, filter_mode) VALUES ($1, $2) ON CONFLICT (guild_id) DO UPDATE SET filter_mode = EXCLUDED.filter_mode", &[gid.into(), mode.clone().into()]).await?;
                c.set_override(gid, "UPDATE_FILTER", actor).await?;
                r["message"] = json!(format!("Filter mode set to {mode} for guild {gid} on {name}."));
                r["filter_mode"] = json!(mode);
            }
            "SHUFFLE" => {
                relay(app, bot, gid, &action, actor).await?;
                let n = c.shuffle(gid).await;
                r["queue_count"] = json!(n);
                r["message"] = json!(format!("Shuffled {n} queued tracks for guild {gid} on {name}."));
            }
            "SMART_RECOMMEND" => {
                if !payload.is_object() {
                    return Err("SMART_RECOMMEND payload must be an object with voice_channel_id".into());
                }
                c.clear_pending(gid).await;
                let vc = chan_or_zero(ps(payload, "voice_channel_id"));
                let text = chan_or_zero(ps(payload, "text_channel_id"));
                let requester = ps(payload, "requester_id");
                let shuffled = c.prime_defaults(gid).await;
                let mut reason = "server_favorite";
                let mut seed = None;
                if let Some(u) = &requester {
                    seed = c.one("SELECT title, video_url, score FROM {p}_user_track_affinity WHERE guild_id = $1 AND user_id = $2 AND dislike_count <= like_count ORDER BY score DESC, last_requested DESC LIMIT 1", &[gid.into(), u.into()]).await;
                    if seed.is_some() {
                        reason = "personal_taste";
                    }
                }
                if seed.is_none() {
                    seed = c
                        .one("SELECT title, video_url, ((finish_count * 3) + (like_count * 5) + play_count - (skip_count * 2) - (dislike_count * 5)) AS smart_score FROM {p}_track_intelligence WHERE guild_id = $1 AND dislike_count <= like_count ORDER BY smart_score DESC, updated_at DESC LIMIT 1", &[gid.into()])
                        .await;
                }
                let seed = seed.ok_or("No smart recommendation seed exists for this bot and guild yet")?;
                let title = seed.s("title").or_else(|| seed.s("video_url")).unwrap_or_default();
                let clean: String = title.split_whitespace().filter(|w| !w.starts_with("http://") && !w.starts_with("https://")).collect::<Vec<_>>().join(" ");
                let query = format!("ytmsearch:{} radio", crate::util::cut(&clean, 180));
                c.order(gid, &vc, &text, "PLAY", &query, actor).await?;
                c.quiet("CREATE TABLE IF NOT EXISTS {p}_smart_recommendations (id SERIAL PRIMARY KEY, guild_id BIGINT NOT NULL, requester_id BIGINT DEFAULT NULL, seed_title TEXT, seed_url TEXT, query_text TEXT, chosen_url TEXT, chosen_title TEXT, reason VARCHAR(80), accepted BOOLEAN DEFAULT TRUE, created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP)", &[]).await;
                c.x("INSERT INTO {p}_smart_recommendations (guild_id, requester_id, seed_title, seed_url, query_text, reason) VALUES ($1, $2, $3, $4, $5, $6)", &[gid.into(), requester.into(), title.clone().into(), seed.s("video_url").into(), query.clone().into(), reason.into()]).await?;
                r["seed_title"] = json!(title);
                r["query_text"] = json!(query);
                r["reason"] = json!(reason);
                r["loop_mode"] = json!("queue");
                r["shuffled_queue_count"] = json!(shuffled);
                r["message"] = json!(format!("Queued a smart recommendation for {name} in guild {gid} using {}.", crate::util::cut(&title, 120)));
            }
            "PLAY" => {
                c.clear_pending(gid).await;
                if !payload.is_object() {
                    return Err("PLAY payload must be an object with source_url and voice_channel_id".into());
                }
                let src = ps(payload, "source_url").or_else(|| ps(payload, "query")).unwrap_or_default().trim().to_string();
                if src.is_empty() {
                    return Err("Missing source_url for PLAY action".into());
                }
                let vc = chan_or_zero(ps(payload, "voice_channel_id"));
                let text = chan_or_zero(ps(payload, "text_channel_id"));
                let shuffled = c.prime_defaults(gid).await;
                c.order(gid, &vc, &text, "PLAY", &src, actor).await?;
                r["loop_mode"] = json!("queue");
                r["shuffled_queue_count"] = json!(shuffled);
                r["message"] = json!(format!("Queued a direct PLAY order for {name} in guild {gid}."));
            }
            "RECOVER" => {
                let vc = chan_or_zero(ps(payload, "voice_channel_id").or_else(|| ps(payload, "vc_id")));
                c.order(gid, &vc, "0", "RECOVER", "panel", actor).await?;
                r["message"] = json!(format!("Queued a direct RECOVER order for {name} in guild {gid}."));
            }
            "LEAVE" => {
                c.clear_pending(gid).await;
                let force = payload.get("force").is_some_and(|f| f.as_bool().unwrap_or(false));
                c.order(gid, "0", "0", "LEAVE", if force { "force" } else { "" }, actor).await?;
                r["message"] = json!(format!("Queued a direct LEAVE order for {name} in guild {gid}."));
            }
            "SET_HOME" => {
                let vc = ps(payload, "voice_channel_id").ok_or("Missing voice_channel_id for SET_HOME action")?;
                c.quiet("CREATE TABLE IF NOT EXISTS {p}_bot_home_channels (guild_id BIGINT, bot_name VARCHAR(50), home_vc_id BIGINT, PRIMARY KEY (guild_id, bot_name))", &[]).await;
                c.x("INSERT INTO {p}_bot_home_channels (guild_id, bot_name, home_vc_id) VALUES ($1, $2, $3) ON CONFLICT (guild_id, bot_name) DO UPDATE SET home_vc_id = EXCLUDED.home_vc_id", &[gid.into(), bot.key.into(), vc.clone().into()]).await?;
                r["voice_channel_id"] = json!(vc);
                r["message"] = json!(format!("Set home channel for {name} in guild {gid}."));
            }
            "SEEK" => {
                if !payload.is_object() {
                    return Err("SEEK payload must be an object with position_seconds".into());
                }
                let pos = payload.f("position_seconds").unwrap_or(0.0).max(0.0).floor() as i64;
                c.ensure_orders().await;
                c.quiet("DELETE FROM {p}_swarm_direct_orders WHERE guild_id = $1 AND bot_name = $2 AND command = 'SEEK'", &[gid.into(), bot.key.into()]).await;
                c.order(gid, "0", "0", "SEEK", &pos.to_string(), actor).await?;
                r["position_seconds"] = json!(pos);
                r["message"] = json!(format!("Seek order queued for {name} in guild {gid} at position {pos}s."));
            }
            other => return Err(format!("Unsupported action: {other}")),
        }
        Ok(())
    }
    .await;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let ok = res.is_ok();
    app.telemetry.record(
        Event::new("bot_command", &action)
            .guild(Some(gid))
            .user(Some(actor))
            .num(Some(ms))
            .meta(json!({ "bot_key": bot.key, "success": ok, "error": res.as_ref().err(), "message": r.get("message"), "payload": payload })),
    );
    res.map(|_| r)
}
