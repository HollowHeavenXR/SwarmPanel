//! Panel accounts in accountlogins.users (port of accounts.lua).

use crate::app::AppRef;
use crate::auth;
use crate::db::{J, P};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

const DB: &str = "accountlogins";
static DM_COLUMNS: AtomicBool = AtomicBool::new(false);

async fn ensure_dm_columns(app: &AppRef) {
    if DM_COLUMNS.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = app
        .db
        .batch(
            DB,
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS discord_user_id VARCHAR(32);
             ALTER TABLE users ADD COLUMN IF NOT EXISTS discord_username VARCHAR(120);
             ALTER TABLE users ADD COLUMN IF NOT EXISTS discord_dm_verified_at TIMESTAMP;
             ALTER TABLE users ADD COLUMN IF NOT EXISTS discord_dm_verification_code_hash CHAR(64);
             ALTER TABLE users ADD COLUMN IF NOT EXISTS discord_dm_verification_sent_at TIMESTAMP;",
        )
        .await;
}

pub fn normalize_username(v: &str) -> Result<String, String> {
    let u = v.trim();
    if u.len() < 2 || u.len() > 80 || !u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
        return Err("Username must be 2-80 characters using letters, numbers, dots, dashes, or underscores.".into());
    }
    Ok(u.to_string())
}

/// Ok(None) for blank.
pub fn normalize_email(v: &str) -> Result<Option<String>, String> {
    let e = v.trim().to_lowercase();
    if e.is_empty() {
        return Ok(None);
    }
    let valid = (|| {
        let (local, domain) = e.split_once('@')?;
        let ok = !local.is_empty() && !domain.contains('@') && !e.chars().any(char::is_whitespace);
        let (a, b) = domain.rsplit_once('.')?;
        (ok && !a.is_empty() && !b.is_empty()).then_some(())
    })();
    if e.len() > 255 || valid.is_none() {
        return Err("Enter a valid email address.".into());
    }
    Ok(Some(e))
}

pub fn normalize_password(v: &str, field: &str) -> Result<String, String> {
    if v.len() < 8 {
        return Err(format!("{field} must be at least 8 characters."));
    }
    Ok(v.to_string())
}

const PROFILE_COLUMNS: &str = "id, username, guild_id, email, email_verified_at, verification_webhook_url, verification_webhook_channel_id, verification_webhook_name, \
webhook_verified_at, webhook_verification_sent_at, discord_user_id, discord_username, discord_dm_verified_at, discord_dm_verification_sent_at, \
password_hash, display_name, avatar_url, bio, profile_headline, profile_tags, profile_links, profile_banner_url, profile_banner_mode, profile_card_style, profile_social_mode, \
favorite_bot, theme_accent, public_profile, server_invite_url, server_name, server_icon_url, panel_preferences, profile_quote, profile_layout_mode, profile_header_style, profile_border_accent, \
panel_role, created_at, last_login_at, last_seen_at, updated_at";

/// Postgres timestamp text (UTC) -> unix seconds.
pub fn ts_epoch(v: &str) -> Option<i64> {
    let s = v.get(..19)?.replace('T', " ");
    chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S").ok().map(|d| d.and_utc().timestamp())
}

pub fn is_recently_seen(v: Option<&str>, window: i64) -> bool {
    v.and_then(ts_epoch).is_some_and(|t| crate::util::now() - t <= window)
}

fn present(v: &Value, k: &str) -> bool {
    !matches!(v.get(k), None | Some(Value::Null))
}
fn nonblank(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_str).is_some_and(|s| !s.is_empty())
}

pub fn serialize_profile(mut p: Value) -> Value {
    if let Some(g) = p.get("guild_id").filter(|g| !g.is_null()).map(|g| g.to_string().trim_matches('"').to_string()) {
        p["guild_id"] = json!(g);
    }
    if p.s("display_name").is_none() {
        p["display_name"] = p.get("username").cloned().unwrap_or(Value::Null);
    }
    p["public_profile"] = json!(p.b("public_profile"));
    let webhook = present(&p, "webhook_verified_at");
    let dm = present(&p, "discord_dm_verified_at");
    let verified = webhook || present(&p, "email_verified_at") || dm;
    p["webhook_verified"] = json!(webhook);
    p["discord_dm_verified"] = json!(dm);
    p["verification_verified"] = json!(verified);
    p["verification_pending"] = json!((nonblank(&p, "verification_webhook_url") || nonblank(&p, "discord_user_id")) && !verified);
    p["verification_webhook_configured"] = json!(nonblank(&p, "verification_webhook_url"));
    p["email_verified"] = json!(verified);
    p["has_password"] = json!(present(&p, "password_hash"));
    p["is_online"] = json!(is_recently_seen(p.get("last_seen_at").and_then(Value::as_str), 180));
    if let Some(m) = p.as_object_mut() {
        m.remove("password_hash");
    }
    for k in ["panel_preferences", "profile_tags", "profile_links"] {
        let v = p.get(k).cloned().unwrap_or(Value::Null);
        p[k] = match v {
            Value::String(s) => serde_json::from_str::<Value>(&s).ok().filter(|d| d.is_object() || d.is_array()).unwrap_or_else(|| json!([])),
            Value::Object(_) | Value::Array(_) => v,
            _ => json!([]),
        };
    }
    p
}

pub struct LoginAccount {
    pub username: String,
    pub guild_id: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub panel_role: String,
}

pub async fn authenticate(app: &AppRef, username: &str, secret: &str) -> Option<LoginAccount> {
    let uname = normalize_username(username).ok()?;
    if secret.is_empty() {
        return None;
    }
    let row = app.db.opt(DB, "SELECT username, guild_id, email, email_verified_at, password_hash, panel_role FROM users WHERE username = $1 LIMIT 1", &[uname.clone().into()]).await?;
    let hash = row.s("password_hash");
    let password_ok = match &hash {
        Some(h) => auth::verify_password_async(secret.to_string(), h.clone()).await,
        None => false,
    };
    let legacy_ok = !password_ok && hash.is_none() && row.s("guild_id").unwrap_or_default() == secret.trim();
    if !password_ok && !legacy_ok {
        return None;
    }
    let gid = row.s("guild_id").unwrap_or_default();
    let _ = app.db.exec(DB, "UPDATE users SET last_login_at = CURRENT_TIMESTAMP, last_seen_at = CURRENT_TIMESTAMP WHERE username = $1 AND guild_id = $2", &[uname.into(), gid.clone().into()]).await;
    Some(LoginAccount {
        username: row.s("username").unwrap_or_default(),
        guild_id: gid,
        email: row.s("email"),
        email_verified: present(&row, "email_verified_at"),
        panel_role: row.s("panel_role").unwrap_or_else(|| "member".into()),
    })
}

pub async fn guild_id_for_username(app: &AppRef, username: &str) -> Option<String> {
    let u = normalize_username(username).ok()?;
    app.db.opt(DB, "SELECT guild_id FROM users WHERE username = $1 LIMIT 1", &[u.into()]).await?.s("guild_id")
}

pub async fn touch_account_seen(app: &AppRef, username: &str, guild_id: &str, min_interval: i64) {
    let Ok(u) = normalize_username(username) else { return };
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET last_seen_at = CURRENT_TIMESTAMP WHERE username = $1 AND guild_id = $2 AND (last_seen_at IS NULL OR last_seen_at < NOW() - make_interval(secs => $3::bigint))",
            &[u.into(), guild_id.into(), min_interval.into()],
        )
        .await;
}

pub async fn get_account_profile(app: &AppRef, username: &str, guild_id: &str) -> Option<Value> {
    ensure_dm_columns(app).await;
    let u = normalize_username(username).ok()?;
    let row = app.db.one(DB, &format!("SELECT {PROFILE_COLUMNS} FROM users WHERE username = $1 AND guild_id = $2 LIMIT 1"), &[u.into(), guild_id.into()]).await.ok().flatten()?;
    Some(serialize_profile(row))
}

pub async fn get_account_by_id(app: &AppRef, id: impl Into<P>) -> Option<Value> {
    ensure_dm_columns(app).await;
    let cols = PROFILE_COLUMNS.replace("password_hash, ", "");
    let row = app.db.opt(DB, &format!("SELECT {cols} FROM users WHERE id = $1 LIMIT 1"), &[id.into()]).await?;
    Some(serialize_profile(row))
}

pub async fn get_panel_preferences(app: &AppRef, username: &str, guild_id: Option<&str>) -> Value {
    let defaults = crate::profiles::default_panel_preferences();
    let Some(g) = guild_id.filter(|g| !g.is_empty()) else { return defaults };
    let Some(p) = get_account_profile(app, username, g).await else { return defaults };
    crate::profiles::clean_panel_preferences(p.get("panel_preferences").filter(|v| v.is_object()), &defaults)
}

pub async fn get_admin_mode_preference(app: &AppRef, username: &str, guild_id: &str) -> Option<bool> {
    let p = get_account_profile(app, username, guild_id).await?;
    p.get("panel_preferences")?.get("admin_mode_enabled").map(|v| v == &Value::Bool(true))
}

pub async fn set_admin_mode_preference(app: &AppRef, username: &str, guild_id: &str, enabled: bool) -> bool {
    let Some(p) = get_account_profile(app, username, guild_id).await else { return false };
    let mut prefs = p.get("panel_preferences").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
    prefs["admin_mode_enabled"] = json!(enabled);
    app.db.exec(DB, "UPDATE users SET panel_preferences = $1 WHERE username = $2 AND guild_id = $3", &[prefs.to_string().into(), username.trim().into(), guild_id.into()]).await.is_ok()
}

const PROFILE_FIELDS: &[&str] = &[
    "email", "email_verified_at", "email_verification_token_hash", "email_verification_code_hash", "email_verification_sent_at", "verification_webhook_url", "verification_webhook_channel_id",
    "verification_webhook_name", "webhook_verified_at", "webhook_verification_code_hash", "webhook_verification_sent_at", "display_name", "avatar_url", "bio", "profile_headline",
    "profile_tags", "profile_links", "profile_banner_url", "profile_banner_mode", "profile_card_style", "profile_social_mode", "favorite_bot", "theme_accent", "public_profile", "server_invite_url",
    "server_name", "server_icon_url", "panel_preferences", "profile_quote", "profile_layout_mode", "profile_header_style", "profile_border_accent", "last_seen_at", "panel_role",
];

/// `updates`: column -> value (Null clears it). Unknown columns are ignored.
pub async fn update_account_profile(app: &AppRef, username: &str, guild_id: &str, updates: &Map<String, Value>) -> Option<Value> {
    let u = normalize_username(username).ok()?;
    let mut sets = Vec::new();
    let mut params: Vec<P> = Vec::new();
    for (k, v) in updates {
        if PROFILE_FIELDS.contains(&k.as_str()) {
            params.push(match v {
                Value::Array(_) | Value::Object(_) => P::Text(v.to_string()),
                other => P::from(other),
            });
            sets.push(format!("{k} = ${}", params.len()));
        }
    }
    if !sets.is_empty() {
        params.push(u.clone().into());
        params.push(guild_id.into());
        let n = params.len();
        let _ = app.db.exec(DB, &format!("UPDATE users SET {} WHERE username = ${} AND guild_id = ${n}", sets.join(", "), n - 1), &params).await;
    }
    get_account_profile(app, &u, guild_id).await
}

pub async fn update_panel_preferences(app: &AppRef, username: &str, guild_id: &str, prefs: &Value) -> Option<Value> {
    let u = normalize_username(username).ok()?;
    let _ = app.db.exec(DB, "UPDATE users SET panel_preferences = $1 WHERE username = $2 AND guild_id = $3", &[prefs.to_string().into(), u.clone().into(), guild_id.into()]).await;
    get_account_profile(app, &u, guild_id).await
}

pub async fn search_account_profiles(app: &AppRef, query: &str, limit: i64, viewer: i64, guild_id: Option<&str>, online_only: bool) -> Vec<Value> {
    let q = query.trim().to_string();
    let lim = limit.clamp(1, 50);
    let like = format!("%{q}%");
    let mut params: Vec<P> = vec![q.into(), like.into(), lim.into()];
    let guild_clause = match guild_id.filter(|g| !g.is_empty()) {
        Some(g) => {
            params.push(g.into());
            "AND guild_id = $4"
        }
        None => "",
    };
    let online = if online_only { "AND last_seen_at >= NOW() - INTERVAL '180 seconds'" } else { "" };
    let order = if online_only { "last_seen_at DESC, username ASC" } else { "COALESCE(updated_at, created_at) DESC, username ASC" };
    let sql = format!(
        "SELECT id, username, guild_id, email_verified_at, webhook_verified_at, display_name, avatar_url, bio, profile_headline, profile_tags, profile_links, profile_banner_url, profile_banner_mode, \
         profile_card_style, profile_social_mode, favorite_bot, theme_accent, public_profile, server_invite_url, server_name, server_icon_url, profile_quote, profile_layout_mode, \
         profile_header_style, profile_border_accent, created_at, last_login_at, last_seen_at, updated_at FROM users u \
         WHERE public_profile = TRUE {guild_clause} {online} AND ($1 = '' OR username ILIKE $2 OR COALESCE(display_name, '') ILIKE $2 OR COALESCE(server_name, '') ILIKE $2 OR COALESCE(favorite_bot, '') ILIKE $2) \
         ORDER BY {order} LIMIT $3"
    );
    let mut profiles: Vec<Value> = app.db.all(DB, &sql, &params).await.into_iter().map(serialize_profile).collect();
    let ids: Vec<i64> = profiles.iter().filter_map(|p| p.i("id")).collect();
    let snaps = crate::social::get_account_social_snapshots(app, &ids, viewer).await;
    for p in profiles.iter_mut() {
        if let Some(Value::Object(s)) = p.i("id").and_then(|id| snaps.get(&id)) {
            for (k, v) in s {
                p[k] = v.clone();
            }
        }
    }
    let gids: Vec<String> = profiles.iter().filter_map(|p| p.s("guild_id")).collect();
    let summaries = crate::dashboard::music_activity_for_guilds(app, &gids).await;
    for p in profiles.iter_mut() {
        let g = p.s("guild_id").unwrap_or_default();
        p["activity"] = summaries.get(&g).cloned().unwrap_or_else(crate::dashboard::empty_activity);
    }
    profiles
}

const ADMIN_COLUMNS: &str = "id, username, guild_id, email, email_verified_at, email_verification_sent_at, verification_webhook_url, verification_webhook_channel_id, verification_webhook_name, \
webhook_verified_at, webhook_verification_sent_at, password_hash, display_name, avatar_url, bio, profile_headline, profile_tags, profile_links, profile_banner_url, profile_banner_mode, \
profile_card_style, profile_social_mode, favorite_bot, theme_accent, public_profile, server_invite_url, server_name, server_icon_url, panel_preferences, profile_quote, profile_layout_mode, \
profile_header_style, profile_border_accent, panel_role, created_at, last_login_at, last_seen_at, updated_at";

pub async fn get_account_admin(app: &AppRef, id: impl Into<P>) -> Option<Value> {
    app.db.opt(DB, &format!("SELECT {ADMIN_COLUMNS} FROM users WHERE id = $1 LIMIT 1"), &[id.into()]).await.map(serialize_profile)
}

pub async fn get_account_admin_data(app: &AppRef, query: &str, limit: i64) -> Value {
    let q = query.trim().to_string();
    let lim = limit.clamp(1, 200);
    let like = format!("%{q}%");
    let s = app
        .db
        .opt(
            DB,
            "SELECT COUNT(*) AS total_accounts, SUM(CASE WHEN email IS NOT NULL THEN 1 ELSE 0 END) AS accounts_with_email, \
             SUM(CASE WHEN webhook_verified_at IS NOT NULL OR email_verified_at IS NOT NULL THEN 1 ELSE 0 END) AS verified_emails, \
             SUM(CASE WHEN verification_webhook_url IS NOT NULL AND verification_webhook_url != '' AND webhook_verified_at IS NULL AND email_verified_at IS NULL THEN 1 ELSE 0 END) AS pending_emails, \
             SUM(CASE WHEN public_profile = TRUE THEN 1 ELSE 0 END) AS public_profiles, SUM(CASE WHEN password_hash IS NOT NULL AND password_hash != '' THEN 1 ELSE 0 END) AS passwords_set FROM users",
            &[],
        )
        .await
        .unwrap_or_else(|| json!({}));
    let users: Vec<Value> = app
        .db
        .all(
            DB,
            "SELECT id, username, guild_id, email, email_verified_at, email_verification_sent_at, verification_webhook_url, verification_webhook_channel_id, verification_webhook_name, \
             webhook_verified_at, webhook_verification_sent_at, password_hash, display_name, favorite_bot, public_profile, server_name, panel_role, created_at, last_login_at, last_seen_at, updated_at \
             FROM users WHERE ($1 = '' OR username ILIKE $2 OR CAST(guild_id AS TEXT) ILIKE $2 OR COALESCE(email, '') ILIKE $2 OR COALESCE(display_name, '') ILIKE $2 OR COALESCE(server_name, '') ILIKE $2) \
             ORDER BY COALESCE(updated_at, created_at) DESC, username ASC LIMIT $3",
            &[q.clone().into(), like.into(), lim.into()],
        )
        .await
        .into_iter()
        .map(serialize_profile)
        .collect();
    let n = |k: &str| s.i(k).unwrap_or(0);
    json!({
        "summary": { "total_accounts": n("total_accounts"), "accounts_with_email": n("accounts_with_email"), "verified_emails": n("verified_emails"), "pending_emails": n("pending_emails"), "public_profiles": n("public_profiles"), "passwords_set": n("passwords_set") },
        "users": users, "query": q, "limit": lim,
    })
}

pub async fn update_account_admin(app: &AppRef, id: i64, updates: &Map<String, Value>) -> Result<Value, String> {
    let allowed = ["username", "guild_id", "email", "display_name", "public_profile", "server_name"];
    let mut unknown: Vec<&String> = updates.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!("Unsupported account fields: {}", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let cur = app.db.opt(DB, "SELECT id, username, guild_id, email FROM users WHERE id = $1 LIMIT 1", &[id.into()]).await.ok_or("SwarmPanel account not found.")?;
    let mut cleaned: Vec<(&str, P)> = Vec::new();
    let mut reset_email = false;
    let str_of = |k: &str| updates.get(k).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| if v.is_null() { String::new() } else { v.to_string() }));
    let mut next_user = cur.s("username").unwrap_or_default();
    let mut next_gid = cur.s("guild_id").unwrap_or_default();
    let mut next_email = cur.s("email");
    if let Some(u) = str_of("username") {
        let u = normalize_username(&u)?;
        next_user = u.clone();
        cleaned.push(("username", u.into()));
    }
    if let Some(g) = str_of("guild_id") {
        next_gid = g.clone();
        cleaned.push(("guild_id", g.into()));
    }
    if let Some(e) = str_of("email") {
        let e = normalize_email(&e)?;
        reset_email = true;
        next_email = e.clone();
        cleaned.push(("email", e.into()));
    }
    if let Some(d) = str_of("display_name") {
        let d = d.trim();
        cleaned.push(("display_name", (!d.is_empty()).then(|| crate::util::cut(d, 80)).into()));
    }
    if updates.contains_key("public_profile") {
        cleaned.push(("public_profile", P::Bool(updates.get("public_profile").is_some_and(|v| v.as_bool().unwrap_or(!v.is_null())))));
    }
    if let Some(sn) = str_of("server_name") {
        let sn = sn.trim();
        cleaned.push(("server_name", (!sn.is_empty()).then(|| crate::util::cut(sn, 120)).into()));
    }
    if reset_email && cur.s("email") == next_email {
        reset_email = false;
    }
    if let Some(c) = app
        .db
        .opt(
            DB,
            "SELECT id, username, guild_id, email FROM users WHERE id != $1 AND (username = $2 OR guild_id = $3::bigint OR ($4::text IS NOT NULL AND email = $4)) LIMIT 1",
            &[id.into(), next_user.clone().into(), next_gid.clone().into(), next_email.clone().into()],
        )
        .await
    {
        if c.s("username").as_deref() == Some(next_user.as_str()) {
            return Err("That username is already registered.".into());
        }
        if next_email.is_some() && c.s("email") == next_email {
            return Err("That email is already registered.".into());
        }
        return Err("That guild ID is already registered to another account.".into());
    }
    let mut sets = Vec::new();
    let mut params: Vec<P> = Vec::new();
    for (k, v) in cleaned.iter() {
        params.push(v.clone());
        sets.push(format!("{k} = ${}", params.len()));
    }
    if reset_email {
        for c in ["email_verified_at", "email_verification_token_hash", "email_verification_code_hash", "email_verification_sent_at"] {
            sets.push(format!("{c} = NULL"));
        }
    }
    if !sets.is_empty() {
        params.push(id.into());
        app.db.exec(DB, &format!("UPDATE users SET {} WHERE id = ${}", sets.join(", "), params.len()), &params).await?;
    }
    let old_gid = cur.s("guild_id").unwrap_or_default();
    if cleaned.iter().any(|(k, _)| *k == "guild_id") && next_gid != old_gid {
        let _ = app.db.exec(DB, "DELETE FROM guild_locks WHERE guild_id = $1", &[old_gid.into()]).await;
        let _ = app.db.exec(DB, "INSERT INTO guild_locks (guild_id, username, created_at) VALUES ($1, $2, CURRENT_TIMESTAMP)", &[next_gid.into(), next_user.into()]).await;
    } else if cleaned.iter().any(|(k, _)| *k == "username") && Some(&next_user) != cur.s("username").as_ref() {
        let _ = app.db.exec(DB, "UPDATE guild_locks SET username = $1 WHERE guild_id = $2", &[next_user.into(), next_gid.into()]).await;
    }
    get_account_admin(app, id).await.ok_or_else(|| "SwarmPanel account not found.".into())
}

pub async fn set_panel_role(app: &AppRef, id: i64, moderator: bool) -> Option<Value> {
    let _ = app.db.exec(DB, "UPDATE users SET panel_role = $1 WHERE id = $2", &[if moderator { "moderator" } else { "member" }.into(), id.into()]).await;
    get_account_admin(app, id).await
}

pub async fn delete_account_admin(app: &AppRef, id: i64) -> Result<(), String> {
    let cur = app.db.opt(DB, "SELECT guild_id FROM users WHERE id = $1 LIMIT 1", &[id.into()]).await.ok_or("SwarmPanel account not found.")?;
    for sql in [
        "DELETE FROM account_follows WHERE follower_account_id = $1 OR followed_account_id = $1",
        "DELETE FROM account_friend_requests WHERE requester_account_id = $1 OR addressee_account_id = $1",
        "DELETE FROM account_messages WHERE sender_account_id = $1 OR recipient_account_id = $1",
        "DELETE FROM account_notifications WHERE recipient_account_id = $1",
        "DELETE FROM users WHERE id = $1",
    ] {
        let _ = app.db.exec(DB, sql, &[id.into()]).await;
    }
    let _ = app.db.exec(DB, "DELETE FROM guild_locks WHERE guild_id = $1", &[cur.s("guild_id").into()]).await;
    Ok(())
}

pub async fn set_webhook_verified_admin(app: &AppRef, id: i64, v: bool) -> Option<Value> {
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET webhook_verified_at = CASE WHEN $1 THEN CURRENT_TIMESTAMP ELSE NULL END, email_verified_at = CASE WHEN $1 THEN email_verified_at ELSE NULL END, \
             email_verification_token_hash = CASE WHEN $1 THEN email_verification_token_hash ELSE NULL END, email_verification_code_hash = CASE WHEN $1 THEN email_verification_code_hash ELSE NULL END, \
             email_verification_sent_at = CASE WHEN $1 THEN email_verification_sent_at ELSE NULL END, webhook_verification_code_hash = CASE WHEN $1 THEN NULL ELSE webhook_verification_code_hash END, \
             webhook_verification_sent_at = CASE WHEN $1 THEN NULL ELSE webhook_verification_sent_at END WHERE id = $2",
            &[v.into(), id.into()],
        )
        .await;
    get_account_admin(app, id).await
}

/// Err(msg) for a wrong current password / weak new one; Ok(None) if the account is gone.
pub async fn update_password(app: &AppRef, username: &str, guild_id: &str, current: &str, new: &str) -> Result<Option<Value>, String> {
    let u = normalize_username(username).map_err(|_| "Invalid username.".to_string())?;
    let Some(row) = app.db.opt(DB, "SELECT id, password_hash FROM users WHERE username = $1 AND guild_id = $2 LIMIT 1", &[u.clone().into(), guild_id.into()]).await else { return Ok(None) };
    let ok = match row.s("password_hash") {
        Some(h) => auth::verify_password_async(current.to_string(), h).await,
        None => false,
    };
    if !ok {
        return Err("Current password is incorrect.".into());
    }
    let pw = normalize_password(new, "New password")?;
    let hash = auth::password_hash_async(pw).await;
    app.db.exec(DB, "UPDATE users SET password_hash = $1 WHERE id = $2", &[hash.into(), row.i("id").into()]).await?;
    Ok(get_account_profile(app, &u, guild_id).await)
}

pub async fn reset_password_admin(app: &AppRef, id: i64, new: &str) -> Result<Option<Value>, String> {
    let pw = normalize_password(new, "Password")?;
    let hash = auth::password_hash_async(pw).await;
    app.db.exec(DB, "UPDATE users SET password_hash = $1 WHERE id = $2", &[hash.into(), id.into()]).await?;
    Ok(get_account_admin(app, id).await)
}

pub struct Registration {
    pub username: String,
    pub guild_id: String,
    pub password: String,
    pub email: String,
    pub webhook_url: Option<String>,
    pub webhook_channel_id: Option<String>,
    pub webhook_name: Option<String>,
    pub webhook_code: Option<String>,
}

pub async fn register(app: &AppRef, r: Registration) -> Result<Value, String> {
    let u = normalize_username(&r.username)?;
    let gid = r.guild_id.trim().to_string();
    if gid.is_empty() || gid == "0" || !gid.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Guild ID must be a positive integer.".into());
    }
    let pw = normalize_password(&r.password, "Password")?;
    let em = normalize_email(&r.email)?;
    let clean = |v: Option<String>, n: usize| v.map(|s| crate::util::cut(s.trim(), n)).filter(|s| !s.is_empty());
    let url = clean(r.webhook_url, 4000);
    let chan = clean(r.webhook_channel_id, 64);
    let name = clean(r.webhook_name, 120);
    let code_hash = match (&url, &r.webhook_code) {
        (Some(_), Some(c)) => Some(auth::sha256_hex(c)),
        _ => None,
    };
    if let Some(e) = app.db.opt(DB, "SELECT username, guild_id, email FROM users WHERE username = $1 OR guild_id = $2::bigint OR ($3::text IS NOT NULL AND email = $3) LIMIT 1", &[u.clone().into(), gid.clone().into(), em.clone().into()]).await {
        if e.s("username").as_deref() == Some(u.as_str()) {
            return Err("That username is already registered.".into());
        }
        if em.is_some() && e.s("email") == em {
            return Err("That email is already registered.".into());
        }
        return Err("That guild ID is already registered to another account.".into());
    }
    let hash = auth::password_hash_async(pw).await;
    app.db
        .transaction(
            DB,
            &[
                ("INSERT INTO guild_locks (guild_id, username, created_at) VALUES ($1, $2, CURRENT_TIMESTAMP)", vec![gid.clone().into(), u.clone().into()]),
                (
                    "INSERT INTO users (username, guild_id, password_hash, email, public_profile, panel_role, created_at, verification_webhook_url, verification_webhook_channel_id, verification_webhook_name, \
                     webhook_verification_code_hash, webhook_verification_sent_at) VALUES ($1, $2, $3, $4, TRUE, 'member', CURRENT_TIMESTAMP, $5, $6, $7, $8, CASE WHEN $8::text IS NULL THEN NULL ELSE CURRENT_TIMESTAMP END)",
                    vec![u.clone().into(), gid.clone().into(), hash.into(), em.clone().into(), url.clone().into(), chan.into(), name.into(), code_hash.into()],
                ),
            ],
        )
        .await?;
    Ok(json!({ "username": u, "guild_id": gid, "email": em, "verification_webhook_url": url }))
}

pub async fn update_verification_webhook(app: &AppRef, username: &str, guild_id: &str, url: Option<String>, chan: Option<String>, name: Option<String>) -> Option<Value> {
    let u = normalize_username(username).ok()?;
    let clean = |v: Option<String>, n: usize| v.map(|s| crate::util::cut(s.trim(), n)).filter(|s| !s.is_empty());
    let (url, chan, name) = (clean(url, 4000), clean(chan, 64), clean(name, 120));
    let cur = app.db.opt(DB, "SELECT verification_webhook_url, verification_webhook_channel_id, verification_webhook_name FROM users WHERE username = $1 AND guild_id = $2 LIMIT 1", &[u.clone().into(), guild_id.into()]).await?;
    if cur.s("verification_webhook_url") == url && cur.s("verification_webhook_channel_id") == chan && cur.s("verification_webhook_name") == name {
        return get_account_profile(app, &u, guild_id).await;
    }
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET verification_webhook_url = $1, verification_webhook_channel_id = $2, verification_webhook_name = $3, webhook_verified_at = NULL, webhook_verification_code_hash = NULL, webhook_verification_sent_at = NULL WHERE username = $4 AND guild_id = $5",
            &[url.into(), chan.into(), name.into(), u.clone().into(), guild_id.into()],
        )
        .await;
    get_account_profile(app, &u, guild_id).await
}

pub async fn issue_webhook_code(app: &AppRef, username: &str, guild_id: &str, code: &str) -> Option<Value> {
    let u = normalize_username(username).ok()?;
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET webhook_verification_code_hash = $1, webhook_verification_sent_at = CURRENT_TIMESTAMP WHERE username = $2 AND guild_id = $3 AND verification_webhook_url IS NOT NULL AND verification_webhook_url != '' AND webhook_verified_at IS NULL",
            &[auth::sha256_hex(code).into(), u.clone().into(), guild_id.into()],
        )
        .await;
    get_account_profile(app, &u, guild_id).await
}

pub async fn issue_webhook_code_by_id(app: &AppRef, id: i64, code: &str) -> Option<Value> {
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET webhook_verification_code_hash = $1, webhook_verification_sent_at = CURRENT_TIMESTAMP WHERE id = $2 AND verification_webhook_url IS NOT NULL AND verification_webhook_url != '' AND webhook_verified_at IS NULL",
            &[auth::sha256_hex(code).into(), id.into()],
        )
        .await;
    get_account_admin(app, id).await
}

pub async fn verify_webhook_code(app: &AppRef, username: &str, guild_id: &str, code: &str, max_age: i64) -> Option<Value> {
    let u = normalize_username(username).ok()?;
    let row = app
        .db
        .opt(
            DB,
            "SELECT username, guild_id FROM users WHERE username = $1 AND guild_id = $2 AND verification_webhook_url IS NOT NULL AND verification_webhook_url != '' AND webhook_verification_code_hash = $3 \
             AND webhook_verification_sent_at IS NOT NULL AND webhook_verification_sent_at >= NOW() - make_interval(secs => $4::bigint) LIMIT 1",
            &[u.into(), guild_id.into(), auth::sha256_hex(code).into(), max_age.max(1).into()],
        )
        .await?;
    let (ru, rg) = (row.s("username").unwrap_or_default(), row.s("guild_id").unwrap_or_default());
    let _ = app.db.exec(DB, "UPDATE users SET webhook_verified_at = CURRENT_TIMESTAMP, webhook_verification_code_hash = NULL, webhook_verification_sent_at = NULL WHERE username = $1 AND guild_id = $2", &[ru.clone().into(), rg.clone().into()]).await;
    get_account_profile(app, &ru, &rg).await
}

pub async fn update_discord_user_id(app: &AppRef, username: &str, guild_id: &str, discord_id: &str) -> Result<Option<Value>, String> {
    ensure_dm_columns(app).await;
    let Ok(u) = normalize_username(username) else { return Ok(None) };
    let id = discord_id.trim();
    if !id.is_empty() && !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Discord User ID must be numbers only.".into());
    }
    let id = (!id.is_empty()).then(|| id.to_string());
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET discord_user_id = $1, discord_username = NULL, discord_dm_verified_at = NULL, discord_dm_verification_code_hash = NULL, discord_dm_verification_sent_at = NULL WHERE username = $2 AND guild_id = $3",
            &[id.into(), u.clone().into(), guild_id.into()],
        )
        .await;
    Ok(get_account_profile(app, &u, guild_id).await)
}

pub async fn issue_discord_dm_code(app: &AppRef, username: &str, guild_id: &str, code: &str) -> Option<Value> {
    ensure_dm_columns(app).await;
    let u = normalize_username(username).ok()?;
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET discord_dm_verification_code_hash = $1, discord_dm_verification_sent_at = CURRENT_TIMESTAMP WHERE username = $2 AND guild_id = $3 AND discord_user_id IS NOT NULL AND discord_user_id != '' AND discord_dm_verified_at IS NULL",
            &[auth::sha256_hex(code).into(), u.clone().into(), guild_id.into()],
        )
        .await;
    get_account_profile(app, &u, guild_id).await
}

pub async fn verify_discord_dm_code(app: &AppRef, username: &str, guild_id: &str, code: &str, max_age: i64, discord_username: Option<String>) -> Option<Value> {
    ensure_dm_columns(app).await;
    let u = normalize_username(username).ok()?;
    let row = app
        .db
        .opt(
            DB,
            "SELECT username, guild_id FROM users WHERE username = $1 AND guild_id = $2 AND discord_user_id IS NOT NULL AND discord_user_id != '' AND discord_dm_verification_code_hash = $3 \
             AND discord_dm_verification_sent_at IS NOT NULL AND discord_dm_verification_sent_at >= NOW() - make_interval(secs => $4::bigint) LIMIT 1",
            &[u.into(), guild_id.into(), auth::sha256_hex(code).into(), max_age.max(1).into()],
        )
        .await?;
    let (ru, rg) = (row.s("username").unwrap_or_default(), row.s("guild_id").unwrap_or_default());
    let _ = app
        .db
        .exec(
            DB,
            "UPDATE users SET discord_dm_verified_at = CURRENT_TIMESTAMP, discord_username = COALESCE($1, discord_username), discord_dm_verification_code_hash = NULL, discord_dm_verification_sent_at = NULL WHERE username = $2 AND guild_id = $3",
            &[discord_username.into(), ru.clone().into(), rg.clone().into()],
        )
        .await;
    get_account_profile(app, &ru, &rg).await
}

pub async fn update_email(app: &AppRef, username: &str, guild_id: &str, email: &str) -> Result<Option<Value>, String> {
    let Ok(u) = normalize_username(username) else { return Ok(None) };
    let e = normalize_email(email)?;
    let Some(cur) = app.db.opt(DB, "SELECT email FROM users WHERE username = $1 AND guild_id = $2 LIMIT 1", &[u.clone().into(), guild_id.into()]).await else { return Ok(None) };
    if cur.s("email") == e {
        return Ok(get_account_profile(app, &u, guild_id).await);
    }
    app.db
        .exec(
            DB,
            "UPDATE users SET email = $1, email_verified_at = NULL, email_verification_token_hash = NULL, email_verification_code_hash = NULL, email_verification_sent_at = NULL WHERE username = $2 AND guild_id = $3",
            &[e.into(), u.clone().into(), guild_id.into()],
        )
        .await
        .map_err(|err| if err.to_lowercase().contains("duplicate") || err.to_lowercase().contains("unique") { "That email is already registered.".to_string() } else { err })?;
    Ok(get_account_profile(app, &u, guild_id).await)
}
