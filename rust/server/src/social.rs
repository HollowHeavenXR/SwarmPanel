//! Follows, friends, messages, notifications (port of social.lua).

use crate::accounts;
use crate::app::AppRef;
use crate::db::{J, P};
use serde_json::{Value, json};
use std::collections::HashMap;

const DB: &str = "accountlogins";
const STATUS_ORDER: &str = "CASE status WHEN 'accepted' THEN 0 WHEN 'pending' THEN 1 WHEN 'declined' THEN 2 ELSE 3 END";

pub fn serialize_row(mut r: Value) -> Value {
    if r.get("last_seen_at").is_some() {
        r["is_online"] = json!(accounts::is_recently_seen(r.get("last_seen_at").and_then(Value::as_str), 180));
    }
    if r.get("public_profile").is_some() {
        r["public_profile"] = json!(r.b("public_profile"));
    }
    if r.get("unread_count").is_some() {
        r["unread_count"] = json!(r.i("unread_count").unwrap_or(0));
    }
    if let Some(g) = r.s("guild_id") {
        r["guild_id"] = json!(g);
    }
    r
}

fn status_for(viewer: i64, requester: i64, status: &str) -> Option<&'static str> {
    match status {
        "accepted" => Some("friends"),
        "pending" => Some(if requester == viewer { "pending_out" } else { "pending_in" }),
        _ => None,
    }
}

pub async fn get_account_social_snapshot(app: &AppRef, target: i64, viewer: i64) -> Value {
    let r = app
        .db
        .opt(
            DB,
            "SELECT (SELECT COUNT(*) FROM account_follows WHERE followed_account_id=$1) AS follower_count, (SELECT COUNT(*) FROM account_follows WHERE follower_account_id=$1) AS following_count, \
             (SELECT COUNT(*) FROM account_friend_requests WHERE status='accepted' AND (requester_account_id=$1 OR addressee_account_id=$1)) AS friend_count, \
             EXISTS(SELECT 1 FROM account_follows WHERE follower_account_id=$2 AND followed_account_id=$1) AS followed_by_me",
            &[target.into(), viewer.into()],
        )
        .await
        .unwrap_or_else(|| json!({}));
    let mut fs = "none";
    if viewer != 0 && viewer == target {
        fs = "self";
    } else if viewer != 0 {
        if let Some(fr) = app
            .db
            .opt(DB, &format!("SELECT status, requester_account_id FROM account_friend_requests WHERE (requester_account_id=$1 AND addressee_account_id=$2) OR (requester_account_id=$2 AND addressee_account_id=$1) ORDER BY {STATUS_ORDER}, created_at DESC LIMIT 1"), &[viewer.into(), target.into()])
            .await
        {
            fs = status_for(viewer, fr.i("requester_account_id").unwrap_or(0), &fr.s("status").unwrap_or_default()).unwrap_or("none");
        }
    }
    json!({
        "follower_count": r.i("follower_count").unwrap_or(0), "following_count": r.i("following_count").unwrap_or(0),
        "friend_count": r.i("friend_count").unwrap_or(0), "followed_by_me": r.b("followed_by_me"), "friend_status": fs,
    })
}

pub async fn get_account_social_snapshots(app: &AppRef, ids: &[i64], viewer: i64) -> HashMap<i64, Value> {
    let mut uniq: Vec<i64> = Vec::new();
    for i in ids {
        if !uniq.contains(i) {
            uniq.push(*i);
        }
    }
    let mut out: HashMap<i64, Value> = uniq
        .iter()
        .map(|id| (*id, json!({ "follower_count": 0, "following_count": 0, "friend_count": 0, "followed_by_me": false, "friend_status": if viewer != 0 && viewer == *id { "self" } else { "none" } })))
        .collect();
    if uniq.is_empty() {
        return out;
    }
    let values = uniq.iter().map(|i| format!("({i}::int)")).collect::<Vec<_>>().join(", ");
    let list = uniq.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
    for r in app
        .db
        .all(
            DB,
            &format!(
                "SELECT t.id AS account_id, (SELECT COUNT(*) FROM account_follows WHERE followed_account_id=t.id) AS follower_count, (SELECT COUNT(*) FROM account_follows WHERE follower_account_id=t.id) AS following_count, \
                 (SELECT COUNT(*) FROM account_friend_requests WHERE status='accepted' AND (requester_account_id=t.id OR addressee_account_id=t.id)) AS friend_count, \
                 EXISTS(SELECT 1 FROM account_follows WHERE follower_account_id=$1 AND followed_account_id=t.id) AS followed_by_me FROM (VALUES {values}) AS t(id)"
            ),
            &[viewer.into()],
        )
        .await
    {
        if let Some(s) = r.i("account_id").and_then(|id| out.get_mut(&id)) {
            for k in ["follower_count", "following_count", "friend_count"] {
                s[k] = json!(r.i(k).unwrap_or(0));
            }
            s["followed_by_me"] = json!(r.b("followed_by_me"));
        }
    }
    if viewer != 0 {
        let rows = app
            .db
            .all(
                DB,
                &format!("SELECT status, requester_account_id, addressee_account_id FROM account_friend_requests WHERE (requester_account_id=$1 AND addressee_account_id IN ({list})) OR (addressee_account_id=$1 AND requester_account_id IN ({list})) ORDER BY {STATUS_ORDER}, created_at DESC"),
                &[viewer.into()],
            )
            .await;
        let mut decided = std::collections::HashSet::new();
        for r in rows {
            let req = r.i("requester_account_id").unwrap_or(0);
            let other = if req == viewer { r.i("addressee_account_id").unwrap_or(0) } else { req };
            if let Some(s) = out.get_mut(&other) {
                if s["friend_status"] != "self" && decided.insert(other) {
                    if let Some(st) = status_for(viewer, req, &r.s("status").unwrap_or_default()) {
                        s["friend_status"] = json!(st);
                    }
                }
            }
        }
    }
    out
}

pub async fn get_public_account_profile(app: &AppRef, id: i64, viewer: i64) -> Option<Value> {
    let mut p = accounts::get_account_by_id(app, id).await?;
    if !p.b("public_profile") && p.i("id") != Some(viewer) {
        return None;
    }
    if let Value::Object(s) = get_account_social_snapshot(app, id, viewer).await {
        for (k, v) in s {
            p[&k] = v;
        }
    }
    let g = p.s("guild_id").unwrap_or_default();
    let sums = crate::dashboard::music_activity_for_guilds(app, std::slice::from_ref(&g)).await;
    p["activity"] = sums.get(&g).cloned().unwrap_or_else(crate::dashboard::empty_activity);
    Some(p)
}

pub async fn set_follow(app: &AppRef, follower: i64, followed: i64, following: bool) -> Result<Value, String> {
    if follower == followed {
        return Err("You cannot follow yourself.".into());
    }
    if accounts::get_account_by_id(app, followed).await.is_none() {
        return Err("Account not found.".into());
    }
    if following {
        app.db.exec(DB, "INSERT INTO account_follows (follower_account_id, followed_account_id, created_at) VALUES ($1, $2, CURRENT_TIMESTAMP) ON CONFLICT (follower_account_id, followed_account_id) DO NOTHING", &[follower.into(), followed.into()]).await?;
    } else {
        app.db.exec(DB, "DELETE FROM account_follows WHERE follower_account_id=$1 AND followed_account_id=$2", &[follower.into(), followed.into()]).await?;
    }
    let n = app.db.opt(DB, "SELECT COUNT(*) AS followers FROM account_follows WHERE followed_account_id=$1", &[followed.into()]).await.and_then(|r| r.i("followers")).unwrap_or(0);
    Ok(json!({ "account_id": followed, "following": following, "follower_count": n }))
}

pub async fn send_friend_request(app: &AppRef, requester: i64, addressee: i64) -> Result<Value, String> {
    if requester == addressee {
        return Err("You cannot friend yourself.".into());
    }
    if accounts::get_account_by_id(app, addressee).await.is_none() {
        return Err("Account not found.".into());
    }
    let existing = app
        .db
        .opt(DB, &format!("SELECT * FROM account_friend_requests WHERE (requester_account_id=$1 AND addressee_account_id=$2) OR (requester_account_id=$2 AND addressee_account_id=$1) ORDER BY {STATUS_ORDER}, created_at DESC LIMIT 1"), &[requester.into(), addressee.into()])
        .await;
    let mut request_id = None;
    if let Some(mut e) = existing {
        match e.s("status").as_deref() {
            Some("accepted") => return Ok(json!({ "status": "friends", "request": serialize_row(e) })),
            Some("pending") => {
                if e.i("requester_account_id") == Some(addressee) {
                    app.db.exec(DB, "UPDATE account_friend_requests SET status='accepted', responded_at=CURRENT_TIMESTAMP WHERE id=$1", &[e.i("id").into()]).await?;
                    e["status"] = json!("accepted");
                    return Ok(json!({ "status": "friends", "request": serialize_row(e) }));
                }
                return Ok(json!({ "status": "pending_out", "request": serialize_row(e) }));
            }
            _ => {
                request_id = e.i("id");
                app.db
                    .exec(DB, "UPDATE account_friend_requests SET requester_account_id=$1, addressee_account_id=$2, status='pending', created_at=CURRENT_TIMESTAMP, responded_at=NULL WHERE id=$3", &[requester.into(), addressee.into(), request_id.into()])
                    .await?;
            }
        }
    } else {
        app.db.exec(DB, "INSERT INTO account_friend_requests (requester_account_id, addressee_account_id, status, created_at) VALUES ($1, $2, 'pending', CURRENT_TIMESTAMP)", &[requester.into(), addressee.into()]).await?;
    }
    let row = app
        .db
        .opt(DB, "SELECT * FROM account_friend_requests WHERE ($1::int IS NULL OR id=$1) AND requester_account_id=$2 AND addressee_account_id=$3 ORDER BY id DESC LIMIT 1", &[request_id.into(), requester.into(), addressee.into()])
        .await
        .unwrap_or_else(|| json!({}));
    Ok(json!({ "status": "pending_out", "request": serialize_row(row) }))
}

pub async fn list_friend_requests(app: &AppRef, account: i64, outgoing: bool) -> Vec<Value> {
    let (own, other) = if outgoing { ("requester_account_id", "addressee_account_id") } else { ("addressee_account_id", "requester_account_id") };
    app.db
        .all(
            DB,
            &format!("SELECT fr.*, u.id AS user_id, u.username, u.guild_id, u.display_name, u.avatar_url, u.server_name, u.server_icon_url, u.public_profile, u.last_seen_at FROM account_friend_requests fr JOIN users u ON u.id = fr.{other} WHERE fr.{own}=$1 AND fr.status='pending' ORDER BY fr.created_at DESC LIMIT 100"),
            &[account.into()],
        )
        .await
        .into_iter()
        .map(serialize_row)
        .collect()
}

pub async fn respond_friend_request(app: &AppRef, account: i64, request: i64, action: &str) -> Result<Option<Value>, String> {
    let status = match action.to_lowercase().as_str() {
        "accept" => "accepted",
        "decline" => "declined",
        "cancel" => "cancelled",
        _ => return Err("Action must be accept, decline, or cancel.".into()),
    };
    let col = if status == "cancelled" { "requester_account_id" } else { "addressee_account_id" };
    let Some(mut row) = app.db.opt(DB, &format!("SELECT * FROM account_friend_requests WHERE id=$1 AND {col}=$2 AND status='pending'"), &[request.into(), account.into()]).await else { return Ok(None) };
    app.db.exec(DB, "UPDATE account_friend_requests SET status=$1, responded_at=CURRENT_TIMESTAMP WHERE id=$2", &[status.into(), request.into()]).await?;
    row["status"] = json!(status);
    Ok(Some(serialize_row(row)))
}

pub async fn list_friends(app: &AppRef, account: i64) -> Vec<Value> {
    app.db
        .all(
            DB,
            "SELECT u.id, u.username, u.guild_id, u.display_name, u.avatar_url, u.bio, u.profile_headline, u.profile_tags, u.profile_links, u.profile_banner_url, u.profile_banner_mode, u.profile_card_style, \
             u.profile_social_mode, u.server_name, u.server_icon_url, u.public_profile, u.last_seen_at, fr.responded_at AS friended_at FROM account_friend_requests fr \
             JOIN users u ON u.id = CASE WHEN fr.requester_account_id=$1 THEN fr.addressee_account_id ELSE fr.requester_account_id END \
             WHERE fr.status='accepted' AND (fr.requester_account_id=$1 OR fr.addressee_account_id=$1) ORDER BY fr.responded_at DESC, fr.created_at DESC LIMIT 200",
            &[account.into()],
        )
        .await
        .into_iter()
        .map(accounts::serialize_profile)
        .collect()
}

pub async fn send_message(app: &AppRef, sender: i64, recipient: i64, body: &str) -> Result<Value, String> {
    if sender == recipient {
        return Err("You cannot message yourself.".into());
    }
    let cleaned = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return Err("Message cannot be empty.".into());
    }
    if cleaned.len() > 2000 {
        return Err("Message must be 2000 characters or fewer.".into());
    }
    if accounts::get_account_by_id(app, recipient).await.is_none() {
        return Err("Account not found.".into());
    }
    let ins = app.db.one(DB, "INSERT INTO account_messages (sender_account_id, recipient_account_id, body, created_at) VALUES ($1, $2, $3, CURRENT_TIMESTAMP) RETURNING id", &[sender.into(), recipient.into(), cleaned.into()]).await?.ok_or("insert failed")?;
    let msg = app
        .db
        .opt(DB, "SELECT msg.*, u.username, u.guild_id, u.display_name, u.avatar_url, u.server_name, u.server_icon_url, u.public_profile, u.last_seen_at FROM account_messages msg JOIN users u ON u.id = msg.sender_account_id WHERE msg.id = $1 LIMIT 1", &[ins.i("id").into()])
        .await
        .unwrap_or_else(|| json!({}));
    let mut item = serialize_row(msg);
    item["mine"] = json!(true);
    Ok(item)
}

pub async fn list_threads(app: &AppRef, account: i64) -> Vec<Value> {
    app.db
        .all(
            DB,
            "SELECT other_user.id, other_user.id AS account_id, other_user.username, other_user.guild_id, other_user.display_name, other_user.avatar_url, other_user.server_name, other_user.server_icon_url, \
             other_user.public_profile, other_user.last_seen_at, latest.id AS last_message_id, latest.body AS last_message, latest.created_at AS last_message_at, \
             latest.sender_account_id AS last_sender_account_id, unread.unread_count FROM ( \
               SELECT CASE WHEN sender_account_id=$1 THEN recipient_account_id ELSE sender_account_id END AS other_id, MAX(id) AS last_id \
               FROM account_messages WHERE sender_account_id=$1 OR recipient_account_id=$1 GROUP BY other_id) threads \
             JOIN account_messages latest ON latest.id = threads.last_id JOIN users other_user ON other_user.id = threads.other_id \
             LEFT JOIN (SELECT sender_account_id AS other_id, COUNT(*) AS unread_count FROM account_messages WHERE recipient_account_id=$1 AND read_at IS NULL GROUP BY sender_account_id) unread ON unread.other_id = threads.other_id \
             ORDER BY latest.created_at DESC LIMIT 100",
            &[account.into()],
        )
        .await
        .into_iter()
        .map(serialize_row)
        .collect()
}

pub async fn list_messages(app: &AppRef, account: i64, other: i64, limit: i64) -> Result<Vec<Value>, String> {
    if account == other {
        return Err("Pick another account to view messages.".into());
    }
    if accounts::get_account_by_id(app, other).await.is_none() {
        return Err("Account not found.".into());
    }
    if app.db.opt(DB, "SELECT 1 AS present FROM account_messages WHERE sender_account_id=$1 AND recipient_account_id=$2 AND read_at IS NULL LIMIT 1", &[other.into(), account.into()]).await.is_some() {
        let _ = app.db.exec(DB, "UPDATE account_messages SET read_at=COALESCE(read_at, CURRENT_TIMESTAMP) WHERE sender_account_id=$1 AND recipient_account_id=$2 AND read_at IS NULL", &[other.into(), account.into()]).await;
    }
    let rows = app
        .db
        .query(
            DB,
            "SELECT msg.*, u.username, u.guild_id, u.display_name, u.avatar_url, u.server_name, u.server_icon_url, u.public_profile, u.last_seen_at FROM account_messages msg JOIN users u ON u.id = msg.sender_account_id \
             WHERE (msg.sender_account_id=$1 AND msg.recipient_account_id=$2) OR (msg.sender_account_id=$2 AND msg.recipient_account_id=$1) ORDER BY msg.created_at DESC, msg.id DESC LIMIT $3",
            &[account.into(), other.into(), limit.clamp(1, 200).into()],
        )
        .await?;
    Ok(rows
        .into_iter()
        .rev()
        .map(|r| {
            let mut i = serialize_row(r);
            i["mine"] = json!(i.i("sender_account_id") == Some(account));
            i
        })
        .collect())
}

pub async fn create_notification(app: &AppRef, recipient: i64, kind: &str, title: &str, body: Option<&str>, link: Option<&str>) {
    let _ = app
        .db
        .exec(
            DB,
            "INSERT INTO account_notifications (recipient_account_id, kind, title, body, link_path, created_at) VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP)",
            &[recipient.into(), crate::util::cut(kind, 40).into(), crate::util::cut(title, 200).into(), body.map(|b| crate::util::cut(b, 500)).into(), link.map(|l| crate::util::cut(l, 200)).into()],
        )
        .await;
}

pub async fn list_notifications(app: &AppRef, account: i64, limit: i64) -> Vec<Value> {
    app.db
        .all(DB, "SELECT id, kind, title, body, link_path, read_at, created_at FROM account_notifications WHERE recipient_account_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2", &[account.into(), limit.clamp(1, 100).into()])
        .await
        .into_iter()
        .map(serialize_row)
        .collect()
}

pub async fn community_counts(app: &AppRef, account: i64) -> Value {
    let r = app
        .db
        .opt(
            DB,
            "SELECT (SELECT COUNT(*) FROM account_messages WHERE recipient_account_id = $1 AND read_at IS NULL) AS unread_messages, (SELECT COUNT(*) FROM account_friend_requests WHERE addressee_account_id = $1 AND status = 'pending') AS pending_requests",
            &[account.into()],
        )
        .await
        .unwrap_or_else(|| json!({}));
    json!({ "unread_messages": r.i("unread_messages").unwrap_or(0), "pending_friend_requests": r.i("pending_requests").unwrap_or(0) })
}

pub async fn ensure_indexes(app: &AppRef) {
    for s in [
        "CREATE INDEX IF NOT EXISTS account_messages_recipient_unread_idx ON account_messages (recipient_account_id, sender_account_id) WHERE read_at IS NULL",
        "CREATE INDEX IF NOT EXISTS account_messages_pair_idx ON account_messages (sender_account_id, recipient_account_id, created_at DESC, id DESC)",
        "CREATE INDEX IF NOT EXISTS account_messages_recipient_pair_idx ON account_messages (recipient_account_id, sender_account_id, id)",
        "CREATE INDEX IF NOT EXISTS account_friend_requests_addressee_status_idx ON account_friend_requests (addressee_account_id, status)",
        "CREATE INDEX IF NOT EXISTS account_friend_requests_requester_status_idx ON account_friend_requests (requester_account_id, status)",
        "CREATE INDEX IF NOT EXISTS account_follows_followed_idx ON account_follows (followed_account_id)",
        "CREATE INDEX IF NOT EXISTS account_notifications_recipient_created_idx ON account_notifications (recipient_account_id, created_at DESC, id DESC)",
        "CREATE INDEX IF NOT EXISTS account_notifications_recipient_unread_idx ON account_notifications (recipient_account_id) WHERE read_at IS NULL",
        "CREATE INDEX IF NOT EXISTS users_last_seen_idx ON users (last_seen_at DESC)",
    ] {
        let _ = app.db.batch(DB, s).await;
    }
}

pub async fn unread_notification_count(app: &AppRef, account: i64) -> i64 {
    app.db.opt(DB, "SELECT COUNT(*) AS unread FROM account_notifications WHERE recipient_account_id = $1 AND read_at IS NULL", &[account.into()]).await.and_then(|r| r.i("unread")).unwrap_or(0)
}

pub async fn mark_notification_read(app: &AppRef, id: i64, account: i64) {
    let _ = app.db.exec(DB, "UPDATE account_notifications SET read_at = COALESCE(read_at, CURRENT_TIMESTAMP) WHERE id = $1 AND recipient_account_id = $2", &[id.into(), account.into()]).await;
}

pub async fn mark_all_notifications_read(app: &AppRef, account: i64) {
    let _ = app.db.exec(DB, "UPDATE account_notifications SET read_at = COALESCE(read_at, CURRENT_TIMESTAMP) WHERE recipient_account_id = $1 AND read_at IS NULL", &[account.into()]).await;
}

/// The friend-status-based message/follow/friend rules (routes.lua social_permissions).
pub fn social_permissions(p: &Value) -> Value {
    let status = p.s("friend_status").unwrap_or_else(|| "none".into()).to_lowercase();
    let mut mode = p.s("profile_social_mode").unwrap_or_else(|| "open".into()).to_lowercase();
    if !["open", "friends", "quiet"].contains(&mode.as_str()) {
        mode = "open".into();
    }
    let public = p.b("public_profile");
    let friend = status == "friends";
    let can = |action: &str| -> bool {
        if status == "self" {
            return true;
        }
        if !public {
            return friend && (action == "message" || action == "view_friends");
        }
        match mode.as_str() {
            "quiet" => false,
            "friends" => {
                if action == "friend_request" {
                    !matches!(status.as_str(), "friends" | "pending_out" | "pending_in" | "self")
                } else {
                    friend
                }
            }
            _ => true,
        }
    };
    json!({
        "can_follow": can("follow") && status != "self" && status != "pending_out",
        "can_friend": can("friend_request"),
        "can_message": can("message") && status != "self",
        "can_view_friends": can("view_friends"),
    })
}

pub fn _p(_: P) {}
