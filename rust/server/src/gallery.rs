//! Image Gallery admin (port of gallery.lua) against the image_gallery database.

use crate::app::AppRef;
use crate::db::{J, P};
use serde_json::{Map, Value, json};

const DB: &str = "image_gallery";
const USER_COLS: &str = "id, username, display_name, email, email_verified_at, email_verification_sent_at, public_profile, show_liked_count, birthdate, age_verified_at, adult_content_consent, created_at, last_login_at";

pub async fn admin_data(app: &AppRef, limit: i64) -> Result<Value, String> {
    let lim = limit.clamp(1, 200);
    let db = &app.db;
    let count = |t: &'static str| async move { db.one(DB, &format!("SELECT COUNT(*) AS count FROM {t}"), &[]).await.map(|r| r.and_then(|r| r.i("count")).unwrap_or(0)) };
    let (u, m, c, col, rep) = tokio::join!(count("users"), count("media_items"), count("media_comments"), count("media_collections"), count("media_reports WHERE status='open'"));
    let summary = json!({ "schema": DB, "users": u?, "media": m?, "comments": c?, "collections": col?, "reports_open": rep? });
    let pl: [P; 1] = [lim.into()];
    let (users, comments, media, reports, categories, collections) = tokio::join!(
        db.query(DB, "SELECT u.id, u.username, u.display_name, u.email, u.email_verified_at, u.email_verification_sent_at, u.public_profile, u.show_liked_count, u.birthdate, u.age_verified_at, u.adult_content_consent, u.created_at, u.last_login_at, \
                      COUNT(DISTINCT m.id) AS media_count, COUNT(DISTINCT c.id) AS comment_count, COUNT(DISTINCT b.media_id) AS bookmark_count, COUNT(DISTINCT col.id) AS collection_count \
                      FROM users u LEFT JOIN media_items m ON m.user_id = u.id LEFT JOIN media_comments c ON c.user_id = u.id LEFT JOIN media_bookmarks b ON b.user_id = u.id LEFT JOIN media_collections col ON col.user_id = u.id \
                      GROUP BY u.id ORDER BY u.created_at DESC LIMIT $1", &pl),
        db.query(DB, "SELECT c.id, c.media_id, c.user_id, c.body, c.created_at, u.username, u.display_name, m.title AS media_title FROM media_comments c JOIN users u ON u.id = c.user_id JOIN media_items m ON m.id = c.media_id ORDER BY c.created_at DESC LIMIT $1", &pl),
        db.query(DB, "SELECT m.id, m.user_id, m.title, m.media_kind, m.file_size, m.views, m.downloads, m.is_adult, m.moderation_status, m.moderation_reason, m.created_at, u.username FROM media_items m JOIN users u ON u.id = m.user_id ORDER BY m.created_at DESC LIMIT $1", &pl),
        db.query(DB, "SELECT r.id, r.media_id, r.user_id, r.reason, r.details, r.status, r.created_at, u.username, u.display_name, m.title AS media_title FROM media_reports r JOIN users u ON u.id = r.user_id JOIN media_items m ON m.id = r.media_id \
                      ORDER BY CASE r.status WHEN 'open' THEN 0 WHEN 'reviewed' THEN 1 ELSE 2 END, r.created_at DESC LIMIT $1", &pl),
        db.query(DB, "SELECT c.id, c.name, c.slug, c.media_kind, c.created_at, COUNT(m.id) AS media_count FROM categories c LEFT JOIN media_items m ON m.category_id = c.id GROUP BY c.id ORDER BY c.name ASC", &[]),
        db.query(DB, "SELECT col.id, col.user_id, col.name, col.is_public, col.created_at, u.username, COUNT(ci.media_id) AS item_count FROM media_collections col JOIN users u ON u.id = col.user_id LEFT JOIN media_collection_items ci ON ci.collection_id = col.id GROUP BY col.id, u.username ORDER BY col.created_at DESC LIMIT $1", &pl),
    );
    Ok(json!({ "schema": DB, "summary": summary, "users": users?, "comments": comments?, "media": media?, "reports": reports?, "categories": categories?, "collections": collections? }))
}

pub async fn count_open_reports(app: &AppRef) -> Option<i64> {
    app.db.one(DB, "SELECT COUNT(*) AS count FROM media_reports WHERE status='open'", &[]).await.ok().map(|r| r.and_then(|r| r.i("count")).unwrap_or(0))
}

pub async fn user_admin(app: &AppRef, id: &P) -> Option<Value> {
    app.db.opt(DB, &format!("SELECT {USER_COLS} FROM users WHERE id = $1 LIMIT 1"), &[id.clone()]).await
}

pub async fn delete_user(app: &AppRef, id: &P) -> Result<u64, String> {
    app.db.exec(DB, "DELETE FROM users WHERE id = $1", &[id.clone()]).await
}
pub async fn delete_comment(app: &AppRef, id: &P) -> Result<u64, String> {
    app.db.exec(DB, "DELETE FROM media_comments WHERE id = $1", &[id.clone()]).await
}
pub async fn delete_media(app: &AppRef, id: &P) -> Result<u64, String> {
    app.db.exec(DB, "DELETE FROM media_items WHERE id = $1", &[id.clone()]).await
}

pub async fn reset_password(app: &AppRef, id: &P, pw: &str) -> Result<(), String> {
    if pw.len() < 8 {
        return Err("Password must be at least 8 characters.".into());
    }
    let h = crate::auth::password_hash_async(pw.to_string()).await;
    app.db.exec(DB, "UPDATE users SET password_hash = $1 WHERE id = $2", &[h.into(), id.clone()]).await.map(|_| ())
}

fn truthy(v: &Value) -> bool {
    v.as_bool().unwrap_or(!v.is_null())
}
fn text(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_else(|| if v.is_null() { String::new() } else { v.to_string() })
}

pub async fn update_user(app: &AppRef, id: &P, u: &Map<String, Value>) -> Result<Option<Value>, String> {
    let allowed = ["username", "display_name", "email", "public_profile", "show_liked_count", "adult_content_consent"];
    let mut unknown: Vec<&String> = u.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!("Unsupported user fields: {}", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<P> = Vec::new();
    let mut push = |c: &str, v: P| {
        params.push(v);
        sets.push(format!("{c} = ${}", params.len()));
    };
    if let Some(v) = u.get("username") {
        let n = crate::util::cut(text(v).trim(), 40);
        if n.is_empty() {
            return Err("Username cannot be empty.".into());
        }
        push("username", n.into());
    }
    if let Some(v) = u.get("display_name") {
        let d = crate::util::cut(text(v).trim(), 80);
        push("display_name", (!d.is_empty()).then_some(d).into());
    }
    let mut reset = false;
    if let Some(v) = u.get("email") {
        let e = crate::accounts::normalize_email(&text(v))?;
        reset = e.is_some();
        push("email", e.into());
    }
    for k in ["public_profile", "show_liked_count", "adult_content_consent"] {
        if let Some(v) = u.get(k) {
            push(k, truthy(v).into());
        }
    }
    if reset {
        sets.extend(["email_verified_at = NULL".into(), "email_verification_token_hash = NULL".into(), "email_verification_sent_at = NULL".into()]);
    }
    if !sets.is_empty() {
        params.push(id.clone());
        app.db.exec(DB, &format!("UPDATE users SET {} WHERE id = ${}", sets.join(", "), params.len()), &params).await?;
    }
    Ok(user_admin(app, id).await)
}

pub async fn set_email_verified(app: &AppRef, id: &P, v: bool) -> Option<Value> {
    let _ = app
        .db
        .exec(DB, "UPDATE users SET email_verified_at = CASE WHEN $1 THEN CURRENT_TIMESTAMP ELSE NULL END, email_verification_token_hash = CASE WHEN $1 THEN NULL ELSE email_verification_token_hash END WHERE id = $2", &[v.into(), id.clone()])
        .await;
    user_admin(app, id).await
}

pub async fn set_age_verified(app: &AppRef, id: &P, v: bool) -> Option<Value> {
    let _ = app.db.exec(DB, "UPDATE users SET age_verified_at = CASE WHEN $1 THEN CURRENT_TIMESTAMP ELSE NULL END, adult_content_consent = $1 WHERE id = $2", &[v.into(), id.clone()]).await;
    user_admin(app, id).await
}

pub async fn issue_email_token(app: &AppRef, id: &P, hash: &str) {
    let _ = app
        .db
        .exec(DB, "UPDATE users SET email_verification_token_hash = $1, email_verification_sent_at = CURRENT_TIMESTAMP WHERE id = $2 AND email IS NOT NULL AND email_verified_at IS NULL", &[hash.into(), id.clone()])
        .await;
}

pub async fn update_media(app: &AppRef, id: &P, u: &Map<String, Value>) -> Result<Option<Value>, String> {
    let allowed = ["title", "is_adult", "moderation_status", "moderation_reason"];
    let mut unknown: Vec<&String> = u.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!("Unsupported media fields: {}", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<P> = Vec::new();
    let mut push = |c: &str, v: P| {
        params.push(v);
        sets.push(format!("{c} = ${}", params.len()));
    };
    if let Some(v) = u.get("title") {
        let t = crate::util::cut(text(v).trim(), 160);
        if t.is_empty() {
            return Err("Media title cannot be empty.".into());
        }
        push("title", t.into());
    }
    if let Some(v) = u.get("is_adult") {
        push("is_adult", truthy(v).into());
        push("adult_marked_by_user", truthy(v).into());
    }
    if let Some(v) = u.get("moderation_status") {
        let s = text(v).trim().to_lowercase();
        if !["clear", "review", "blocked"].contains(&s.as_str()) {
            return Err("Moderation status must be clear, review, or blocked.".into());
        }
        push("moderation_status", s.into());
    }
    if let Some(v) = u.get("moderation_reason") {
        let r = crate::util::cut(text(v).trim(), 300);
        push("moderation_reason", (!r.is_empty()).then_some(r).into());
    }
    if !sets.is_empty() {
        params.push(id.clone());
        app.db.exec(DB, &format!("UPDATE media_items SET {}, moderated_at = CURRENT_TIMESTAMP WHERE id = ${}", sets.join(", "), params.len()), &params).await?;
    }
    Ok(app.db.opt(DB, "SELECT * FROM media_items WHERE id = $1", &[id.clone()]).await)
}

pub async fn update_report_status(app: &AppRef, id: &P, status: &str) -> Result<(), String> {
    let s = status.trim().to_lowercase();
    if !["open", "reviewed", "dismissed"].contains(&s.as_str()) {
        return Err("Report status must be open, reviewed, or dismissed.".into());
    }
    app.db.exec(DB, "UPDATE media_reports SET status = $1 WHERE id = $2", &[s.into(), id.clone()]).await.map(|_| ())
}
