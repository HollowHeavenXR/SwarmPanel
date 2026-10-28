//! Saved queues (port of queues.lua).

use crate::app::AppRef;
use crate::db::{J, P};
use serde_json::{Value, json};

const DB: &str = "accountlogins";
const COLS: &str = "id, guild_id, bot_key, name, created_by_username, items, created_at";

fn out(mut r: Value) -> Value {
    if let Some(g) = r.s("guild_id") {
        r["guild_id"] = json!(g);
    }
    r["items"] = match r.get("items").cloned() {
        Some(Value::String(s)) => serde_json::from_str::<Value>(&s).ok().filter(|v| v.is_array() || v.is_object()).unwrap_or_else(|| json!([])),
        Some(v @ Value::Array(_)) => v,
        _ => json!([]),
    };
    r
}

pub async fn list(app: &AppRef, gid: &str, bot_key: Option<&str>) -> Result<Vec<Value>, String> {
    let mut params: Vec<P> = vec![gid.into()];
    let mut where_ = "WHERE guild_id = $1".to_string();
    if let Some(b) = bot_key.filter(|b| !b.is_empty()) {
        params.push(b.trim().to_lowercase().into());
        where_.push_str(" AND bot_key = $2");
    }
    Ok(app.db.query(DB, &format!("SELECT {COLS} FROM swarm_saved_queues {where_} ORDER BY created_at DESC, id DESC"), &params).await?.into_iter().map(out).collect())
}

pub async fn create(app: &AppRef, gid: &str, bot_key: &str, name: &str, by: &str, items: &Value) -> Result<Value, String> {
    let mut name = crate::util::cut(name.trim(), 120);
    if name.is_empty() {
        name = "Saved Queue".into();
    }
    let mut safe = Vec::new();
    for it in items.as_array().cloned().unwrap_or_default() {
        let url = it.s("video_url").unwrap_or_default().trim().to_string();
        if url.is_empty() {
            continue;
        }
        let title = it.s("title").map(|t| crate::util::cut(&t, 400)).filter(|t| !t.is_empty());
        safe.push(json!({ "title": title, "video_url": crate::util::cut(&url, 1000), "requester_id": it.s("requester_id") }));
        if safe.len() >= 200 {
            break;
        }
    }
    if safe.is_empty() {
        return Err("A saved queue needs at least one track with a video_url.".into());
    }
    let r = app
        .db
        .one(
            DB,
            &format!("INSERT INTO swarm_saved_queues (guild_id, bot_key, name, created_by_username, items, created_at) VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP) RETURNING {COLS}"),
            &[gid.into(), bot_key.trim().to_lowercase().into(), name.into(), crate::util::cut(by, 80).into(), json!(safe).to_string().into()],
        )
        .await?;
    Ok(r.map(out).unwrap_or_else(|| json!({})))
}

pub async fn delete(app: &AppRef, id: &str, gid: &str) {
    let _ = app.db.exec(DB, "DELETE FROM swarm_saved_queues WHERE id = $1 AND guild_id = $2", &[id.into(), gid.into()]).await;
}

pub async fn rename(app: &AppRef, id: &str, gid: &str, name: &str) -> Result<Option<Value>, String> {
    let name = crate::util::cut(name.trim(), 120);
    if name.is_empty() {
        return Err("Name cannot be empty.".into());
    }
    app.db.exec(DB, "UPDATE swarm_saved_queues SET name = $1 WHERE id = $2 AND guild_id = $3", &[name.into(), id.into(), gid.into()]).await?;
    Ok(app.db.opt(DB, &format!("SELECT {COLS} FROM swarm_saved_queues WHERE id = $1"), &[id.into()]).await.map(out))
}
