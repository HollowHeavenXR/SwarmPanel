//! Discord REST lookups with caching (port of discord_service.lua): bot
//! identity, guilds, channels, name resolution, channel type conversion,
//! invite URLs.

use crate::app::AppRef;
use crate::db::J;
use base64::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

const API: &str = "https://discord.com/api/v10";

pub async fn request(app: &AppRef, token: &str, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, String> {
    for attempt in 0..3 {
        let mut rb = app
            .http
            .request(method.clone(), format!("{API}{path}"))
            .header("Authorization", format!("Bot {token}"))
            .header("User-Agent", "DiscordBot (https://swarmpanel, 2.0) SwarmPanel-Rust")
            .timeout(Duration::from_secs(crate::config::env_num("SWARM_PANEL_DISCORD_TIMEOUT_SECONDS", 5)));
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = rb.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status == 429 && attempt < 2 {
            let wait = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("retry_after").and_then(Value::as_f64)).unwrap_or(1.0);
            tokio::time::sleep(Duration::from_secs_f64(wait.clamp(0.05, 10.0))).await;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(format!("http {status} {}", crate::util::cut(&text, 300)));
        }
        return Ok(if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::Null) });
    }
    Err("http 429 rate limited".into())
}

async fn cached_get(app: &AppRef, token: &str, path: &str) -> Result<Value, String> {
    let key = format!("{}\0{path}", crate::auth::sha256_hex(token));
    if let Some(v) = app.cache.discord.get(&key, Duration::from_secs(300)) {
        return match v.get("err").and_then(Value::as_str) {
            Some(e) => Err(e.to_string()),
            None => Ok(v["ok"].clone()),
        };
    }
    if let Some(v) = app.cache.discord.get(&format!("err:{key}"), Duration::from_secs(30)) {
        return Err(v.as_str().unwrap_or("Discord request failed.").to_string());
    }
    match request(app, token, reqwest::Method::GET, path, None).await {
        Ok(v) => {
            app.cache.discord.put(&key, json!({ "ok": v }));
            Ok(v)
        }
        Err(e) => {
            app.cache.discord.put(&format!("err:{key}"), json!(e));
            Err(e)
        }
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.s(k)
}

pub async fn fetch_identity(app: &AppRef, token: &str) -> Result<Value, String> {
    let d = cached_get(app, token, "/users/@me").await?;
    let id = s(&d, "id");
    let avatar = s(&d, "avatar");
    Ok(json!({
        "id": id, "username": s(&d, "username"), "global_name": s(&d, "global_name"), "discriminator": s(&d, "discriminator"), "avatar": avatar,
        "avatar_url": match (&id, &avatar) { (Some(i), Some(a)) => Some(format!("https://cdn.discordapp.com/avatars/{i}/{a}.png?size=128")), _ => None },
    }))
}

pub async fn fetch_guilds(app: &AppRef, token: &str) -> (Vec<Value>, Option<String>) {
    match cached_get(app, token, "/users/@me/guilds?limit=200").await {
        Err(e) => (vec![], Some(e)),
        Ok(d) => (
            d.as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|g| {
                    let id = s(g, "id").unwrap_or_default();
                    json!({ "id": id, "name": s(g, "name").unwrap_or_else(|| format!("Guild {id}")), "owner": g.b("owner"), "permissions": g.get("permissions") })
                })
                .collect(),
            None,
        ),
    }
}

pub async fn fetch_guild(app: &AppRef, token: &str, gid: &str) -> Value {
    match cached_get(app, token, &format!("/guilds/{gid}")).await {
        Ok(d) => json!({ "id": s(&d, "id").unwrap_or_else(|| gid.into()), "name": s(&d, "name").unwrap_or_else(|| format!("Guild {gid}")) }),
        Err(_) => json!({ "id": gid, "name": format!("Unknown/Inaccessible Guild {gid}") }),
    }
}

fn type_name(t: i64) -> String {
    match t {
        0 => "text".into(),
        2 => "voice".into(),
        4 => "category".into(),
        5 => "announcement".into(),
        10 => "announcement_thread".into(),
        11 => "public_thread".into(),
        12 => "private_thread".into(),
        13 => "stage".into(),
        15 => "forum".into(),
        other => other.to_string(),
    }
}

pub async fn fetch_guild_channels(app: &AppRef, token: &str, gid: &str) -> Vec<Value> {
    let Ok(d) = cached_get(app, token, &format!("/guilds/{gid}/channels")).await else { return vec![] };
    d.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let id = s(c, "id").unwrap_or_default();
            let t = c.i("type").unwrap_or(-1);
            json!({ "id": id, "name": s(c, "name").unwrap_or_else(|| format!("Channel {id}")), "type": t, "type_name": type_name(t), "parent_id": s(c, "parent_id") })
        })
        .collect()
}

pub const VOICE: i64 = 2;
pub const STAGE: i64 = 13;

/// Recreate a channel as voice/stage (Discord can't convert in place).
/// Ok((new_id, warning)).
pub async fn recreate_channel_as_type(app: &AppRef, token: &str, channel_id: &str, new_type: i64) -> Result<(String, Option<String>), String> {
    let full = request(app, token, reqwest::Method::GET, &format!("/channels/{channel_id}"), None).await.map_err(|e| format!("could not read existing channel: {e}"))?;
    if full.i("type") == Some(new_type) {
        return Ok((s(&full, "id").unwrap_or_default(), None));
    }
    let mut payload = json!({
        "name": full.get("name"), "type": new_type, "parent_id": full.get("parent_id"), "position": full.get("position"),
        "permission_overwrites": full.get("permission_overwrites"), "bitrate": full.get("bitrate"), "user_limit": full.get("user_limit"),
        "rtc_region": full.get("rtc_region"), "nsfw": full.get("nsfw"),
    });
    let mut dropped: Vec<String> = Vec::new();
    let guild = s(&full, "guild_id").unwrap_or_default();
    let mut created = Err(String::new());
    for _ in 0..4 {
        created = request(app, token, reqwest::Method::POST, &format!("/guilds/{guild}/channels"), Some(&payload)).await;
        let Err(e) = &created else { break };
        let offending = regex::Regex::new(r#""errors":\s*\{\s*"([A-Za-z_]+)""#).unwrap().captures(e).map(|c| c[1].to_string()).or_else(|| e.contains("30052").then(|| "bitrate".to_string()));
        match offending {
            Some(f) if payload.get(&f).is_some_and(|v| !v.is_null()) => {
                payload.as_object_mut().unwrap().remove(&f);
                dropped.push(f);
            }
            _ => break,
        }
    }
    let created = created.map_err(|e| format!("could not create replacement channel: {e}"))?;
    let new_id = s(&created, "id").unwrap_or_default();
    let del = request(app, token, reqwest::Method::DELETE, &format!("/channels/{channel_id}"), None).await;
    app.cache.discord.remove(&format!("{}\0/guilds/{guild}/channels", crate::auth::sha256_hex(token)));
    let mut warning = (!dropped.is_empty()).then(|| {
        format!(
            "created with Discord's default {} for a {} channel -- the original value{} not valid for that channel type",
            dropped.join("/"),
            if new_type == STAGE { "stage" } else { "voice" },
            if dropped.len() > 1 { "s were" } else { " was" }
        )
    });
    if let Err(e) = del {
        let d = format!("created the replacement channel but couldn't delete the old one ({e}) -- it's now an orphaned duplicate, safe to delete by hand");
        warning = Some(match warning {
            Some(w) => format!("{w}; {d}"),
            None => d,
        });
    }
    Ok((new_id, warning))
}

pub async fn fetch_inventory(app: &AppRef, token: &str, include_channels: bool, hints: &[String]) -> Value {
    let mut errors: Vec<String> = Vec::new();
    let identity = match fetch_identity(app, token).await {
        Ok(i) => i,
        Err(e) => {
            errors.push(format!("identity: {e}"));
            Value::Null
        }
    };
    let (guilds, gerr) = fetch_guilds(app, token).await;
    if let Some(e) = gerr {
        errors.push(format!("guilds: {e}"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Value> = guilds.into_iter().filter(|g| seen.insert(g.s("id").unwrap_or_default())).collect();
    for h in hints {
        if seen.insert(h.clone()) {
            out.push(fetch_guild(app, token, h).await);
        }
    }
    for g in out.iter_mut() {
        if include_channels {
            let chans = fetch_guild_channels(app, token, &g.s("id").unwrap_or_default()).await;
            if chans.is_empty() {
                g["channels_error"] = json!("No channels visible to this bot in this guild.");
            }
            g["channels"] = json!(chans);
        } else {
            g["channels"] = json!([]);
        }
    }
    json!({ "identity": identity, "guilds": out, "errors": errors })
}

/// (guild_id, channel_id or "") -> {guild_name, channel_name}
pub async fn resolve_names(app: &AppRef, token: &str, placements: &[(String, Option<String>)]) -> HashMap<(String, String), (String, Option<String>)> {
    let mut by: HashMap<String, Vec<String>> = HashMap::new();
    for (g, c) in placements {
        by.entry(g.clone()).or_default().push(c.clone().unwrap_or_default());
    }
    let mut out = HashMap::new();
    for (g, cids) in by {
        let guild = fetch_guild(app, token, &g).await;
        let gname = guild.s("name").unwrap_or_default();
        let chans = fetch_guild_channels(app, token, &g).await;
        for c in cids {
            let cname = (!c.is_empty()).then(|| chans.iter().find(|x| x.s("id").as_deref() == Some(c.as_str())).and_then(|x| x.s("name"))).flatten();
            out.insert((g.clone(), c), (gname.clone(), cname));
        }
    }
    out
}

pub fn client_id_from_token(token: &str) -> Option<String> {
    let head = token.split('.').next().filter(|h| !h.is_empty())?;
    let dec = base64::engine::general_purpose::STANDARD.decode(head).or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(head)).ok()?;
    Some(String::from_utf8(dec).ok()?.trim().to_string()).filter(|s| !s.is_empty())
}

const BITS: &[(&str, u32)] = &[
    ("Kick Members", 1), ("Ban Members", 2), ("Manage Channels", 4), ("Manage Server", 5), ("View Channels", 10), ("Send Messages", 11), ("Manage Messages", 13), ("Embed Links", 14),
    ("Attach Files", 15), ("Read Message History", 16), ("Connect", 20), ("Speak", 21), ("Use Voice Activity", 25), ("Manage Nicknames", 27), ("Manage Roles", 28),
    ("Use Application Commands", 31), ("Request To Speak", 32), ("Send Messages In Threads", 38), ("Timeout Members", 40), ("Set Voice Channel Status", 48),
];
pub const MUSIC_PERMS: &[&str] = &["View Channels", "Send Messages", "Send Messages In Threads", "Embed Links", "Attach Files", "Read Message History", "Connect", "Speak", "Use Voice Activity", "Use Application Commands", "Request To Speak", "Manage Channels", "Set Voice Channel Status"];
pub const ARIA_PERMS: &[&str] = &["View Channels", "Send Messages", "Send Messages In Threads", "Embed Links", "Attach Files", "Read Message History", "Use Application Commands", "Kick Members", "Manage Messages", "Manage Channels", "Manage Server", "Manage Roles", "Manage Nicknames", "Ban Members", "Timeout Members"];

pub fn capability_summary(kind: &str) -> &'static str {
    match kind {
        "music" => "Music worker node: slash commands, queue controls, voice/stage playback, feedback messages, embeds, buttons, and channel status/topic updates.",
        "orchestrator" => "Aria orchestrator: slash commands, swarm routing, AI/game/economy tools, scheduled messages, moderation, roles, nicknames, slowmode, channel locks, and server utilities.",
        _ => "Discord bot with slash commands and server tools.",
    }
}

pub fn permission_value(perms: &[&str]) -> u64 {
    perms.iter().filter_map(|p| BITS.iter().find(|(n, _)| n == p)).map(|(_, b)| 1u64 << b).sum()
}

pub fn invite_url(client_id: &str, perms: u64, guild: Option<&str>) -> String {
    let mut u = format!("https://discord.com/oauth2/authorize?client_id={client_id}&permissions={perms}&scope=bot%20applications.commands");
    if let Some(g) = guild.filter(|g| !g.is_empty()) {
        u.push_str(&format!("&guild_id={g}&disable_guild_select=true"));
    }
    u
}
