//! /api/users/*, /api/friends/*, /api/messages/*, /api/notifications*,
//! /api/appearance/presets, /api/community/counts, /api/admin/overview.

use crate::app::{account_guild_id, account_id_for_auth};
use crate::auth::Auth;
use crate::db::J;
use crate::http::{R, Reply, Req};
use crate::profiles::with_derived_accent;
use crate::social;
use crate::telemetry::Event;
use serde_json::{Value, json};

pub fn favorite_bot_options(req: &Req) -> Value {
    let s = &req.app.settings;
    json!(s.bots.iter().chain(std::iter::once(&s.aria)).map(|b| json!({ "key": b.key, "display_name": b.display_name, "kind": b.kind })).collect::<Vec<_>>())
}

async fn authed(req: &Req) -> R<Auth> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    Ok(a)
}

async fn actor(req: &Req) -> R<(Auth, i64)> {
    let a = authed(req).await?;
    let id = account_id_for_auth(&req.app, &a).await.map_err(|e| Reply::err(403, e))?;
    Ok((a, id))
}

fn bot_keys(req: &Req) -> Vec<&'static str> {
    let s = &req.app.settings;
    s.bots.iter().map(|b| b.key).chain(std::iter::once("aria")).collect()
}

pub async fn me(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    let app = &req.app;
    let Some(g) = account_guild_id(&a) else {
        let u = if a.username.is_empty() { app.settings.admin_username.clone() } else { a.username.clone() };
        return Ok(Reply::ok(json!({
            "editable": false,
            "profile": { "username": u, "display_name": u, "guild_id": null, "public_profile": false, "has_password": false, "panel_preferences": crate::profiles::default_panel_preferences(),
                         "bio": "Built-in admin sessions can browse the user directory; registered accounts own public profiles." },
            "favorite_bot_options": favorite_bot_options(&req),
        }))
        .by(&a));
    };
    let mut p = crate::accounts::get_account_profile(app, &a.username, &g).await.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    let id = p.i("id").unwrap_or(0);
    if let Value::Object(s) = social::get_account_social_snapshot(app, id, id).await {
        for (k, v) in s {
            p[&k] = v;
        }
    }
    let sums = crate::dashboard::music_activity_for_guilds(app, std::slice::from_ref(&g)).await;
    p["activity"] = sums.get(&g).cloned().unwrap_or_else(crate::dashboard::empty_activity);
    Ok(Reply::ok(json!({ "editable": true, "profile": with_derived_accent(p, "theme_accent", "accent_secondary"), "favorite_bot_options": favorite_bot_options(&req) })).by(&a))
}

pub async fn me_update(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    let app = &req.app;
    let g = account_guild_id(&a).filter(|_| !a.username.is_empty()).ok_or_else(|| Reply::err(403, "Guild account access required to edit a public profile"))?;
    let updates = crate::profiles::clean_profile_updates(&req.json, &bot_keys(&req)).map_err(|e| Reply::err(400, e))?;
    let mut p = crate::accounts::update_account_profile(app, &a.username, &g, &updates).await.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    app.telemetry.record(Event::new("customization", "profile_update").user(Some(&a.username)).guild(Some(&g)).meta(Value::Object(updates)));
    let sums = crate::dashboard::music_activity_for_guilds(app, std::slice::from_ref(&g)).await;
    p["activity"] = sums.get(&g).cloned().unwrap_or_else(crate::dashboard::empty_activity);
    Ok(Reply::ok(json!({ "ok": true, "profile": with_derived_accent(p, "theme_accent", "accent_secondary") })).by(&a))
}

pub async fn preferences(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    let defaults = crate::profiles::default_panel_preferences();
    let Some(g) = account_guild_id(&a).filter(|_| !a.username.is_empty()) else {
        return Ok(Reply::ok(json!({ "editable": false, "preferences": with_derived_accent(defaults, "accent_color", "accent_secondary") })).by(&a));
    };
    let p = crate::accounts::get_account_profile(&req.app, &a.username, &g).await.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    let prefs = crate::profiles::clean_panel_preferences(p.get("panel_preferences").filter(|v| v.is_object()), &defaults);
    Ok(Reply::ok(json!({ "editable": true, "preferences": with_derived_accent(prefs, "accent_color", "accent_secondary") })).by(&a))
}

pub async fn preferences_update(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    let app = &req.app;
    let g = account_guild_id(&a).filter(|_| !a.username.is_empty()).ok_or_else(|| Reply::err(403, "Guild account access required to save panel preferences"))?;
    let cur = crate::accounts::get_account_profile(app, &a.username, &g).await.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    let defaults = crate::profiles::default_panel_preferences();
    let base = crate::profiles::clean_panel_preferences(cur.get("panel_preferences").filter(|v| v.is_object()), &defaults);
    let mut incoming = req.json.as_object().cloned().unwrap_or_default();
    incoming.remove("admin_mode_enabled");
    let prefs = crate::profiles::try_clean_panel_preferences(Some(&Value::Object(incoming)), &base).map_err(|e| Reply::err(400, e))?;
    let p = crate::accounts::update_panel_preferences(app, &a.username, &g, &prefs).await.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    let diff = crate::audit::diff_details(&base, &prefs);
    if !diff.is_empty() {
        app.telemetry.record(Event::new("customization", "panel_preferences_update").user(Some(&a.username)).guild(Some(&g)).text(diff));
    }
    let saved = p.get("panel_preferences").filter(|v| v.is_object()).cloned().unwrap_or(prefs);
    Ok(Reply::ok(json!({ "ok": true, "preferences": with_derived_accent(saved, "accent_color", "accent_secondary") })).by(&a))
}

pub async fn presets(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    Ok(Reply::ok(json!({ "panel": crate::profiles::panel_looks(), "profile": crate::profiles::profile_looks() })).by(&a))
}

fn clean_q(req: &Req) -> String {
    crate::util::cut(&req.q("q").unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "), 80)
}

pub async fn search(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    let q = clean_q(&req);
    let limit = req.q("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(24).clamp(1, 50);
    let viewer = account_id_for_auth(&req.app, &a).await.unwrap_or(0);
    let users = crate::accounts::search_account_profiles(&req.app, &q, limit, viewer, None, false).await;
    Ok(Reply::ok(json!({ "ok": true, "query": q, "users": users, "limit": limit })).by(&a))
}

pub async fn directory(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    let q = clean_q(&req);
    let limit = req.q("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(24).clamp(1, 50);
    let viewer = account_id_for_auth(&req.app, &a).await.unwrap_or(0);
    let online = matches!(req.q("online").as_deref(), Some("1" | "true"));
    let users = crate::accounts::search_account_profiles(&req.app, &q, limit, viewer, None, online).await;
    Ok(Reply::ok(json!({ "ok": true, "query": q, "users": users, "limit": limit, "online_only": online })).by(&a))
}

/// Target account + social snapshot relative to the viewer (no visibility filter).
pub async fn social_target(req: &Req, target: &str, viewer: i64) -> Option<Value> {
    let mut t = crate::accounts::get_account_by_id(&req.app, target).await?;
    let id = t.i("id")?;
    if let Value::Object(s) = social::get_account_social_snapshot(&req.app, id, viewer).await {
        for (k, v) in s {
            t[&k] = v;
        }
    }
    Some(t)
}

pub async fn profile(req: Req) -> R<Reply> {
    let a = authed(&req).await?;
    let viewer = account_id_for_auth(&req.app, &a).await.unwrap_or(0);
    let id: i64 = req.p("account_id").parse().map_err(|_| Reply::err(404, "Profile not found"))?;
    let p = social::get_public_account_profile(&req.app, id, viewer).await.ok_or_else(|| Reply::err(404, "Profile not found"))?;
    let perms = social::social_permissions(&p);
    let friends: Vec<Value> = if perms.b("can_view_friends") { social::list_friends(&req.app, id).await.into_iter().take(12).collect() } else { vec![] };
    Ok(Reply::ok(json!({ "ok": true, "profile": p, "friends": friends, "social_permissions": perms, "favorite_bot_options": favorite_bot_options(&req) })).by(&a))
}

pub async fn follow(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let following = req.json.get("following").is_some_and(|v| v.as_bool().unwrap_or(!v.is_null()));
    let tid = req.p("account_id");
    let target = match tid.parse::<i64>().ok() {
        Some(id) => match social::get_public_account_profile(&req.app, id, me).await {
            Some(t) => Some(t),
            None => social_target(&req, &tid, me).await,
        },
        None => None,
    }
    .ok_or_else(|| Reply::err(404, "Profile not found"))?;
    if following && !social::social_permissions(&target).b("can_follow") {
        return Err(Reply::err(403, "This profile is not accepting follows."));
    }
    let tnum = target.i("id").unwrap_or(0);
    let mut r = social::set_follow(&req.app, me, tnum, following).await.map_err(|e| Reply::err(400, e))?;
    if following {
        social::create_notification(&req.app, tnum, "follow", &format!("{} started following you", a.username), None, Some(&format!("/users/{me}"))).await;
    }
    r["ok"] = json!(true);
    Ok(Reply::ok(r).by(&a))
}

pub async fn friend_request(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let target = social_target(&req, &req.p("account_id"), me).await.ok_or_else(|| Reply::err(404, "Profile not found"))?;
    if !social::social_permissions(&target).b("can_friend") {
        return Err(Reply::err(403, "This profile is not accepting friend requests."));
    }
    let tnum = target.i("id").unwrap_or(0);
    let mut r = social::send_friend_request(&req.app, me, tnum).await.map_err(|e| Reply::err(400, e))?;
    match r.s("status").as_deref() {
        Some("pending_out") => social::create_notification(&req.app, tnum, "friend_request", &format!("{} sent you a friend request", a.username), None, Some("/friends")).await,
        Some("friends") => social::create_notification(&req.app, tnum, "friend_accept", &format!("You and {} are now friends", a.username), None, Some("/friends")).await,
        _ => {}
    }
    r["ok"] = json!(true);
    Ok(Reply::ok(r).by(&a))
}

pub async fn friend_requests(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let (i, o) = tokio::join!(social::list_friend_requests(&req.app, me, false), social::list_friend_requests(&req.app, me, true));
    Ok(Reply::ok(json!({ "ok": true, "incoming": i, "outgoing": o })).by(&a))
}

pub async fn friend_respond(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let rid: i64 = req.p("request_id").parse().map_err(|_| Reply::err(400, "Invalid id"))?;
    let r = social::respond_friend_request(&req.app, me, rid, &req.js("action").unwrap_or_default()).await.map_err(|e| Reply::err(400, e))?.ok_or_else(|| Reply::err(404, "Friend request not found"))?;
    if r.s("status").as_deref() == Some("accepted") {
        if let Some(rq) = r.i("requester_account_id").filter(|r| *r != me) {
            social::create_notification(&req.app, rq, "friend_accept", &format!("{} accepted your friend request", a.username), None, Some("/friends")).await;
        }
    }
    Ok(Reply::ok(json!({ "ok": true, "request": r })).by(&a))
}

pub async fn my_friends(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    Ok(Reply::ok(json!({ "ok": true, "friends": social::list_friends(&req.app, me).await })).by(&a))
}

pub async fn threads(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    Ok(Reply::ok(json!({ "ok": true, "threads": social::list_threads(&req.app, me).await })).by(&a))
}

async fn message_target(req: &Req, me: i64) -> R<i64> {
    let t = social_target(req, &req.p("account_id"), me).await;
    match t {
        Some(t) if social::social_permissions(&t).b("can_message") => Ok(t.i("id").unwrap_or(0)),
        _ => Err(Reply::err(403, "This profile is not accepting direct messages.")),
    }
}

pub async fn messages(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let t = message_target(&req, me).await?;
    let limit = req.q("limit").and_then(|l| l.parse().ok()).unwrap_or(80);
    let m = social::list_messages(&req.app, me, t, limit).await.map_err(|e| Reply::err(400, e))?;
    Ok(Reply::ok(json!({ "ok": true, "messages": m })).by(&a))
}

pub async fn send_message(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let t = message_target(&req, me).await?;
    let body = req.js("body").unwrap_or_default();
    let m = social::send_message(&req.app, me, t, &body).await.map_err(|e| Reply::err(400, e))?;
    social::create_notification(&req.app, t, "message", &format!("New message from {}", a.username), Some(&crate::util::cut(&body, 200)), Some(&format!("/messages/{me}"))).await;
    Ok(Reply::ok(json!({ "ok": true, "message": m })).by(&a))
}

pub async fn notifications(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let limit = req.q("limit").and_then(|l| l.parse().ok()).unwrap_or(30);
    Ok(Reply::ok(json!({ "ok": true, "notifications": social::list_notifications(&req.app, me, limit).await })).by(&a))
}

pub async fn unread_count(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    Ok(Reply::ok(json!({ "ok": true, "unread_count": social::unread_notification_count(&req.app, me).await })).by(&a))
}

pub async fn notification_read(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    if let Ok(id) = req.p("notification_id").parse::<i64>() {
        social::mark_notification_read(&req.app, id, me).await;
    }
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

pub async fn notifications_read_all(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    social::mark_all_notifications_read(&req.app, me).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

pub async fn community_counts(req: Req) -> R<Reply> {
    let (a, me) = actor(&req).await?;
    let mut c = social::community_counts(&req.app, me).await;
    c["ok"] = json!(true);
    Ok(Reply::ok(c).by(&a))
}

pub async fn admin_overview(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    let admin = a.is_admin();
    if !(admin || a.moderator) {
        return Err(Reply::err(403, "Admin or moderator access required"));
    }
    req.read_limit(&a)?;
    let app = &req.app;
    let mut out = json!({ "ok": true, "generated_at": crate::util::iso_now() });
    let d = crate::dashboard::get_dashboard_data(app).await;
    let music: Vec<&Value> = d["bots"].as_array().map(|b| b.iter().filter(|b| b.s("kind").is_none_or(|k| k == "music")).collect()).unwrap_or_default();
    out["bots_total"] = json!(music.len());
    out["bots_online"] = json!(music.iter().filter(|b| b.s("status").unwrap_or_default().to_lowercase() != "offline" && b.i("heartbeat_age_seconds").is_none_or(|x| x <= 120)).count());
    out["alert_rules_enabled"] = json!(crate::alerts::list_enabled(app).await.len());
    out["audit_entries_recent"] = json!(crate::audit::count_recent(app, 24).await);
    if admin {
        if let Some(n) = crate::gallery::count_open_reports(app).await {
            out["open_reports"] = json!(n);
        }
    }
    Ok(Reply::ok(out).by(&a))
}
