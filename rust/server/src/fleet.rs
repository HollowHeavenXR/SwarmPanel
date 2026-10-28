//! Payload builders shared by the REST routes, the live socket and the pages
//! (routes.lua's build_dashboard_payload / build_bots_payload /
//! enrich_control_state_with_discord / bot guild access checks).

use crate::app::{AppRef, OWNER_SCOPE, scoped_guild_id};
use crate::auth::Auth;
use crate::db::J;
use crate::http::{R, Reply};
use futures_util::future::join_all;
use serde_json::{Value, json};

/// The fleet snapshot scoped to what this caller may see. `enrich` adds
/// Discord identity + guild/channel names (REST only; the 2s push skips it).
pub async fn dashboard_payload(app: &AppRef, a: &Auth, enrich: bool) -> Value {
    let mut data = crate::dashboard::get_dashboard_data(app).await;
    let scoped = scoped_guild_id(a);
    let mut bots: Vec<Value> = data["bots"].as_array().cloned().unwrap_or_default();
    if let Some(sc) = &scoped {
        bots = bots
            .into_iter()
            .filter(|b| b.s("kind").as_deref() == Some("music") && sc != OWNER_SCOPE && b["known_guild_ids"].as_array().is_some_and(|ids| ids.iter().any(|g| g.as_str() == Some(sc))))
            .map(|mut b| {
                let sessions: Vec<Value> = b["sessions"].as_array().cloned().unwrap_or_default().into_iter().filter(|s| s.s("guild_id").as_deref() == Some(sc.as_str())).collect();
                b["active_playing_count"] = json!(sessions.iter().filter(|s| s.b("is_playing")).count());
                b["queue_depth"] = json!(sessions.iter().map(|s| s.i("queue_count").unwrap_or(0)).sum::<i64>());
                b["backup_queue_depth"] = json!(sessions.iter().map(|s| s.i("backup_queue_count").unwrap_or(0)).sum::<i64>());
                b["sessions"] = json!(sessions);
                b["known_guild_count"] = json!(1);
                b["guild_count"] = json!(1);
                b
            })
            .collect();
    }
    let mut flat = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for b in bots.iter_mut() {
        let key = b.s("key").unwrap_or_default();
        let name = b.s("display_name").unwrap_or_default();
        if b.get("name").is_none_or(Value::is_null) {
            b["name"] = json!(name);
        }
        if b.get("guild_count").is_none_or(Value::is_null) {
            b["guild_count"] = json!(b.i("known_guild_count").unwrap_or(0));
        }
        if let Some(m) = b.as_object_mut() {
            m.remove("known_guild_ids");
        }
        if let Some(sessions) = b["sessions"].as_array_mut() {
            for s in sessions.iter_mut() {
                s["bot_key"] = json!(key);
                s["bot_name"] = json!(name);
                s["bot_display"] = json!(name);
                if seen.insert(format!("{key}\0{}", s.s("guild_id").unwrap_or_else(|| "0".into()))) {
                    flat.push(s.clone());
                }
            }
        }
    }
    // Discord enrichment, per bot in parallel; failures land on bot.discord.
    let enriched = join_all(bots.into_iter().map(|mut b| async move {
        let token = app.settings.token(&b.s("key").unwrap_or_default());
        if !enrich || token.is_empty() {
            b["discord"] = json!({ "token_configured": !token.is_empty() });
            return b;
        }
        let mut d = json!({ "token_configured": true });
        match crate::discord::fetch_identity(app, &token).await {
            Ok(i) => d["identity"] = i,
            Err(e) => d["error"] = json!(e),
        }
        let placements: Vec<(String, Option<String>)> = b["sessions"]
            .as_array()
            .map(|ss| ss.iter().flat_map(|s| [s.s("channel_id"), s.s("home_channel_id")].into_iter().flatten().map(|c| (s.s("guild_id").unwrap_or_default(), Some(c))).collect::<Vec<_>>()).collect())
            .unwrap_or_default();
        if !placements.is_empty() {
            let names = crate::discord::resolve_names(app, &token, &placements).await;
            if let Some(ss) = b["sessions"].as_array_mut() {
                for s in ss.iter_mut() {
                    let g = s.s("guild_id").unwrap_or_default();
                    if let Some((gn, cn)) = s.s("channel_id").and_then(|c| names.get(&(g.clone(), c))) {
                        s["guild_name"] = json!(gn);
                        s["channel_name"] = json!(cn);
                    }
                    if let Some((gn, cn)) = s.s("home_channel_id").and_then(|c| names.get(&(g.clone(), c))) {
                        if s.s("guild_name").is_none() {
                            s["guild_name"] = json!(gn);
                        }
                        s["home_channel_name"] = json!(cn);
                    }
                }
            }
        }
        b["discord"] = d;
        b
    }))
    .await;
    data["bots"] = json!(enriched);
    data["sessions"] = json!(flat);
    data
}

pub async fn bot_has_registered_guild(app: &AppRef, bot_key: &str, gid: &str) -> bool {
    let Some(bot) = app.settings.music_bot(bot_key) else { return false };
    let snap = crate::dashboard::music_bot_snapshot(app, bot).await;
    if snap["known_guild_ids"].as_array().is_some_and(|ids| ids.iter().any(|g| g.as_str() == Some(gid))) {
        return true;
    }
    let token = app.settings.token(bot_key);
    if token.is_empty() {
        return false;
    }
    crate::discord::fetch_guilds(app, &token).await.0.iter().any(|g| g.s("id").as_deref() == Some(gid))
}

pub async fn require_bot_guild_access(app: &AppRef, a: &Auth, bot_key: &str, gid: &str) -> R<()> {
    let scoped = scoped_guild_id(a);
    if let Some(s) = &scoped {
        if s != gid {
            return Err(Reply::err(403, "This account can only control its registered guild"));
        }
        if !bot_has_registered_guild(app, bot_key, s).await {
            return Err(Reply::err(403, "This bot is not registered with your guild"));
        }
    }
    Ok(())
}

/// Flat control state -> {key, display_name, guild_id, db, discord, session}.
pub async fn enrich_control_state(app: &AppRef, flat: Value) -> Value {
    let key = flat.s("bot_key").unwrap_or_default();
    let gid = flat.s("guild_id").unwrap_or_default();
    let mut session = flat.clone();
    let mut state = json!({
        "key": key, "display_name": flat.get("bot_display"), "guild_id": gid,
        "db": { "status": flat.get("status"), "reachable": flat.s("status").as_deref() == Some("online"), "message": flat.get("heartbeat_status") },
    });
    let token = app.settings.token(&key);
    if token.is_empty() {
        state["session"] = session;
        state["discord"] = json!({ "status": "missing", "reachable": false, "message": "Panel token is not configured for Discord inventory access.", "token_configured": false });
        return state;
    }
    let guild = crate::discord::fetch_guild(app, &token, &gid).await;
    let mut placements = vec![(gid.clone(), None)];
    for f in ["channel_id", "home_channel_id"] {
        if let Some(c) = session.s(f) {
            placements.push((gid.clone(), Some(c)));
        }
    }
    let names = crate::discord::resolve_names(app, &token, &placements).await;
    let gname = names.get(&(gid.clone(), String::new())).map(|n| n.0.clone()).or_else(|| guild.s("name")).unwrap_or_else(|| format!("Guild {gid}"));
    session["guild_name"] = json!(gname);
    for (f, out) in [("channel_id", "channel_name"), ("home_channel_id", "home_channel_name")] {
        if let Some(c) = session.s(f) {
            session[out] = json!(names.get(&(gid.clone(), c)).and_then(|n| n.1.clone()));
        }
    }
    let inaccessible = guild.s("name").is_some_and(|n| n.starts_with("Unknown/Inaccessible"));
    state["discord"] = if inaccessible {
        json!({ "status": "error", "reachable": false, "message": format!("Discord guild {gid} is not reachable with this bot's token."), "token_configured": true })
    } else {
        json!({ "status": "online", "reachable": true, "message": format!("Live Discord route is valid in {gname}."), "token_configured": true })
    };
    state["session"] = session;
    state
}

pub async fn bots_payload(app: &AppRef, a: &Auth) -> Value {
    let s = &app.settings;
    let all: Vec<&crate::config::Bot> = s.bots.iter().chain(std::iter::once(&s.aria)).collect();
    let bots: Vec<Value> = all.iter().map(|b| json!({ "key": b.key, "display_name": b.display_name, "name": b.display_name, "kind": b.kind, "schema": b.db_schema, "token_configured": !s.token(b.key).is_empty() })).collect();
    let scoped = scoped_guild_id(a).filter(|g| g != OWNER_SCOPE);
    let mut visible = std::collections::HashSet::new();
    if let Some(g) = &scoped {
        let d = crate::dashboard::get_dashboard_data(app).await;
        for b in d["bots"].as_array().cloned().unwrap_or_default() {
            if b["known_guild_ids"].as_array().is_some_and(|ids| ids.iter().any(|x| x.as_str() == Some(g.as_str()))) {
                visible.insert(b.s("key").unwrap_or_default());
            }
        }
    }
    let invite = join_all(all.iter().map(|b| {
        let scoped = scoped.clone();
        let vis = visible.contains(b.key);
        async move {
            let token = s.token(b.key);
            let mut client_id = crate::discord::client_id_from_token(&token);
            let (mut identity, mut ierr) = (json!({}), None);
            if !token.is_empty() {
                match crate::discord::fetch_identity(app, &token).await {
                    Ok(i) => {
                        if let Some(id) = i.s("id") {
                            client_id = Some(id);
                        }
                        identity = i;
                    }
                    Err(e) => ierr = Some(e),
                }
            }
            let perms = if b.kind == "orchestrator" { crate::discord::ARIA_PERMS } else { crate::discord::MUSIC_PERMS };
            let pv = crate::discord::permission_value(perms);
            json!({
                "key": b.key, "display_name": b.display_name, "name": b.display_name, "kind": b.kind, "schema": b.db_schema, "token_configured": !token.is_empty(),
                "client_id": client_id, "invite_url": client_id.as_deref().map(|c| crate::discord::invite_url(c, pv, scoped.as_deref())),
                "permission_integer": pv.to_string(), "permissions": perms, "capability_summary": crate::discord::capability_summary(b.kind),
                "accent": b.accent, "connected_to_session_guild": scoped.is_some() && vis,
                "icon_url": identity.get("avatar_url"), "identity_name": identity.s("global_name").or_else(|| identity.s("username")), "identity_error": ierr,
            })
        }
    }))
    .await;
    json!({ "bots": bots, "invite_bots": invite, "scoped_guild_id": scoped })
}

pub fn exports_payload(app: &AppRef) -> Value {
    let dir = std::path::PathBuf::from(&app.settings.scheduled_exports_dir);
    let mut snaps = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let date = e.file_name().to_string_lossy().to_string();
            let is_date = date.len() == 10 && date.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
            if !is_date {
                continue;
            }
            let mut files = Vec::new();
            if let Ok(fd) = std::fs::read_dir(e.path()) {
                for f in fd.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    if name.ends_with(".csv") && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
                        files.push(json!({ "name": name, "size_bytes": f.metadata().map(|m| m.len()).unwrap_or(0) }));
                    }
                }
            }
            if !files.is_empty() {
                snaps.push(json!({ "date": date, "files": files }));
            }
        }
    }
    snaps.sort_by(|a, b| b.s("date").cmp(&a.s("date")));
    json!({ "ok": true, "enabled": app.settings.scheduled_exports_enabled, "snapshots": snaps })
}
