//! /api/swarm-accounts/* -- admin CRUD over panel accounts.

use crate::accounts;
use crate::audit::{self, Opts};
use crate::db::J;
use crate::http::{R, Reply, Req};
use serde_json::{Value, json};

fn account_id(req: &Req) -> R<i64> {
    req.ji("account_id").ok_or_else(|| Reply::err(400, "account_id is required"))
}

pub async fn resend_verification(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let app = &req.app;
    let id = account_id(&req)?;
    let acct = accounts::get_account_admin(app, id).await.ok_or_else(|| Reply::err(404, "SwarmPanel account not found"))?;
    if acct.s("verification_webhook_url").is_none_or(|u| u.is_empty()) {
        return Err(Reply::err(400, "This SwarmPanel account does not have a Discord verification webhook configured."));
    }
    if acct.b("webhook_verified") || acct.b("email_verified") {
        return Ok(Reply::ok(json!({ "ok": true, "verification_sent": false, "already_verified": true })).by(&a));
    }
    let code = crate::auth::verification_code();
    let sent = match accounts::issue_webhook_code_by_id(app, id, &code).await {
        Some(acct) => crate::notify::send_verification_webhook_code(app, &acct.s("verification_webhook_url").unwrap_or_default(), &code, &acct.s("username").unwrap_or_default(), &acct.s("guild_id").unwrap_or_default()).await,
        None => false,
    };
    audit::record(app, &a.username, "swarm_account_resend_verification", Opts { target_type: Some("account".into()), target_id: Some(id.to_string()), details: Some(format!("sent={sent}")), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": sent, "verification_sent": sent, "already_verified": false })).by(&a))
}

pub async fn admin_list(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let limit = req.q("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(100).clamp(1, 500);
    let q = crate::util::cut(&req.q("query").unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "), 120);
    Ok(Reply::ok(json!({ "ok": true, "data": accounts::get_account_admin_data(&req.app, &q, limit).await })).by(&a))
}

pub async fn update(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let id = account_id(&req)?;
    let mut updates = serde_json::Map::new();
    for k in ["username", "guild_id", "email", "display_name", "public_profile", "server_name"] {
        if let Some(v) = req.json.get(k).filter(|v| !v.is_null()) {
            updates.insert(k.into(), v.clone());
        }
    }
    let before = accounts::get_account_admin(&req.app, id).await;
    let acct = accounts::update_account_admin(&req.app, id, &updates).await.map_err(|e| Reply::err(400, e))?;
    let mut keys: Vec<&String> = updates.keys().collect();
    keys.sort();
    let details = match &before {
        Some(b) => audit::diff_details(b, &acct),
        None => keys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","),
    };
    audit::record(&req.app, &a.username, "swarm_account_update", Opts { target_type: Some("account".into()), target_id: Some(id.to_string()), details: Some(details).filter(|d| !d.is_empty()), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "account": acct })).by(&a))
}

pub async fn email_verified(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let acct = accounts::set_webhook_verified_admin(&req.app, account_id(&req)?, req.jb("verified")).await.ok_or_else(|| Reply::err(404, "SwarmPanel account not found"))?;
    Ok(Reply::ok(json!({ "ok": true, "account": acct })).by(&a))
}

pub async fn reset_password(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let id = account_id(&req)?;
    let acct = accounts::reset_password_admin(&req.app, id, &req.js("new_password").unwrap_or_default()).await.map_err(|e| Reply::err(400, e))?.ok_or_else(|| Reply::err(404, "SwarmPanel account not found"))?;
    audit::record(&req.app, &a.username, "swarm_account_reset_password", Opts { target_type: Some("account".into()), target_id: Some(id.to_string()), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "account": acct })).by(&a))
}

pub async fn moderator(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let id = account_id(&req)?;
    let m = req.jb("moderator");
    let acct = accounts::set_panel_role(&req.app, id, m).await.ok_or_else(|| Reply::err(404, "SwarmPanel account not found"))?;
    audit::record(&req.app, &a.username, "swarm_account_set_moderator", Opts { target_type: Some("account".into()), target_id: Some(id.to_string()), details: Some(format!("moderator={m}")), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "account": acct })).by(&a))
}

pub async fn delete(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let id = account_id(&req)?;
    accounts::delete_account_admin(&req.app, id).await.map_err(|e| Reply::err(400, e))?;
    audit::record(&req.app, &a.username, "swarm_account_delete", Opts { target_type: Some("account".into()), target_id: Some(id.to_string()), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

/// Unique ids (max 200) from body.ids.
pub fn bulk_ids(req: &Req) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for v in req.json.get("ids").and_then(Value::as_array).cloned().unwrap_or_default() {
        if out.len() < 200 && !out.iter().any(|o| o.to_string() == v.to_string()) {
            out.push(v);
        }
    }
    out
}
fn id_of(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

pub async fn bulk_delete(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for v in bulk_ids(&req) {
        match id_of(&v) {
            Some(id) => match accounts::delete_account_admin(&req.app, id).await {
                Ok(()) => ok.push(v),
                Err(e) => bad.push(json!({ "id": v, "error": crate::util::cut(&e, 240) })),
            },
            None => bad.push(json!({ "id": v, "error": "Invalid id" })),
        }
    }
    audit::record(&req.app, &a.username, "swarm_account_bulk_delete", Opts { target_type: Some("account".into()), details: Some(format!("succeeded_ids={} failed={}", ok.len(), bad.len())), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "succeeded": ok, "failed": bad })).by(&a))
}

pub async fn bulk_verify(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let verified = req.jb("verified");
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for v in bulk_ids(&req) {
        match id_of(&v) {
            Some(id) if accounts::set_webhook_verified_admin(&req.app, id, verified).await.is_some() => ok.push(v),
            _ => bad.push(json!({ "id": v, "error": "Account not found" })),
        }
    }
    audit::record(&req.app, &a.username, "swarm_account_bulk_verify", Opts { target_type: Some("account".into()), details: Some(format!("verified={verified} succeeded_ids={} failed={}", ok.len(), bad.len())), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "succeeded": ok, "failed": bad })).by(&a))
}
