//! /healthz, /api/health, /api/session/*, /api/telemetry/client.

use crate::accounts;
use crate::app::{account_guild_id, client_name};
use crate::auth::{self, IssueOpts};
use crate::db::J;
use crate::http::{R, Reply, Req};
use crate::notify;
use crate::telemetry::Event;
use serde_json::{Value, json};

pub async fn healthz(req: Req) -> Reply {
    match req.app.db.one("accountlogins", "SELECT 1 AS ok", &[]).await {
        Ok(_) => Reply::ok(json!({ "ok": true, "db": "ok" })),
        Err(_) => Reply::json(503, json!({ "ok": false, "db": "unavailable" })),
    }
}

pub async fn health(req: Req) -> R<Reply> {
    let a = req.auth().ok_or_else(|| Reply::err(401, "Authentication required"))?;
    let ok = req.app.db.one("accountlogins", "SELECT 1 AS ok", &[]).await.is_ok();
    let s = &req.app.settings;
    Ok(Reply::ok(json!({ "ok": ok, "db": if ok { "ok" } else { "unavailable" }, "api_max_rows": s.api_max_rows, "bot_control_source_max_chars": s.bot_control_source_max_chars })).by(&a))
}

async fn resolve_guild(req: &Req, a: &auth::Auth) -> Option<String> {
    if a.guild_id.is_some() {
        return a.guild_id.clone();
    }
    accounts::guild_id_for_username(&req.app, &a.username).await
}

pub async fn session(req: Req) -> Reply {
    let app = &req.app;
    let Some(a) = req.auth() else { return Reply::ok(json!({ "authenticated": false, "pages_public_url": app.settings.pages_public_url })) };
    let username = if a.username.is_empty() { app.settings.admin_username.clone() } else { a.username.clone() };
    let role = a.role.clone().unwrap_or_else(|| "admin".into());
    let gid = resolve_guild(&req, &a).await;
    let (site_owner, admin_mode) = (a.site_owner, a.is_admin());
    let token = app.issue(&username, IssueOpts { role: role.clone(), guild_id: gid.clone().or(a.guild_id.clone()), admin_mode: Some(admin_mode), site_owner, moderator: a.moderator, ttl: app.token_ttl(a.device), device: a.device, client: a.client.clone() });
    if a.client.as_deref().is_some_and(|c| c != "web") {
        app.telemetry.record(Event::new("auth", "session_refresh").user(Some(&username)).guild(gid.as_deref()).ip(&req.client_ip).num(Some((a.exp - crate::util::now()) as f64)).meta(json!({ "client": a.client, "device": a.device })));
    }
    Reply::ok(json!({
        "authenticated": true, "mode": "token", "username": username, "role": role,
        "guild_id": a.guild_id.clone().or(gid.clone()), "account_guild_id": gid, "site_owner": site_owner, "admin_mode": admin_mode, "moderator": a.moderator,
        "image_gallery_owner": admin_mode && site_owner, "token": token, "pages_public_url": app.settings.pages_public_url, "expires_in": app.token_ttl(a.device),
    }))
    .cookie(app.session_cookie(&token))
    .by(&a)
}

pub async fn admin_mode(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    let app = &req.app;
    if !a.site_owner {
        return Err(Reply::err(403, "Admin mode is locked to the verified owner account."));
    }
    let username = if a.username.is_empty() { app.settings.admin_username.clone() } else { a.username.clone() };
    let gid = resolve_guild(&req, &a).await.ok_or_else(|| Reply::err(400, "This owner account is not linked to a registered guild."))?;
    let on = req.jb("enabled");
    if !accounts::set_admin_mode_preference(app, &username, &gid, on).await {
        println!("[swarmpanel] could not save admin-mode preference for {username}");
    }
    let role = a.role.clone().unwrap_or_else(|| "account".into());
    let token = app.issue(&username, IssueOpts { role: role.clone(), guild_id: Some(gid.clone()), admin_mode: Some(on), site_owner: true, moderator: a.moderator, ttl: app.token_ttl(a.device), device: a.device, client: a.client.clone() });
    Ok(Reply::ok(json!({
        "ok": true, "username": username, "role": role, "guild_id": gid, "account_guild_id": gid, "site_owner": true, "admin_mode": on,
        "image_gallery_owner": on, "token": token, "pages_public_url": app.settings.pages_public_url, "expires_in": app.token_ttl(a.device),
    }))
    .cookie(app.session_cookie(&token))
    .by(&a))
}

pub struct LoginResult {
    pub token: String,
    pub body: Value,
}

/// Shared by POST /api/session/login and the server-rendered sign-in form.
pub async fn do_login(req: &Req, username: &str, password: &str, guild_id: Option<String>, client: Option<String>, remember: bool, disable_admin: bool) -> R<LoginResult> {
    let app = &req.app;
    let lim = &app.limiter;
    lim.check(&format!("login-api:{}:{}", req.client_ip, crate::util::cut(&username.to_lowercase(), 80)), 15, 900)?;
    lim.check(&format!("login-api-username:{}", crate::util::cut(&username.to_lowercase(), 80)), 30, 900)?;
    let s = &app.settings;
    let mut res: Option<(String, String, Option<String>, bool, bool, bool)> = None; // username, role, guild, site_owner, moderator, admin_mode
    if auth::verify_credentials(username, password, &s.admin_username, &s.admin_password) {
        res = Some((username.to_string(), "admin".into(), None, true, false, true));
    } else {
        let secret = if !password.is_empty() { Some(password.to_string()) } else { guild_id.clone() };
        if let Some(secret) = secret {
            if let Some(acct) = accounts::authenticate(app, username, &secret).await {
                let owner = acct.email.as_deref().is_some_and(|e| !s.site_owner_email.is_empty() && e.to_lowercase() == s.site_owner_email) && (!s.owner_email_requires_verification || acct.email_verified);
                let mut admin = owner;
                if owner && accounts::get_admin_mode_preference(app, &acct.username, &acct.guild_id).await == Some(false) {
                    admin = false;
                }
                res = Some((acct.username, "account".into(), Some(acct.guild_id), owner, acct.panel_role == "moderator", admin));
            }
        }
    }
    let client = client_name(client).unwrap_or_else(|| "web".into());
    let Some((user, role, gid, owner, moderator, mut admin)) = res else {
        app.telemetry.record(Event::new("auth", "login_failed").user(Some(username)).ip(&req.client_ip).meta(json!({ "client": client })));
        return Err(Reply::err(401, "Invalid username or password"));
    };
    let device = remember && client != "web";
    app.telemetry.record(Event::new("auth", "login_success").user(Some(&user)).guild(gid.as_deref()).ip(&req.client_ip).meta(json!({ "role": role, "site_owner": owner, "client": client, "device": device })));
    if disable_admin {
        admin = false;
    }
    let ttl = app.token_ttl(device);
    let token = app.issue(&user, IssueOpts { role: role.clone(), guild_id: gid.clone(), admin_mode: Some(admin), site_owner: owner, moderator, ttl, device, client: Some(client) });
    let body = json!({
        "ok": true, "token": token, "username": user, "role": role, "guild_id": gid, "account_guild_id": gid, "site_owner": owner, "admin_mode": admin, "moderator": moderator,
        "image_gallery_owner": admin && owner, "pages_public_url": s.pages_public_url, "expires_in": ttl,
    });
    Ok(LoginResult { token, body })
}

pub async fn login(req: Req) -> R<Reply> {
    let r = do_login(&req, &req.js("username").unwrap_or_default(), &req.js("password").unwrap_or_default(), req.js("guild_id"), req.js("client"), req.jb("remember_device"), req.jb("disable_admin_mode")).await?;
    Ok(Reply::ok(r.body).cookie(req.app.session_cookie(&r.token)))
}

fn clean_event_name(v: &Value) -> Option<String> {
    let s: String = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()).chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
    let s = crate::util::cut(&s, 80);
    (!s.is_empty() && !v.is_null()).then_some(s)
}
fn meta_str(v: Option<&Value>, n: usize) -> Value {
    match v {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(s)) => json!(crate::util::cut(s, n)),
        Some(o) => json!(crate::util::cut(&o.to_string(), n)),
    }
}

pub async fn client_telemetry(req: Req) -> Reply {
    let app = &req.app;
    if req.raw_body.len() > 65536 {
        return Reply::err(413, "Telemetry batch too large.");
    }
    let a = req.auth();
    let who = a.as_ref().map(|a| a.username.clone()).unwrap_or_else(|| req.client_ip.clone()).to_lowercase();
    if !app.limiter.allow(&format!("client-telemetry:{who}"), 10, 60) {
        return Reply::err(429, "Too many telemetry batches. Try again later.");
    }
    let client = client_name(req.js("client")).unwrap_or_else(|| "unknown".into());
    let base = json!({ "client": client, "app_version": meta_str(req.json.get("app_version"), 40), "os_version": meta_str(req.json.get("os_version"), 40), "device_model": meta_str(req.json.get("device_model"), 60), "launch_id": meta_str(req.json.get("launch_id"), 64) });
    let mut accepted = 0;
    for e in req.json.get("events").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().take(50) {
        let Some(name) = e.get("name").and_then(clean_event_name) else { continue };
        let mut meta = base.clone();
        if let Some(m) = e.get("metadata").and_then(Value::as_object) {
            for (k, v) in m.iter().take(20) {
                meta[crate::util::cut(k, 40)] = if v.is_number() || v.is_boolean() { v.clone() } else { meta_str(Some(v), 200) };
            }
        }
        meta["client_ts"] = json!(e.f("at"));
        let mut ev = Event::new("client", &format!("{client}.{name}")).ip(&req.client_ip).num(e.f("value")).meta(meta);
        if let Some(t) = e.get("text").filter(|t| !t.is_null()) {
            ev.text_value = meta_str(Some(t), 500).as_str().map(str::to_string);
        }
        if let Some(a) = &a {
            ev = ev.user(Some(&a.username)).guild(a.guild_id.as_deref());
        }
        app.telemetry.record(ev);
        accepted += 1;
    }
    Reply::ok(json!({ "ok": true, "accepted": accepted }))
}

pub async fn logout(req: Req) -> Reply {
    Reply::ok(json!({ "ok": true })).cookie(req.app.clear_cookie())
}

/// (username, guild) for account-only routes.
async fn account(req: &Req) -> R<(auth::Auth, String, String)> {
    let a = req.require_auth().await?;
    let g = account_guild_id(&a).filter(|_| !a.username.is_empty()).ok_or_else(|| Reply::err(403, "Guild account access required"))?;
    let u = a.username.clone();
    Ok((a, u, g))
}

pub async fn password(req: Req) -> R<Reply> {
    let (a, u, g) = account(&req).await?;
    req.app.limiter.check(&format!("password-change:{}", u.to_lowercase()), 3, 600)?;
    match accounts::update_password(&req.app, &u, &g, &req.js("current_password").unwrap_or_default(), &req.js("new_password").unwrap_or_default()).await {
        Err(e) => Err(Reply::err(400, e)),
        Ok(None) => Err(Reply::err(404, "Account profile not found")),
        Ok(Some(p)) => Ok(Reply::ok(json!({ "ok": true, "profile": p })).by(&a)),
    }
}

pub async fn register(req: Req) -> R<Reply> {
    let app = &req.app;
    app.limiter.check(&format!("register-api:{}", req.client_ip), 10, 3600)?;
    let proof_url = req.js("verification_webhook_url").or_else(|| req.js("registration_proof_url")).filter(|s| !s.is_empty());
    let code = auth::verification_code();
    let gid_in = req.js("guild_id").unwrap_or_default();
    let mut proof = (None, None);
    if let Some(u) = &proof_url {
        let (c, n) = notify::verify_guild_proof(app, &gid_in, u).await.map_err(|e| Reply::err(400, e))?;
        proof = (Some(c), Some(n));
    }
    let acct = accounts::register(
        app,
        accounts::Registration {
            username: req.js("username").unwrap_or_default(),
            guild_id: gid_in,
            password: req.js("password").unwrap_or_default(),
            email: req.js("email").unwrap_or_default(),
            webhook_url: proof_url.clone(),
            webhook_channel_id: proof.0,
            webhook_name: proof.1,
            webhook_code: Some(code.clone()),
        },
    )
    .await
    .map_err(|e| Reply::err(400, e))?;
    let (u, g) = (acct.s("username").unwrap_or_default(), acct.s("guild_id").unwrap_or_default());
    let mut sent = false;
    if let Some(url) = &proof_url {
        sent = notify::send_verification_webhook_code(app, url, &code, &u, &g).await;
    }
    let dm_id = req.js("discord_user_id").unwrap_or_default().trim().to_string();
    if !sent && !dm_id.is_empty() && accounts::update_discord_user_id(app, &u, &g, &dm_id).await.is_ok() {
        accounts::issue_discord_dm_code(app, &u, &g, &code).await;
        sent = notify::send_discord_dm(app, &dm_id, &notify::dm_code_text(&code, app.settings.discord_dm_verification_ttl_seconds)).await.is_ok();
    }
    let client = client_name(req.js("client")).unwrap_or_else(|| "web".into());
    let device = req.jb("remember_device") && client != "web";
    let ttl = app.token_ttl(device);
    let token = app.issue(&u, IssueOpts { role: "account".into(), guild_id: Some(g.clone()), admin_mode: Some(false), site_owner: false, moderator: false, ttl, device, client: Some(client) });
    Ok(Reply::ok(json!({
        "ok": true, "token": token, "username": u, "role": "account", "guild_id": g, "account_guild_id": g, "site_owner": false, "admin_mode": false, "moderator": false,
        "image_gallery_owner": false, "verification_sent": sent, "pages_public_url": app.settings.pages_public_url, "expires_in": ttl,
    })))
}

fn verified(p: &Value) -> bool {
    p.b("webhook_verified") || p.b("email_verified") || p.b("discord_dm_verified")
}

pub async fn resend_verification(req: Req) -> R<Reply> {
    let (a, u, g) = account(&req).await?;
    let app = &req.app;
    app.limiter.check(&format!("session-verification-resend:{}", u.to_lowercase()), 8, 3600)?;
    let p = accounts::get_account_profile(app, &u, &g).await.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    let hook = p.s("verification_webhook_url").filter(|s| !s.is_empty());
    let dm = p.s("discord_user_id").filter(|s| !s.is_empty());
    if hook.is_none() && dm.is_none() {
        return Err(Reply::err(400, "Set a Discord verification webhook or Discord User ID before requesting a code."));
    }
    if verified(&p) {
        return Ok(Reply::ok(json!({ "ok": true, "verification_sent": false, "already_verified": true })).by(&a));
    }
    let code = auth::verification_code();
    let sent = if hook.is_some() {
        match accounts::issue_webhook_code(app, &u, &g, &code).await {
            Some(p) => notify::send_verification_webhook_code(app, &p.s("verification_webhook_url").unwrap_or_default(), &code, &u, &g).await,
            None => false,
        }
    } else {
        match accounts::issue_discord_dm_code(app, &u, &g, &code).await {
            Some(p) => notify::send_discord_dm(app, &p.s("discord_user_id").unwrap_or_default(), &notify::dm_code_text(&code, app.settings.discord_dm_verification_ttl_seconds)).await.is_ok(),
            None => false,
        }
    };
    Ok(Reply::ok(json!({ "ok": sent, "verification_sent": sent, "already_verified": false })).by(&a))
}

pub async fn verification_discord(req: Req) -> R<Reply> {
    let (a, u, g) = account(&req).await?;
    let app = &req.app;
    app.limiter.check(&format!("session-verification-discord:{}", u.to_lowercase()), 8, 3600)?;
    let id = req.js("discord_user_id").unwrap_or_default().trim().to_string();
    let p = accounts::update_discord_user_id(app, &u, &g, &id).await.map_err(|e| Reply::err(400, e))?.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    if id.is_empty() {
        return Ok(Reply::ok(json!({ "ok": true, "profile": p, "verification_sent": false })).by(&a));
    }
    let code = auth::verification_code();
    let p = accounts::issue_discord_dm_code(app, &u, &g, &code).await;
    notify::send_discord_dm(app, &id, &notify::dm_code_text(&code, app.settings.discord_dm_verification_ttl_seconds)).await.map_err(|e| Reply::err(400, e))?;
    Ok(Reply::ok(json!({ "ok": true, "profile": p, "verification_sent": true })).by(&a))
}

pub async fn verification_webhook(req: Req) -> R<Reply> {
    let (a, u, g) = account(&req).await?;
    let app = &req.app;
    app.limiter.check(&format!("session-verification-webhook:{}", u.to_lowercase()), 8, 3600)?;
    let url = req.js("verification_webhook_url").unwrap_or_default().trim().to_string();
    accounts::get_account_profile(app, &u, &g).await.ok_or_else(|| Reply::err(404, "Account profile not found"))?;
    if url.is_empty() {
        let p = accounts::update_verification_webhook(app, &u, &g, None, None, None).await;
        return Ok(Reply::ok(json!({ "ok": true, "profile": p, "verification_sent": false })).by(&a));
    }
    let (chan, name) = notify::verify_guild_proof(app, &g, &url).await.map_err(|e| Reply::err(400, e))?;
    accounts::update_verification_webhook(app, &u, &g, Some(url.clone()), Some(chan), Some(name)).await;
    let code = auth::verification_code();
    let p = accounts::issue_webhook_code(app, &u, &g, &code).await;
    let sent = p.is_some() && notify::send_verification_webhook_code(app, &url, &code, &u, &g).await;
    Ok(Reply::ok(json!({ "ok": true, "profile": p, "verification_sent": sent })).by(&a))
}

pub async fn email(req: Req) -> R<Reply> {
    let (a, u, g) = account(&req).await?;
    req.app.limiter.check(&format!("session-email-update:{}", u.to_lowercase()), 8, 3600)?;
    match accounts::update_email(&req.app, &u, &g, &req.js("email").unwrap_or_default()).await {
        Err(e) => Err(Reply::err(400, e)),
        Ok(None) => Err(Reply::err(404, "Account profile not found")),
        Ok(Some(p)) => Ok(Reply::ok(json!({ "ok": true, "profile": p })).by(&a)),
    }
}

pub async fn verify(req: Req) -> R<Reply> {
    let (a, u, g) = account(&req).await?;
    let app = &req.app;
    app.limiter.check(&format!("session-verification-verify:{}", u.to_lowercase()), 20, 3600)?;
    let code: String = req.js("code").unwrap_or_default().chars().filter(char::is_ascii_digit).take(16).collect();
    if code.is_empty() {
        return Err(Reply::err(400, "Enter the verification code you received."));
    }
    let mut p = accounts::verify_webhook_code(app, &u, &g, &code, app.settings.email_verification_ttl_seconds).await;
    if p.is_none() {
        let pending = accounts::get_account_profile(app, &u, &g).await;
        let handle = match pending.and_then(|p| p.s("discord_user_id")) {
            Some(id) => notify::discord_username(app, &id).await,
            None => None,
        };
        p = accounts::verify_discord_dm_code(app, &u, &g, &code, app.settings.discord_dm_verification_ttl_seconds, handle).await;
    }
    let p = p.ok_or_else(|| Reply::err(400, "Invalid verification code."))?;
    Ok(Reply::ok(json!({ "ok": true, "profile": p })).by(&a))
}
