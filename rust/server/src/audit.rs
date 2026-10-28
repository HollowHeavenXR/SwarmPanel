//! Admin audit trail (port of audit.lua).

use crate::app::AppRef;
use crate::db::{J, P};
use crate::telemetry::Event;
use serde_json::{Map, Value, json};

const DB: &str = "accountlogins";

pub fn diff_details(before: &Value, after: &Value) -> String {
    let (mut b, mut a) = (Map::new(), Map::new());
    if let Some(after) = after.as_object() {
        for (k, nv) in after {
            if matches!(k.as_str(), "updated_at" | "is_online" | "last_seen_at") {
                continue;
            }
            let ov = before.get(k).cloned().unwrap_or(Value::Null);
            if &ov != nv {
                b.insert(k.clone(), ov);
                a.insert(k.clone(), nv.clone());
            }
        }
    }
    if a.is_empty() { String::new() } else { json!({ "before": b, "after": a }).to_string() }
}

#[derive(Default)]
pub struct Opts {
    pub actor_user_id: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub details: Option<String>,
}

pub async fn record(app: &AppRef, actor: &str, action: &str, o: Opts) {
    let cut = crate::util::cut;
    let _ = app
        .db
        .exec(
            DB,
            "INSERT INTO swarm_audit_log (actor_username, actor_user_id, action, target_type, target_id, details, created_at) VALUES ($1, $2, $3, $4, $5, $6, CURRENT_TIMESTAMP)",
            &[
                cut(actor, 80).into(),
                o.actor_user_id.clone().filter(|s| !s.is_empty()).into(),
                cut(action, 80).into(),
                o.target_type.as_deref().map(|t| cut(t, 40)).into(),
                o.target_id.as_deref().filter(|s| !s.is_empty()).map(|t| cut(t, 80)).into(),
                o.details.as_deref().map(|d| cut(d, 4000)).into(),
            ],
        )
        .await;
    app.telemetry.record(Event::new("admin", action).user(Some(actor)).meta(json!({ "target_type": o.target_type, "target_id": o.target_id, "details": o.details })));
}

/// Shorthand: target + JSON details.
pub async fn log(app: &AppRef, actor: &str, action: &str, target_type: &str, target_id: impl ToString, details: Value) {
    record(app, actor, action, Opts { target_type: Some(target_type.into()), target_id: Some(target_id.to_string()), details: Some(details.to_string()), ..Default::default() }).await;
}

pub async fn list(app: &AppRef, limit: i64, offset: i64, action: Option<&str>) -> Value {
    let (lim, off) = (limit.clamp(1, 500), offset.max(0));
    let filter = action.filter(|a| !a.is_empty()).map(|a| crate::util::cut(a, 80));
    let (where_, mut params): (&str, Vec<P>) = match &filter {
        Some(a) => ("WHERE action = $1", vec![a.clone().into()]),
        None => ("", vec![]),
    };
    let n = params.len();
    params.push(lim.into());
    params.push(off.into());
    let rows = app
        .db
        .all(DB, &format!("SELECT id, actor_username, actor_user_id, action, target_type, target_id, details, created_at FROM swarm_audit_log {where_} ORDER BY created_at DESC, id DESC LIMIT ${} OFFSET ${}", n + 1, n + 2), &params)
        .await;
    let total = app.db.opt(DB, &format!("SELECT COUNT(*) AS count FROM swarm_audit_log {where_}"), &params[..n]).await.and_then(|r| r.i("count")).unwrap_or(0);
    json!({ "entries": rows, "total": total })
}

pub async fn count_recent(app: &AppRef, hours: i64) -> i64 {
    app.db.opt(DB, "SELECT COUNT(*) AS count FROM swarm_audit_log WHERE created_at >= NOW() - make_interval(hours => $1::int)", &[hours.clamp(1, 720).into()]).await.and_then(|r| r.i("count")).unwrap_or(0)
}

pub async fn get(app: &AppRef, id: i64) -> Option<Value> {
    app.db.opt(DB, "SELECT id, actor_username, actor_user_id, action, target_type, target_id, details, created_at FROM swarm_audit_log WHERE id = $1 LIMIT 1", &[id.into()]).await
}
