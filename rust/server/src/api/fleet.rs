//! Fleet API: dashboard, bots, inventory, control, queues, guild views.

use crate::app::{OWNER_SCOPE, scoped_guild_id};
use crate::audit;
use crate::db::J;
use crate::fleet;
use crate::http::{R, Reply, Req};
use futures_util::future::join_all;
use serde_json::{Value, json};

pub async fn dashboard(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    Ok(Reply::ok(fleet::dashboard_payload(&req.app, &a, true).await).by(&a))
}

pub async fn swarm_leaderboard(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    if !a.is_admin() {
        return Err(Reply::err(403, "Swarm-wide leaderboard is admin-only (it queries every bot's database)."));
    }
    Ok(Reply::ok(crate::dashboard::swarm_leaderboard(&req.app, req.q("days"), req.q("limit")).await).by(&a))
}

pub async fn bots(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    Ok(Reply::ok(fleet::bots_payload(&req.app, &a).await).by(&a))
}

pub async fn inventory(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let app = &req.app;
    let key = req.p("bot_key");
    let bot = app.settings.bot(&key).ok_or_else(|| Reply::err(404, "Unknown bot key"))?;
    let scoped = scoped_guild_id(&a);
    if let Some(s) = &scoped {
        fleet::require_bot_guild_access(app, &a, &key, s).await?;
    }
    let token = app.settings.token(bot.key);
    if token.is_empty() {
        return Err(Reply::err(400, format!("Missing token env for {}", bot.display_name)));
    }
    let hints: Vec<String> = match &scoped {
        Some(s) => vec![s.clone()],
        None if bot.is_music() => crate::dashboard::music_bot_snapshot(app, bot).await["known_guild_ids"].as_array().map(|a| a.iter().filter_map(|g| g.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        None => vec![],
    };
    let mut inv = crate::discord::fetch_inventory(app, &token, req.q("include_channels").as_deref() != Some("false"), &hints).await;
    if let Some(s) = &scoped {
        let gs: Vec<Value> = inv["guilds"].as_array().cloned().unwrap_or_default().into_iter().filter(|g| g.s("id").as_deref() == Some(s.as_str())).collect();
        inv["guilds"] = json!(gs);
    }
    inv["bot"] = json!({ "key": bot.key, "display_name": bot.display_name, "kind": bot.kind });
    Ok(Reply::ok(inv).by(&a))
}

pub async fn control_state(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let gid = req.q("guild_id").ok_or_else(|| Reply::err(400, "guild_id is required"))?;
    let key = req.p("bot_key");
    fleet::require_bot_guild_access(&req.app, &a, &key, &gid).await?;
    let bot = req.app.settings.music_bot(&key).ok_or_else(|| Reply::err(404, "Unknown music bot key"))?;
    let st = crate::dashboard::get_bot_control_state(&req.app, bot, &gid).await;
    Ok(Reply::ok(fleet::enrich_control_state(&req.app, st).await).by(&a))
}

pub async fn control(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let app = &req.app;
    let action = req.js("action").or_else(|| req.js("command")).unwrap_or_default().to_uppercase();
    if !crate::control::VALID_ACTIONS.contains(&action.as_str()) {
        return Err(Reply::err(400, format!("Unsupported action: {action}")));
    }
    if action == "RESTART" && !a.is_admin() {
        return Err(Reply::err(403, "Restart is admin-only because it affects the whole bot node"));
    }
    let key = req.js("bot_key").unwrap_or_default();
    let bot = app.settings.music_bot(&key).ok_or_else(|| Reply::err(404, "Unknown bot key"))?;
    let gid = if action == "RESTART" { "0".to_string() } else { req.js("guild_id").unwrap_or_default() };
    if gid.is_empty() {
        return Err(Reply::err(400, "guild_id is required"));
    }
    if action != "RESTART" {
        fleet::require_bot_guild_access(app, &a, &key, &gid).await?;
    }
    let payload = req.json.get("payload").cloned().unwrap_or(Value::Null);
    let res = crate::control::control_bot(app, bot, &gid, &action, &payload, &a.username).await;
    audit::log(app, &a.username, "swarm_bot_control", "music_bot", &key, json!({ "action": action, "guild_id": gid, "payload": payload, "ok": res.is_ok(), "error": res.as_ref().err() })).await;
    let mut r = res.map_err(|e| Reply::err(400, e))?;
    r["ok"] = json!(true);
    app.cache.dashboard.remove("snap");
    Ok(Reply::ok(r).by(&a))
}

pub async fn convert_channels(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.app.limiter.check(&format!("api-write:{}", a.username.to_lowercase()), 10, 60)?;
    let gid = req.p("guild_id");
    req.require_guild_scope(&a, &gid)?;
    let dir = req.js("direction").unwrap_or_default().to_lowercase();
    if dir != "stage" && dir != "voice" {
        return Err(Reply::err(400, "direction must be 'stage' or 'voice'"));
    }
    let res = crate::channel_convert::convert_guild_channels(&req.app, &gid, &dir).await;
    audit::log(&req.app, &a.username, "swarm_convert_channels", "guild", &gid, json!({ "direction": dir, "ok": res.is_ok(), "result": res.as_ref().map(|v| v.clone()).unwrap_or_else(|e| json!(e)) })).await;
    let mut r = res.map_err(|e| Reply::err(500, format!("Channel conversion failed: {e}")))?;
    r["ok"] = json!(true);
    Ok(Reply::ok(r).by(&a))
}

pub async fn queues_list(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let gid = req.q("guild_id").ok_or_else(|| Reply::err(400, "guild_id is required"))?;
    req.require_guild_scope(&a, &gid)?;
    let q = crate::queues::list(&req.app, &gid, req.q("bot_key").as_deref()).await.map_err(|e| Reply::err(503, format!("Saved queues unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "queues": q })).by(&a))
}

fn body_guild(req: &Req) -> R<String> {
    req.js("guild_id").filter(|g| !g.is_empty()).ok_or_else(|| Reply::err(400, "guild_id is required"))
}

pub async fn queues_create(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let gid = body_guild(&req)?;
    req.require_guild_scope(&a, &gid)?;
    let bk = req.js("bot_key").unwrap_or_default();
    let name = req.js("name").unwrap_or_default();
    let q = crate::queues::create(&req.app, &gid, &bk, &name, &a.username, req.json.get("items").unwrap_or(&Value::Null)).await.map_err(|e| Reply::err(400, e))?;
    audit::record(&req.app, &a.username, "saved_queue_create", audit::Opts { target_type: Some("saved_queue".into()), target_id: q.s("id"), details: Some(format!("guild={gid} bot={bk} name={name}")), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "queue": q })).by(&a))
}

pub async fn queues_rename(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let gid = body_guild(&req)?;
    req.require_guild_scope(&a, &gid)?;
    let id = req.p("queue_id");
    let name = req.js("name").unwrap_or_default();
    let q = crate::queues::rename(&req.app, &id, &gid, &name).await.map_err(|e| Reply::err(400, e))?;
    audit::record(&req.app, &a.username, "saved_queue_rename", audit::Opts { target_type: Some("saved_queue".into()), target_id: Some(id), details: Some(format!("name={name}")), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "queue": q })).by(&a))
}

pub async fn queues_delete(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let gid = body_guild(&req)?;
    req.require_guild_scope(&a, &gid)?;
    let id = req.p("queue_id");
    crate::queues::delete(&req.app, &id, &gid).await;
    audit::record(&req.app, &a.username, "saved_queue_delete", audit::Opts { target_type: Some("saved_queue".into()), target_id: Some(id), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

pub async fn control_matrix(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let gid = req.p("guild_id");
    req.require_guild_scope(&a, &gid)?;
    let app = &req.app;
    let bots = join_all(app.settings.bots.iter().map(|b| {
        let gid = gid.clone();
        async move {
            let st = crate::dashboard::get_bot_control_state(app, b, &gid).await;
            fleet::enrich_control_state(app, st).await
        }
    }))
    .await;
    Ok(Reply::ok(json!({ "generated_at": crate::util::iso_now(), "guild_id": gid, "bots": bots })).by(&a))
}

pub async fn guild_leaderboard(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let gid = req.p("guild_id");
    req.require_guild_scope(&a, &gid)?;
    let bot = req.q("bot_key").and_then(|k| req.app.settings.music_bot(&k)).ok_or_else(|| Reply::err(400, "This action is only supported for music bots"))?;
    let limit = req.q("limit").and_then(|l| l.parse().ok()).unwrap_or(10);
    Ok(Reply::ok(json!({ "ok": true, "data": crate::dashboard::guild_leaderboard(&req.app, bot, &gid, limit).await })).by(&a))
}

pub async fn music_intelligence(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    let mut gid = req.q("guild_id");
    match scoped_guild_id(&a) {
        Some(s) if s != OWNER_SCOPE => gid = Some(s),
        _ => match &gid {
            Some(g) => req.require_guild_scope(&a, g)?,
            None if !a.is_admin() => return Err(Reply::err(403, "Admin access required")),
            None => {}
        },
    }
    let limit = req.q("limit").and_then(|l| l.parse().ok()).unwrap_or(8);
    let r = crate::dashboard::music_intelligence(&req.app, gid.as_deref(), req.q("bot_key").as_deref(), limit).await.map_err(|e| Reply::err(503, format!("Music intelligence unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "data": r })).by(&a))
}

pub async fn bot_telemetry(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    let bot = req.q("bot_key").and_then(|k| req.app.settings.music_bot(&k)).ok_or_else(|| Reply::err(400, "unknown or unsupported bot_key"))?;
    let limit = req.q("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(100).clamp(1, 1000);
    let table = format!("{}_telemetry_events", bot.key);
    if !req.app.db.table_exists(&bot.db_schema, &table).await {
        return Ok(Reply::ok(json!({ "ok": true, "events": [] })).by(&a));
    }
    let rows = match req.q("category") {
        Some(c) => req.app.db.query(&bot.db_schema, &format!("SELECT event_category, event_name, guild_id::text AS guild_id, numeric_value, text_value, created_at FROM {table} WHERE event_category = $1 ORDER BY created_at DESC LIMIT $2"), &[c.into(), limit.into()]).await,
        None => req.app.db.query(&bot.db_schema, &format!("SELECT event_category, event_name, guild_id::text AS guild_id, numeric_value, text_value, created_at FROM {table} ORDER BY created_at DESC LIMIT $1"), &[limit.into()]).await,
    }
    .map_err(|e| Reply::err(503, format!("Telemetry query failed: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "events": rows })).by(&a))
}

pub async fn alert_rules(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    let r = crate::alerts::list(&req.app).await.map_err(|e| Reply::err(503, format!("Alert rules unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "rules": r })).by(&a))
}

pub async fn alert_rule_create(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let rule = crate::alerts::create(&req.app, &req.json).await.map_err(|e| Reply::err(400, e))?;
    audit::record(&req.app, &a.username, "alert_rule_create", audit::Opts { target_type: Some("alert_rule".into()), target_id: rule.s("id"), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "rule": rule })).by(&a))
}

pub async fn alert_rule_update(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let id = req.p("rule_id");
    let before = crate::alerts::get(&req.app, &id).await;
    let body = req.json.as_object().cloned().unwrap_or_default();
    let rule = crate::alerts::update(&req.app, &id, &body).await.map_err(|e| Reply::err(400, e))?.ok_or_else(|| Reply::err(404, "Alert rule not found"))?;
    let mut keys: Vec<&String> = body.keys().collect();
    keys.sort();
    let details = match &before {
        Some(b) => audit::diff_details(b, &rule),
        None => keys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","),
    };
    audit::record(&req.app, &a.username, "alert_rule_update", audit::Opts { target_type: Some("alert_rule".into()), target_id: Some(id), details: Some(details).filter(|d| !d.is_empty()), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "rule": rule })).by(&a))
}

pub async fn alert_rule_delete(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let id = req.p("rule_id");
    crate::alerts::delete(&req.app, &id).await;
    audit::record(&req.app, &a.username, "alert_rule_delete", audit::Opts { target_type: Some("alert_rule".into()), target_id: Some(id), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}
