//! Alert rules (port of alerts.lua).

use crate::app::AppRef;
use crate::db::{J, P};
use serde_json::{Map, Value, json};

const DB: &str = "accountlogins";
const COLS: &str = "id, rule_type, threshold_minutes, enabled, escalation_minutes, escalate_email, created_at";
const TYPES: &[&str] = &["bot_offline", "queue_stuck", "stale_metrics", "recovery_pending"];

fn out(mut r: Value) -> Value {
    r["enabled"] = json!(r.b("enabled"));
    r["escalate_email"] = json!(r.b("escalate_email"));
    r
}

pub async fn list(app: &AppRef) -> Result<Vec<Value>, String> {
    Ok(app.db.query(DB, &format!("SELECT {COLS} FROM swarm_alert_rules ORDER BY created_at DESC, id DESC"), &[]).await?.into_iter().map(out).collect())
}
pub async fn list_enabled(app: &AppRef) -> Vec<Value> {
    app.db.all(DB, &format!("SELECT {COLS} FROM swarm_alert_rules WHERE enabled = TRUE"), &[]).await.into_iter().map(out).collect()
}
pub async fn get(app: &AppRef, id: &str) -> Option<Value> {
    app.db.opt(DB, &format!("SELECT {COLS} FROM swarm_alert_rules WHERE id = $1"), &[id.into()]).await.map(out)
}

fn rule_type(v: &Value) -> Result<String, String> {
    let t = v.as_str().map(str::to_string).unwrap_or_default().trim().to_lowercase();
    if TYPES.contains(&t.as_str()) {
        Ok(t)
    } else {
        let mut n = TYPES.to_vec();
        n.sort();
        Err(format!("Unsupported rule_type: '{t}'. Expected one of: {}", n.join(", ")))
    }
}
fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}
fn truthy(v: &Value) -> bool {
    v.as_bool().unwrap_or(!v.is_null())
}

pub async fn create(app: &AppRef, body: &Value) -> Result<Value, String> {
    let t = rule_type(body.get("rule_type").unwrap_or(&Value::Null))?;
    let thr = body.get("threshold_minutes").and_then(num).unwrap_or(5.0).clamp(1.0, 1440.0) as i64;
    let esc = body.get("escalation_minutes").and_then(num).filter(|n| *n > 0.0).map(|n| n.clamp(1.0, 10080.0) as i64);
    let enabled = body.get("enabled").is_none_or(truthy);
    let r = app
        .db
        .one(
            DB,
            &format!("INSERT INTO swarm_alert_rules (rule_type, threshold_minutes, enabled, escalation_minutes, escalate_email, created_at) VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP) RETURNING {COLS}"),
            &[t.into(), thr.into(), enabled.into(), esc.into(), body.get("escalate_email").is_some_and(truthy).into()],
        )
        .await?;
    Ok(r.map(out).unwrap_or_else(|| json!({})))
}

pub async fn update(app: &AppRef, id: &str, u: &Map<String, Value>) -> Result<Option<Value>, String> {
    let allowed = ["rule_type", "threshold_minutes", "enabled", "escalation_minutes", "escalate_email"];
    let mut unknown: Vec<&String> = u.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!("Unsupported alert rule fields: {}", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let mut sets = Vec::new();
    let mut params: Vec<P> = Vec::new();
    let mut push = |col: &str, v: P| {
        params.push(v);
        sets.push(format!("{col} = ${}", params.len()));
    };
    if let Some(v) = u.get("rule_type") {
        push("rule_type", rule_type(v)?.into());
    }
    if let Some(v) = u.get("threshold_minutes") {
        push("threshold_minutes", (num(v).unwrap_or(5.0).clamp(1.0, 1440.0) as i64).into());
    }
    if let Some(v) = u.get("enabled") {
        push("enabled", truthy(v).into());
    }
    if let Some(v) = u.get("escalate_email") {
        push("escalate_email", truthy(v).into());
    }
    if let Some(v) = u.get("escalation_minutes") {
        match num(v).filter(|n| *n > 0.0) {
            Some(n) => push("escalation_minutes", (n.clamp(1.0, 10080.0) as i64).into()),
            None => sets.push("escalation_minutes = NULL".into()),
        }
    }
    if !sets.is_empty() {
        params.push(id.into());
        app.db.exec(DB, &format!("UPDATE swarm_alert_rules SET {} WHERE id = ${}", sets.join(", "), params.len()), &params).await?;
    }
    Ok(get(app, id).await)
}

pub async fn delete(app: &AppRef, id: &str) {
    let _ = app.db.exec(DB, "DELETE FROM swarm_alert_rules WHERE id = $1", &[id.into()]).await;
}
