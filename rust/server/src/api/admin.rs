//! Admin/moderator API: image gallery, monitoring, Lumisound, databases,
//! exports, audit log, Lumisound download.

use crate::audit::{self, Opts};
use crate::db::{J, P};
use crate::http::{R, Reply, Req};
use serde_json::{Value, json};

fn lim(req: &Req, d: i64, max: i64) -> i64 {
    req.q("limit").and_then(|l| l.parse::<f64>().ok()).map(|f| f as i64).unwrap_or(d).clamp(1, max)
}
fn pid(req: &Req, k: &str) -> P {
    req.json.get(k).map(P::from).unwrap_or(P::Null)
}
fn sid(req: &Req, k: &str) -> Option<String> {
    req.js(k)
}

// ---- image gallery -------------------------------------------------------------------------
pub async fn gallery_admin(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let d = crate::gallery::admin_data(&req.app, lim(&req, 50, 200)).await.map_err(|e| Reply::err(503, format!("Image Gallery database unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "data": d })).by(&a))
}

pub async fn gallery_media_csv(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let d = crate::gallery::admin_data(&req.app, lim(&req, 200, 500)).await.map_err(|e| Reply::err(503, format!("Image Gallery database unavailable: {e}")))?;
    audit::record(&req.app, &a.username, "image_gallery_export_media_csv", Opts { target_type: Some("gallery_media".into()), ..Default::default() }).await;
    Ok(crate::csv::response(d["media"].as_array().map(Vec::as_slice).unwrap_or(&[]), "image_gallery_media").by(&a))
}

pub async fn gallery_users_csv(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let d = crate::gallery::admin_data(&req.app, lim(&req, 200, 500)).await.map_err(|e| Reply::err(503, format!("Image Gallery database unavailable: {e}")))?;
    audit::record(&req.app, &a.username, "image_gallery_export_users_csv", Opts { target_type: Some("gallery_user".into()), ..Default::default() }).await;
    Ok(crate::csv::response(d["users"].as_array().map(Vec::as_slice).unwrap_or(&[]), "image_gallery_users").by(&a))
}

pub async fn gallery_tables(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let t = crate::browser::list_tables(&req.app, "image_gallery").await.map_err(|e| Reply::err(503, format!("Image Gallery database unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "schema": "image_gallery", "tables": t })).by(&a))
}

pub async fn gallery_table_data(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let d = crate::browser::table_data(&req.app, "image_gallery", &req.q("table_name").unwrap_or_default(), lim(&req, 100, 500)).await.map_err(|e| Reply::err(400, e))?;
    Ok(Reply::ok(json!({ "ok": true, "rows": d["rows"], "count": d["count"], "data": d })).by(&a))
}

pub async fn gallery_user_delete(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let _ = crate::gallery::delete_user(&req.app, &pid(&req, "user_id")).await;
    audit::record(&req.app, &a.username, "image_gallery_delete_user", Opts { target_type: Some("gallery_user".into()), target_id: sid(&req, "user_id"), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

pub async fn gallery_comment_delete(req: Req) -> R<Reply> {
    let a = req.require_gallery_admin_or_mod().await?;
    let _ = crate::gallery::delete_comment(&req.app, &pid(&req, "comment_id")).await;
    audit::record(&req.app, &a.username, "image_gallery_delete_comment", Opts { target_type: Some("gallery_comment".into()), target_id: sid(&req, "comment_id"), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

pub async fn gallery_reset_password(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    crate::gallery::reset_password(&req.app, &pid(&req, "user_id"), &req.js("new_password").unwrap_or_default()).await.map_err(|e| Reply::err(400, e))?;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

pub async fn gallery_user_update(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let mut u = serde_json::Map::new();
    for k in ["username", "display_name", "email", "public_profile", "show_liked_count", "adult_content_consent"] {
        if let Some(v) = req.json.get(k).filter(|v| !v.is_null()) {
            u.insert(k.into(), v.clone());
        }
    }
    let user = crate::gallery::update_user(&req.app, &pid(&req, "user_id"), &u).await.map_err(|e| Reply::err(400, e))?;
    Ok(Reply::ok(json!({ "ok": true, "user": user })).by(&a))
}

pub async fn gallery_email_verified(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let user = crate::gallery::set_email_verified(&req.app, &pid(&req, "user_id"), req.jb("verified")).await;
    Ok(Reply::ok(json!({ "ok": true, "user": user })).by(&a))
}

pub async fn gallery_age_verified(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let user = crate::gallery::set_age_verified(&req.app, &pid(&req, "user_id"), req.jb("verified")).await;
    Ok(Reply::ok(json!({ "ok": true, "user": user })).by(&a))
}

pub async fn gallery_resend_verification(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let app = &req.app;
    let id = pid(&req, "user_id");
    let user = crate::gallery::user_admin(app, &id).await.ok_or_else(|| Reply::err(404, "Image Gallery user not found"))?;
    let Some(email) = user.s("email") else { return Err(Reply::err(400, "This Image Gallery user does not have an email address.")) };
    if user.s("email_verified_at").is_some() {
        return Ok(Reply::ok(json!({ "ok": true, "email_verification_sent": false, "already_verified": true })).by(&a));
    }
    let token = crate::auth::verification_code();
    crate::gallery::issue_email_token(app, &id, &crate::auth::sha256_hex(&token)).await;
    let url = crate::notify::image_gallery_verification_url(&app.settings, &token);
    let sent = match crate::notify::send_image_gallery_verification_email(&app.settings, &email, &url, &token).await {
        Ok(()) => true,
        Err(e) => {
            println!("[swarmpanel] image gallery verification email send failed: {e}");
            false
        }
    };
    Ok(Reply::ok(json!({ "ok": sent, "email_verification_sent": sent, "already_verified": false })).by(&a))
}

pub async fn gallery_media_update(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let mut u = serde_json::Map::new();
    for k in ["title", "is_adult", "moderation_status", "moderation_reason"] {
        if let Some(v) = req.json.get(k).filter(|v| !v.is_null()) {
            u.insert(k.into(), v.clone());
        }
    }
    let m = crate::gallery::update_media(&req.app, &pid(&req, "media_id"), &u).await.map_err(|e| Reply::err(400, e))?;
    Ok(Reply::ok(json!({ "ok": true, "media": m })).by(&a))
}

pub async fn gallery_media_delete(req: Req) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let _ = crate::gallery::delete_media(&req.app, &pid(&req, "media_id")).await;
    audit::record(&req.app, &a.username, "image_gallery_delete_media", Opts { target_type: Some("gallery_media".into()), target_id: sid(&req, "media_id"), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

#[derive(Clone, Copy)]
enum Bulk {
    Media,
    Comments,
    Users,
}

async fn gallery_bulk(req: Req, kind: Bulk) -> R<Reply> {
    let a = req.require_gallery_owner().await?;
    let app = &req.app;
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for v in crate::api::accounts_admin::bulk_ids(&req) {
        let id = P::from(&v);
        let r = match kind {
            Bulk::Media => crate::gallery::delete_media(app, &id).await,
            Bulk::Comments => crate::gallery::delete_comment(app, &id).await,
            Bulk::Users => crate::gallery::delete_user(app, &id).await,
        };
        match r {
            Ok(_) => ok.push(v),
            Err(e) => bad.push(json!({ "id": v, "error": crate::util::cut(&e, 240) })),
        }
    }
    let (action, tt) = match kind {
        Bulk::Media => ("image_gallery_bulk_delete_media", "gallery_media"),
        Bulk::Comments => ("image_gallery_bulk_delete_comments", "gallery_comment"),
        Bulk::Users => ("image_gallery_bulk_delete_users", "gallery_user"),
    };
    audit::record(app, &a.username, action, Opts { target_type: Some(tt.into()), details: Some(format!("succeeded_ids={} failed={}", ok.len(), bad.len())), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "succeeded": ok, "failed": bad })).by(&a))
}

pub async fn gallery_bulk_media(req: Req) -> R<Reply> {
    gallery_bulk(req, Bulk::Media).await
}
pub async fn gallery_bulk_comments(req: Req) -> R<Reply> {
    gallery_bulk(req, Bulk::Comments).await
}
pub async fn gallery_bulk_users(req: Req) -> R<Reply> {
    gallery_bulk(req, Bulk::Users).await
}

pub async fn gallery_report_status(req: Req) -> R<Reply> {
    let a = req.require_gallery_admin_or_mod().await?;
    crate::gallery::update_report_status(&req.app, &pid(&req, "report_id"), &req.js("status").unwrap_or_default()).await.map_err(|e| Reply::err(400, e))?;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

// ---- monitoring -------------------------------------------------------------------------------
pub async fn stability(req: Req) -> R<Reply> {
    let a = req.require_auth().await?;
    req.read_limit(&a)?;
    Ok(Reply::ok(crate::metrics::stability(&req.app).await).by(&a))
}

pub async fn metrics(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    Ok(Reply::ok(crate::metrics::snapshot(&req.app).await).by(&a))
}

fn hours(req: &Req) -> i64 {
    req.q("hours").and_then(|h| h.parse::<f64>().ok()).map(|h| h as i64).unwrap_or(24).clamp(1, 720)
}

pub async fn metrics_history(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let m = req.q("metric").unwrap_or_else(|| "queued_tracks".into());
    let h = hours(&req);
    let p = crate::metrics::history(&req.app, &m, h).await.map_err(|e| Reply::err(503, format!("Metrics history unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "metric": m, "hours": h, "points": p })).by(&a))
}

pub async fn metrics_anomalies(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let m = req.q("metric").unwrap_or_else(|| "queued_tracks".into());
    let h = hours(&req);
    let p = crate::metrics::anomalies(&req.app, &m, h).await.map_err(|e| Reply::err(503, format!("Metric anomalies unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "metric": m, "hours": h, "anomalies": p })).by(&a))
}

pub async fn events(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    Ok(Reply::ok(json!({ "events": crate::metrics::events(&req.app, lim(&req, 50, 100)).await })).by(&a))
}

// ---- lumisound --------------------------------------------------------------------------------
pub async fn lumisound_admin(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    let d = crate::lumisound::admin_data(&req.app, lim(&req, 50, 200)).await.map_err(|e| Reply::err(503, format!("Lumisound database unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "ok": true, "data": d })).by(&a))
}

pub async fn lumisound_upload_delete(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    crate::lumisound::delete_upload(&req.app, &pid(&req, "upload_id")).await;
    audit::record(&req.app, &a.username, "lumisound_delete_upload", Opts { target_type: Some("lumisound_upload".into()), target_id: sid(&req, "upload_id"), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

pub async fn lumisound_user_active(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    let active = req.jb("active");
    let u = crate::lumisound::set_user_active(&req.app, &pid(&req, "user_id"), active).await.ok_or_else(|| Reply::err(404, "Lumisound user not found"))?;
    audit::record(&req.app, &a.username, if active { "lumisound_user_active" } else { "lumisound_user_suspend" }, Opts { target_type: Some("lumisound_user".into()), target_id: sid(&req, "user_id"), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "user": u })).by(&a))
}

pub async fn lumisound_bug_status(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    let st = req.js("status").unwrap_or_default();
    crate::lumisound::update_bug_status(&req.app, &pid(&req, "report_id"), &st).await.map_err(|e| Reply::err(400, e))?;
    audit::record(&req.app, &a.username, "lumisound_bug_report_status", Opts { target_type: Some("lumisound_bug_report".into()), target_id: sid(&req, "report_id"), details: Some(st), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true })).by(&a))
}

// ---- databases --------------------------------------------------------------------------------
pub async fn databases(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let schemas = crate::browser::list_schemas(&req.app);
    if req.q("include_tables").unwrap_or_default().to_lowercase() != "true" {
        return Ok(Reply::ok(json!({ "schemas": schemas })).by(&a));
    }
    let out = futures_util::future::join_all(schemas.iter().map(|s| {
        let app = req.app.clone();
        async move { json!({ "schema": s, "tables": crate::browser::list_tables(&app, s).await.unwrap_or_default() }) }
    }))
    .await;
    Ok(Reply::ok(json!({ "schemas": out })).by(&a))
}

pub async fn database_tables(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let s = req.p("schema");
    let t = crate::browser::list_tables(&req.app, &s).await.map_err(|e| Reply::err(503, format!("Database unavailable: {e}")))?;
    Ok(Reply::ok(json!({ "schema": s, "tables": t })).by(&a))
}

fn owner_phrase_ok(req: &Req) -> R<()> {
    let expected = req.app.settings.destructive_confirmation_phrase.trim().to_string();
    if !expected.is_empty() && req.js("owner_confirm_text").unwrap_or_default().trim() != expected {
        return Err(Reply::err(400, format!("Owner confirmation mismatch. Expected exact text: {expected}")));
    }
    Ok(())
}

pub async fn truncate_table(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let (s, t) = (req.js("schema_name").unwrap_or_default(), req.js("table_name").unwrap_or_default());
    let expected = format!("TRUNCATE {s}.{t}");
    if req.js("confirm_text").unwrap_or_default().trim() != expected {
        return Err(Reply::err(400, format!("Confirmation mismatch. Expected exact text: {expected}")));
    }
    owner_phrase_ok(&req)?;
    crate::browser::truncate_table(&req.app, &s, &t).await.map_err(|e| Reply::err(503, format!("Database unavailable: {e}")))?;
    audit::record(&req.app, &a.username, "truncate_table", Opts { target_type: Some("table".into()), target_id: Some(format!("{s}.{t}")), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "message": format!("Truncated {s}.{t}") })).by(&a))
}

pub async fn truncate_schema(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let s = req.js("schema_name").unwrap_or_default();
    let expected = format!("TRUNCATE ALL {s}");
    if req.js("confirm_text").unwrap_or_default().trim() != expected {
        return Err(Reply::err(400, format!("Confirmation mismatch. Expected exact text: {expected}")));
    }
    owner_phrase_ok(&req)?;
    let mut r = crate::browser::truncate_schema(&req.app, &s).await.map_err(|e| Reply::err(503, format!("Database unavailable: {e}")))?;
    audit::record(&req.app, &a.username, "truncate_schema", Opts { target_type: Some("schema".into()), target_id: Some(s), details: Some(format!("truncated_tables={}", r["truncated_tables"])), ..Default::default() }).await;
    r["ok"] = json!(true);
    Ok(Reply::ok(r).by(&a))
}

pub async fn database_data(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let (s, t) = (req.q("schema_name").unwrap_or_default(), req.q("table_name").unwrap_or_default());
    let d = crate::browser::table_data(&req.app, &s, &t, lim(&req, 100, 500)).await.map_err(|e| Reply::err(400, e))?;
    if req.q("format").unwrap_or_default().to_lowercase() == "csv" {
        audit::record(&req.app, &a.username, "database_export_csv", Opts { target_type: Some("table".into()), target_id: Some(format!("{s}.{t}")), ..Default::default() }).await;
        return Ok(crate::csv::response(d["rows"].as_array().map(Vec::as_slice).unwrap_or(&[]), &format!("{s}_{t}")).by(&a));
    }
    Ok(Reply::ok(json!({ "ok": true, "rows": d["rows"], "count": d["count"], "data": d })).by(&a))
}

// ---- exports ------------------------------------------------------------------------------------
pub async fn exports(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    Ok(Reply::ok(crate::fleet::exports_payload(&req.app)).by(&a))
}

pub async fn export_file(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let (date, file) = (req.p("date"), req.p("filename"));
    let date_ok = date.len() == 10 && date.chars().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == '-' } else { c.is_ascii_digit() });
    let file_ok = file.ends_with(".csv") && file.len() > 4 && file.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) && !file.contains("..");
    if !date_ok || !file_ok {
        return Err(Reply::err(400, "Invalid export path"));
    }
    let body = tokio::fs::read(format!("{}/{date}/{file}", req.app.settings.scheduled_exports_dir)).await.map_err(|_| Reply::err(404, "Export file not found"))?;
    Ok(Reply::bytes(200, body, "text/csv").header("Content-Disposition", format!("attachment; filename=\"{date}_{file}\"")).by(&a))
}

// ---- audit log --------------------------------------------------------------------------------
pub async fn audit_log(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    let off = req.q("offset").and_then(|o| o.parse::<i64>().ok()).unwrap_or(0).max(0);
    let action = req.q("action").map(|s| crate::util::cut(&s, 80));
    Ok(Reply::ok(json!({ "ok": true, "data": audit::list(&req.app, lim(&req, 100, 500), off, action.as_deref()).await })).by(&a))
}

pub async fn audit_revert(req: Req) -> R<Reply> {
    let a = req.require_admin().await?;
    let app = &req.app;
    let id: i64 = req.p("entry_id").parse().map_err(|_| Reply::err(404, "Audit log entry not found"))?;
    let e = audit::get(app, id).await.ok_or_else(|| Reply::err(404, "Audit log entry not found"))?;
    let action = e.s("action").unwrap_or_default();
    let before = e.s("details").and_then(|d| serde_json::from_str::<Value>(&d).ok()).and_then(|d| d.get("before").cloned()).filter(|b| b.is_object());
    let target = e.s("target_id").unwrap_or_default();
    let pick = |keys: &[&str]| -> serde_json::Map<String, Value> {
        let b = before.clone().unwrap_or_default();
        keys.iter().filter_map(|k| b.get(*k).filter(|v| !v.is_null()).map(|v| (k.to_string(), v.clone()))).collect()
    };
    let result = match action.as_str() {
        "swarm_account_update" | "alert_rule_update" => {
            if before.is_none() {
                return Err(Reply::err(400, "This entry has no recorded before-state to revert to."));
            }
            if action == "swarm_account_update" {
                let tid: i64 = target.parse().map_err(|_| Reply::err(404, "Revert target no longer exists."))?;
                crate::accounts::update_account_admin(app, tid, &pick(&["username", "guild_id", "email", "display_name", "public_profile", "server_name"])).await.map_err(|e| Reply::err(400, e))?
            } else {
                crate::alerts::update(app, &target, &pick(&["rule_type", "threshold_minutes", "enabled", "escalation_minutes", "escalate_email"])).await.map_err(|e| Reply::err(400, e))?.ok_or_else(|| Reply::err(404, "Revert target no longer exists."))?
            }
        }
        _ => return Err(Reply::err(400, format!("Action '{action}' cannot be reverted."))),
    };
    audit::record(app, &a.username, &format!("{action}_revert"), Opts { target_type: e.s("target_type"), target_id: Some(target), details: Some(format!("reverted_from_entry={id}")), ..Default::default() }).await;
    Ok(Reply::ok(json!({ "ok": true, "result": result })).by(&a))
}

// ---- Lumisound .ipa download (private GitHub release, via the REST API) ------------------------
const LUMISOUND_REPO: &str = "HollowHeavenXR/Lumisound";

pub async fn lumisound_download(req: Req) -> R<Reply> {
    let a = req.require_admin_or_mod().await?;
    let app = &req.app;
    app.limiter.check(&format!("lumisound-download:{}", a.username.to_lowercase()), 5, 3600)?;
    let token = crate::config::env("GH_TOKEN", "");
    let gh = |url: String, accept: &'static str| {
        let mut r = app.http.get(url).header("User-Agent", "SwarmPanel-Rust").header("Accept", accept).timeout(std::time::Duration::from_secs(120));
        if !token.is_empty() {
            r = r.bearer_auth(&token);
        }
        r
    };
    let unreachable = || Reply::err(502, "Could not reach GitHub to check the latest Lumisound release. Try again shortly.");
    let rel: Value = gh(format!("https://api.github.com/repos/{LUMISOUND_REPO}/releases?per_page=1"), "application/vnd.github+json").send().await.map_err(|_| unreachable())?.json().await.map_err(|_| unreachable())?;
    let rel = rel.get(0).cloned().ok_or_else(unreachable)?;
    let tag: String = rel.s("tag_name").unwrap_or_default().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    let asset = rel["assets"].as_array().and_then(|a| a.iter().find(|x| x.s("name").is_some_and(|n| n.ends_with(".ipa")))).cloned().ok_or_else(unreachable)?;
    let dir = format!("{}/.runtime/project_downloads", app.settings.panel_root);
    let path = format!("{dir}/lumisound_{tag}.ipa");
    let body = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => {
            let url = asset.s("url").ok_or_else(unreachable)?;
            let bytes = gh(url, "application/octet-stream").send().await.map_err(|_| unreachable())?.bytes().await.map_err(|_| unreachable())?.to_vec();
            let _ = tokio::fs::create_dir_all(&dir).await;
            let tmp = format!("{path}.tmp.{}", rand::random::<u32>());
            if tokio::fs::write(&tmp, &bytes).await.is_ok() {
                let _ = tokio::fs::rename(&tmp, &path).await;
            }
            bytes
        }
    };
    Ok(Reply::bytes(200, body, "application/octet-stream")
        .header("Content-Disposition", format!("attachment; filename=\"Lumisound-{tag}.ipa\""))
        .header("Cache-Control", "private, max-age=0, no-cache")
        .by(&a))
}
