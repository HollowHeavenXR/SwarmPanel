//! /ws live feed (port of routes.lua's WebSocket broadcast registry).
//! Protocol: client sends {"type":"auth","token":...} first, then
//! {"type":"watch","key":...,"params":{...}} / {"type":"unwatch","key":...}.
//! Server pushes {"type":"snapshot","key","data","generated_at"} only when a
//! key's data changed, {"type":"snapshot_error",...} on failure, and
//! {"type":"ping"} after 45s of silence. One shared 1s tick; builds are
//! shared by every connection with the same key+scope on that tick.

use crate::app::{AppRef, OWNER_SCOPE, account_guild_id, account_id_for_auth, scoped_guild_id};
use crate::auth::Auth;
use crate::db::J;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const KEYS: &[(&str, u64)] = &[
    ("dashboard", 2), ("notifications", 5), ("friends", 5), ("account", 10), ("community_counts", 10), ("threads", 5), ("thread_messages", 2),
    ("control_state", 2), ("queues", 5), ("invites", 60), ("lumisound_admin", 15), ("stability", 5), ("metrics_snapshot", 5), ("events", 5),
    ("trends", 60), ("audit_log", 15), ("alert_rules", 30), ("exports", 60),
];

fn interval(key: &str) -> Option<u64> {
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, i)| *i)
}

struct Conn {
    auth: Auth,
    tx: mpsc::Sender<String>,
    watches: Mutex<HashMap<String, Value>>,
    digests: Mutex<HashMap<String, u64>>,
    due: Mutex<HashMap<String, Instant>>,
}

static CONNS: LazyLock<Mutex<Vec<Arc<Conn>>>> = LazyLock::new(Default::default);
static LOOP: AtomicU64 = AtomicU64::new(0);

fn p(params: &Value, k: &str) -> Option<String> {
    params.s(k).filter(|s| !s.is_empty())
}

fn dash_scope(a: &Auth) -> String {
    match scoped_guild_id(a) {
        Some(g) => format!("guild:{g}"),
        None if a.is_admin() => "admin".into(),
        None => format!("session:{}", a.username),
    }
}
fn admin_scope(a: &Auth) -> &'static str {
    if a.is_admin() { "admin" } else { "denied" }
}
fn mod_scope(a: &Auth) -> &'static str {
    if a.is_admin() || a.moderator { "admin_or_mod" } else { "denied" }
}

async fn scope_key(app: &AppRef, key: &str, a: &Auth, params: &Value) -> String {
    let acct = || async { format!("account:{}", account_id_for_auth(app, a).await.map(|i| i.to_string()).unwrap_or_else(|_| "none".into())) };
    match key {
        "dashboard" | "invites" => dash_scope(a),
        "notifications" | "friends" | "community_counts" | "threads" => acct().await,
        "account" => format!("acct:{}:{:?}", a.username, account_guild_id(a)),
        "thread_messages" => format!("thread:{}:{:?}", acct().await, p(params, "account_id")),
        "control_state" => format!("cs:{:?}:{:?}:{:?}", scoped_guild_id(a), p(params, "bot_key"), p(params, "guild_id")),
        "queues" => format!("q:{:?}:{:?}:{:?}", scoped_guild_id(a), p(params, "guild_id"), p(params, "bot_key")),
        "lumisound_admin" | "audit_log" | "alert_rules" => mod_scope(a).into(),
        "stability" => "global".into(),
        _ => admin_scope(a).into(),
    }
}

pub async fn build(app: &AppRef, key: &str, a: &Auth, params: &Value) -> Result<Value, String> {
    let actor = || async { account_id_for_auth(app, a).await };
    let need_admin = || if a.is_admin() { Ok(()) } else { Err("Admin access required".to_string()) };
    let need_mod = || if a.is_admin() || a.moderator { Ok(()) } else { Err("Admin or moderator access required".to_string()) };
    Ok(match key {
        "dashboard" => crate::fleet::dashboard_payload(app, a, false).await,
        "notifications" => {
            let id = actor().await?;
            json!({ "unread_count": crate::social::unread_notification_count(app, id).await, "notifications": crate::social::list_notifications(app, id, 30).await })
        }
        "friends" => {
            let id = actor().await?;
            let (f, i, o) = tokio::join!(crate::social::list_friends(app, id), crate::social::list_friend_requests(app, id, false), crate::social::list_friend_requests(app, id, true));
            json!({ "friends": f, "incoming": i, "outgoing": o })
        }
        "account" => {
            let username = if a.username.is_empty() { app.settings.admin_username.clone() } else { a.username.clone() };
            let gid = account_guild_id(a);
            let defaults = crate::profiles::default_panel_preferences();
            let mut summary = json!({ "username": username, "display_name": username, "guild_id": gid });
            let mut prefs = defaults.clone();
            if let Some(g) = &gid {
                if let Some(pr) = crate::accounts::get_account_profile(app, &username, g).await {
                    for f in ["display_name", "avatar_url", "server_name", "server_icon_url", "profile_headline", "favorite_bot", "theme_accent"] {
                        if let Some(v) = pr.get(f).filter(|v| !v.is_null()) {
                            summary[f] = v.clone();
                        }
                    }
                    prefs = crate::profiles::clean_panel_preferences(pr.get("panel_preferences").filter(|v| v.is_object()), &defaults);
                }
            }
            if let Some(m) = prefs.as_object_mut() {
                m.remove("admin_mode_enabled");
            }
            summary["role"] = json!(a.role);
            summary["admin_mode"] = json!(a.is_admin());
            json!({ "profile": summary, "preferences": crate::profiles::with_derived_accent(prefs, "accent_color", "accent_secondary") })
        }
        "community_counts" => crate::social::community_counts(app, actor().await?).await,
        "threads" => json!({ "threads": crate::social::list_threads(app, actor().await?).await }),
        "thread_messages" => {
            let me = actor().await?;
            let target = p(params, "account_id").ok_or("account_id is required")?;
            let ck = format!("threadperm:{me}:{target}");
            let allowed = match app.cache.misc.get(&ck, Duration::from_secs(20)) {
                Some(v) => v.as_bool().unwrap_or(false),
                None => {
                    let mut ok = false;
                    if let Some(mut t) = crate::accounts::get_account_by_id(app, target.as_str()).await {
                        if let Some(tid) = t.i("id") {
                            if let Value::Object(s) = crate::social::get_account_social_snapshot(app, tid, me).await {
                                for (k, v) in s {
                                    t[&k] = v;
                                }
                            }
                            ok = crate::social::social_permissions(&t).b("can_message");
                        }
                    }
                    app.cache.misc.put(&ck, json!(ok));
                    ok
                }
            };
            if !allowed {
                return Err("This profile is not accepting direct messages.".into());
            }
            let tid: i64 = target.parse().map_err(|_| "Invalid id")?;
            json!({ "messages": crate::social::list_messages(app, me, tid, 80).await? })
        }
        "control_state" => {
            let (bk, g) = (p(params, "bot_key"), p(params, "guild_id"));
            let (Some(bk), Some(g)) = (bk, g) else { return Err("bot_key/guild_id required".into()) };
            crate::fleet::require_bot_guild_access(app, a, &bk, &g).await.map_err(|r| match r.payload {
                crate::http::Payload::Json(v) => v.s("detail").unwrap_or_else(|| "forbidden".into()),
                _ => "forbidden".into(),
            })?;
            let bot = app.settings.music_bot(&bk).ok_or("Unknown music bot key")?;
            let st = crate::dashboard::get_bot_control_state(app, bot, &g).await;
            crate::fleet::enrich_control_state(app, st).await
        }
        "queues" => {
            let g = p(params, "guild_id").ok_or("guild_id is required")?;
            if let Some(s) = scoped_guild_id(a) {
                if s != g {
                    return Err("This account can only control its registered guild".into());
                }
            }
            json!({ "queues": crate::queues::list(app, &g, p(params, "bot_key").as_deref()).await? })
        }
        "invites" => crate::fleet::bots_payload(app, a).await,
        "lumisound_admin" => {
            need_mod()?;
            json!({ "data": crate::lumisound::admin_data(app, 50).await? })
        }
        "stability" => crate::metrics::stability(app).await,
        "metrics_snapshot" => {
            need_admin()?;
            crate::metrics::snapshot(app).await
        }
        "events" => {
            need_admin()?;
            json!({ "events": crate::metrics::events(app, 80).await })
        }
        "trends" => {
            need_admin()?;
            let mut out = json!({});
            for m in ["queued_tracks", "active_bots"] {
                out[m] = json!({ "points": crate::metrics::history(app, m, 24).await.unwrap_or_default(), "anomalies": crate::metrics::anomalies(app, m, 24).await.unwrap_or_default() });
            }
            out
        }
        "audit_log" => {
            need_mod()?;
            json!({ "data": crate::audit::list(app, 200, 0, None).await })
        }
        "alert_rules" => {
            need_mod()?;
            json!({ "rules": crate::alerts::list(app).await? })
        }
        "exports" => {
            need_admin()?;
            crate::fleet::exports_payload(app)
        }
        _ => return Err("unknown key".into()),
    })
}

fn digest(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn ensure_loop(app: &AppRef) {
    if LOOP.swap(1, Ordering::SeqCst) == 1 {
        return;
    }
    let app = app.clone();
    tokio::spawn(async move {
        loop {
            let conns: Vec<Arc<Conn>> = CONNS.lock().unwrap().clone();
            if conns.is_empty() {
                LOOP.store(0, Ordering::SeqCst);
                if CONNS.lock().unwrap().is_empty() {
                    return;
                }
                LOOP.store(1, Ordering::SeqCst);
            }
            let mut cache: HashMap<String, (String, u64)> = HashMap::new();
            for c in &conns {
                let watches: Vec<(String, Value)> = c.watches.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                for (key, params) in watches {
                    let Some(ivl) = interval(&key) else { continue };
                    let now = Instant::now();
                    {
                        let mut due = c.due.lock().unwrap();
                        if due.get(&key).is_some_and(|d| now < *d) {
                            continue;
                        }
                        due.insert(key.clone(), now + Duration::from_secs(ivl));
                    }
                    let ck = format!("{key}:{}", scope_key(&app, &key, &c.auth, &params).await);
                    let (frame, dg) = match cache.get(&ck) {
                        Some(x) => x.clone(),
                        None => {
                            let built = build(&app, &key, &c.auth, &params).await;
                            let x = match built {
                                Ok(data) => {
                                    let dj = data.to_string();
                                    let f = format!("{{\"type\":\"snapshot\",\"key\":{},\"data\":{dj},\"generated_at\":\"{}\"}}", json!(key), crate::util::iso_now());
                                    (f, digest(&format!("s{dj}")))
                                }
                                Err(e) => {
                                    let dg = digest(&format!("e{e}"));
                                    if c.digests.lock().unwrap().get(&key) != Some(&dg) {
                                        app.telemetry.record(crate::telemetry::Event::new("websocket", "snapshot_error").user(Some(&c.auth.username)).text(e.clone()).meta(json!({ "key": key })));
                                    }
                                    (json!({ "type": "snapshot_error", "key": key, "error": e, "generated_at": crate::util::iso_now() }).to_string(), dg)
                                }
                            };
                            cache.insert(ck, x.clone());
                            x
                        }
                    };
                    let changed = c.digests.lock().unwrap().get(&key) != Some(&dg);
                    if changed && c.tx.try_send(frame).is_ok() {
                        c.digests.lock().unwrap().insert(key, dg);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

pub async fn handler(State(app): State<AppRef>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if !crate::http::ws_origin_ok(&app, &headers) {
        return (axum::http::StatusCode::FORBIDDEN, "").into_response();
    }
    let ip = crate::http::client_ip(&headers, None);
    ws.max_message_size(64 * 1024).on_upgrade(move |sock| run(app, sock, ip))
}

async fn run(app: AppRef, sock: WebSocket, ip: String) {
    let (mut sink, mut stream) = sock.split();
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next()).await.ok().flatten().and_then(|m| m.ok());
    let auth = match first {
        Some(Message::Text(t)) => {
            let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
            if v.s("type").as_deref() == Some("auth") {
                match crate::auth::verify_api_token(&app.settings.session_secret, &v.s("token").unwrap_or_default()) {
                    Ok(a) => Some(a),
                    Err(reason) => {
                        app.record_token_rejected("/ws", &ip, reason, "websocket");
                        None
                    }
                }
            } else {
                None
            }
        }
        _ => None,
    };
    let Some(auth) = auth else {
        let _ = sink.send(Message::Close(Some(CloseFrame { code: 4401, reason: "".into() }))).await;
        return;
    };
    app.telemetry.ws_connected();
    app.telemetry.record(crate::telemetry::Event::new("websocket", "connect").path("/ws"));
    let started = Instant::now();
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let conn = Arc::new(Conn { auth: auth.clone(), tx, watches: Mutex::new(HashMap::from([("dashboard".into(), Value::Bool(true))])), digests: Mutex::default(), due: Mutex::default() });
    CONNS.lock().unwrap().push(conn.clone());
    ensure_loop(&app);

    let mut last_touch = Instant::now();
    loop {
        tokio::select! {
            out = rx.recv() => {
                let Some(out) = out else { break };
                if sink.send(Message::Text(out.into())).await.is_err() { break }
            }
            msg = tokio::time::timeout(Duration::from_secs(45), stream.next()) => {
                match msg {
                    Err(_) => {
                        let ping = json!({ "type": "ping", "timestamp": crate::util::iso_now() }).to_string();
                        if sink.send(Message::Text(ping.into())).await.is_err() { break }
                    }
                    Ok(None) | Ok(Some(Err(_))) => break,
                    Ok(Some(Ok(m))) => {
                        if last_touch.elapsed() >= Duration::from_secs(60) {
                            last_touch = Instant::now();
                            app.touch_presence(&auth).await;
                        }
                        match m {
                            Message::Text(t) => {
                                if t.len() > 512 {
                                    let _ = sink.send(Message::Close(Some(CloseFrame { code: 4409, reason: "".into() }))).await;
                                    break;
                                }
                                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                                let key = v.s("key").unwrap_or_default();
                                match v.s("type").as_deref() {
                                    Some("watch") if interval(&key).is_some() => {
                                        let params = v.get("params").filter(|p| p.is_object()).cloned().unwrap_or(Value::Bool(true));
                                        conn.watches.lock().unwrap().insert(key.clone(), params);
                                        conn.due.lock().unwrap().remove(&key);
                                        conn.digests.lock().unwrap().remove(&key);
                                    }
                                    Some("unwatch") => {
                                        conn.watches.lock().unwrap().remove(&key);
                                        conn.due.lock().unwrap().remove(&key);
                                        conn.digests.lock().unwrap().remove(&key);
                                    }
                                    _ => {}
                                }
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    CONNS.lock().unwrap().retain(|c| !Arc::ptr_eq(c, &conn));
    app.telemetry.ws_disconnected();
    app.telemetry.record(crate::telemetry::Event::new("websocket", "disconnect").path("/ws").num(Some(started.elapsed().as_secs_f64())));
    let _ = OWNER_SCOPE;
}
