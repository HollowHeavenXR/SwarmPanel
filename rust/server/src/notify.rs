//! Outbound notices (port of notify.lua): Discord webhook verification codes,
//! bot DMs, guild-ownership proof via a webhook, and SMTP mail.

use crate::app::AppRef;
use crate::config::Settings;
use crate::db::J;
use serde_json::{Value, json};
use std::time::Duration;

const UA: &str = "SwarmPanel-Rust (https://swarmpanel, 2.0)";

pub fn validate_webhook_url(v: &str) -> Result<String, String> {
    let url = v.trim().to_string();
    let bad = || "Invalid Discord webhook URL".to_string();
    let rest = url.strip_prefix("https://").ok_or_else(bad)?;
    let (host, path) = rest.split_once('/').ok_or_else(bad)?;
    if !matches!(host.to_lowercase().as_str(), "discord.com" | "www.discord.com" | "discordapp.com" | "www.discordapp.com") {
        return Err(bad());
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 4 || parts[0] != "api" || parts[1] != "webhooks" {
        return Err(bad());
    }
    Ok(url)
}

fn webhook_parts(url: &str) -> Option<(String, String)> {
    let path = url.strip_prefix("https://")?.split_once('/')?.1;
    let p: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    Some((p.get(2)?.to_string(), p.get(3)?.to_string()))
}

async fn post_json(app: &AppRef, url: &str, body: &Value) -> Result<u16, String> {
    let r = app.http.post(url).header("User-Agent", UA).json(body).timeout(Duration::from_secs(10)).send().await.map_err(|e| format!("request failed: {e}"))?;
    Ok(r.status().as_u16())
}

pub async fn send_verification_webhook_code(app: &AppRef, url: &str, code: &str, username: &str, guild_id: &str) -> bool {
    let Ok(url) = validate_webhook_url(url) else { return false };
    let content = crate::util::cut(
        &format!("SwarmPanel verification for `{username}` in guild `{guild_id}`\nCode: **{code}**\nEnter this code in SwarmPanel to verify your account. If you did not request this, you can ignore this message."),
        1900,
    );
    matches!(post_json(app, &url, &json!({ "content": content, "allowed_mentions": { "parse": [] }, "username": "SwarmPanel Verify" })).await, Ok(s) if s < 400)
}

#[allow(dead_code)] // kept for alert escalation to Discord webhooks
pub async fn send_webhook_content(app: &AppRef, url: &str, content: &str, username: Option<&str>) -> Result<(), String> {
    let url = validate_webhook_url(url)?;
    let mut body = json!({ "content": content, "allowed_mentions": { "parse": [] } });
    if let Some(u) = username {
        body["username"] = json!(u);
    }
    match post_json(app, &url, &body).await? {
        s if s < 400 => Ok(()),
        s => Err(format!("http {s}")),
    }
}

pub async fn send_discord_dm(app: &AppRef, user_id: &str, content: &str) -> Result<(), String> {
    let token = app.settings.discord_bot_token.clone();
    if token.is_empty() {
        return Err("Discord DM verification isn't configured on this server yet.".into());
    }
    if user_id.is_empty() || !user_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("That doesn't look like a valid Discord User ID (numbers only).".into());
    }
    let ch = crate::discord::request(app, &token, reqwest::Method::POST, "/users/@me/channels", Some(&json!({ "recipient_id": user_id })))
        .await
        .ok()
        .and_then(|c| c.s("id"))
        .ok_or("Could not open a DM with that account -- make sure you share a server with the verification bot and allow direct messages from server members.")?;
    crate::discord::request(app, &token, reqwest::Method::POST, &format!("/channels/{ch}/messages"), Some(&json!({ "content": content })))
        .await
        .map(|_| ())
        .map_err(|e| format!("Discord rejected the message: {e}"))
}

pub fn dm_code_text(code: &str, ttl: i64) -> String {
    format!("SwarmPanel verification code: **{code}**\nEnter this code in SwarmPanel to verify your account. It expires in {} minutes.", ttl / 60)
}

pub async fn discord_username(app: &AppRef, user_id: &str) -> Option<String> {
    let token = app.settings.discord_bot_token.clone();
    if token.is_empty() {
        return None;
    }
    let u = crate::discord::request(app, &token, reqwest::Method::GET, &format!("/users/{user_id}"), None).await.ok()?;
    u.s("global_name").or_else(|| u.s("username"))
}

/// The webhook must belong to the guild being registered. Ok((channel_id, webhook_name)).
pub async fn verify_guild_proof(app: &AppRef, guild_id: &str, proof_url: &str) -> Result<(String, String), String> {
    let g = guild_id.trim();
    if g.is_empty() || !g.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Guild ID must be a Discord server ID number.".into());
    }
    let g = g.trim_start_matches('0');
    let g = if g.is_empty() { "0" } else { g };
    let url = validate_webhook_url(proof_url)?;
    let (id, tok) = webhook_parts(&url).ok_or("Invalid Discord webhook URL")?;
    let r = app
        .http
        .get(format!("https://discord.com/api/v10/webhooks/{id}/{tok}"))
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| "The Discord webhook proof could not be verified right now. Try again in a moment.")?;
    if r.status().as_u16() >= 400 {
        return Err("The Discord webhook proof could not be verified right now. Try again in a moment.".into());
    }
    let p: Value = r.json().await.map_err(|_| "The Discord webhook proof could not be verified. Create a new webhook in that server and try again.")?;
    if p.s("guild_id").map(|s| s.trim().to_string()).as_deref() != Some(g) {
        return Err("The Discord webhook proof belongs to a different guild.".into());
    }
    let ch = p.s("channel_id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).ok_or("The Discord webhook proof did not return a channel binding.")?;
    let name = crate::util::cut(p.s("name").unwrap_or_else(|| "Webhook".into()).trim(), 80);
    Ok((ch, name))
}

pub fn smtp_configured(s: &Settings) -> bool {
    !s.smtp_host.is_empty() && !s.smtp_from_email.is_empty()
}

pub async fn send_email(s: &Settings, to: &str, subject: &str, body: &str) -> Result<(), String> {
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor, transport::smtp::authentication::Credentials};
    if !smtp_configured(s) {
        return Err("SMTP is not configured".into());
    }
    if crate::accounts::normalize_email(to).ok().flatten().is_none() {
        return Err("invalid recipient address".into());
    }
    let msg = Message::builder()
        .from(s.smtp_from_email.parse().map_err(|e| format!("bad from address: {e}"))?)
        .to(to.parse().map_err(|e| format!("bad recipient: {e}"))?)
        .subject(subject.replace(['\r', '\n'], " "))
        .body(body.to_string())
        .map_err(|e| e.to_string())?;
    let mut b = if s.smtp_use_tls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&s.smtp_host).map_err(|e| e.to_string())?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&s.smtp_host)
    }
    .port(s.smtp_port)
    .timeout(Some(Duration::from_secs(20)));
    if !s.smtp_username.is_empty() {
        b = b.credentials(Credentials::new(s.smtp_username.clone(), s.smtp_password.clone()));
    }
    b.build().send(msg).await.map(|_| ()).map_err(|e| e.to_string())
}

pub fn image_gallery_verification_url(s: &Settings, token: &str) -> String {
    let mut origin = crate::config::env_opt("IMAGE_GALLERY_PUBLIC_BACKEND_URL").map(|o| o.trim().to_string()).unwrap_or_default();
    if origin.is_empty() {
        if let Ok(t) = std::fs::read_to_string(format!("{}/../Image Gallery/live-config.json", s.panel_root)) {
            if let Some(u) = serde_json::from_str::<Value>(&t).ok().and_then(|v| v.s("gallery_url")).filter(|u| u.starts_with("http")) {
                origin = u.trim().to_string();
            }
        }
    }
    let mut origin = origin.trim_end_matches('/').to_string();
    if origin.is_empty() {
        origin = s.pages_public_url.trim_end_matches('/').to_string();
    }
    format!("{origin}/api/auth/verify-email?token={}", urlencoding::encode(token))
}

pub async fn send_image_gallery_verification_email(s: &Settings, to: &str, url: &str, code: &str) -> Result<(), String> {
    let body = format!(
        "Welcome to Image Gallery.\n\nYour verification code is:\n\n{code}\n\nEnter this code in Image Gallery to verify your email address.\n\nYou can also verify by opening the link below:\n\n{url}\n\nIf you did not create this account, you can ignore this message."
    );
    send_email(s, to, "Verify your Image Gallery email", &body).await
}
