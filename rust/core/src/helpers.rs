//! App logic that used to live in Swift view models and model extensions.

use crate::client::{CachedDashboard, Inner};
use crate::error::ApiError;
use crate::lenient::{Lenient, SwiftInt};
use crate::models::*;
use crate::store::keys;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// ---- internal (used by SwarmCore) -----------------------------------------------------------
pub(crate) fn cache_dashboard(i: &Inner, d: &DashboardResponse) {
    let now = chrono::Utc::now().timestamp() as f64;
    let last = i.store.get(keys::DASHBOARD_CACHE_AT.into(), false).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    if now - last < 30.0 {
        return;
    }
    if let Ok(s) = serde_json::to_string(d) {
        if s.len() < 250_000 {
            i.store.set(keys::DASHBOARD_CACHE.into(), Some(s), false);
            i.store.set(keys::DASHBOARD_CACHE_AT.into(), Some(now.to_string()), false);
        }
    }
}

pub(crate) fn cached_dashboard(i: &Inner) -> Option<CachedDashboard> {
    let text = i.store.get(keys::DASHBOARD_CACHE.into(), false)?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let saved_at = i.store.get(keys::DASHBOARD_CACHE_AT.into(), false).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some(CachedDashboard { dashboard: DashboardResponse::from_json(&v)?, saved_at })
}

pub(crate) async fn own_session(i: &Arc<Inner>) -> Result<OwnSession, ApiError> {
    if i.token().is_none() {
        return Err(ApiError::invalid("Sign in to SwarmPanel first."));
    }
    let g = i.store.get(keys::LAST_GUILD.into(), true).filter(|g| !g.is_empty()).ok_or_else(|| ApiError::invalid("No guild is linked to this account."))?;
    let d: DashboardResponse = i.get("/api/dashboard", &[]).await?;
    own_session_in(d, g).ok_or_else(|| ApiError::invalid("Your guild's bot has no active session right now."))
}

pub(crate) fn convert_outcome(r: &ChannelConvertResponse, label: &str) -> ActionOutcome {
    let converted = r.converted.clone().unwrap_or_default();
    let warnings: Vec<String> = converted.iter().filter_map(|c| c.warning.clone()).collect();
    let failures: Vec<String> = r.failed.clone().unwrap_or_default().iter().filter_map(|f| f.error.clone()).collect();
    let mut m = format!("Converted {} channel(s) to {label}", converted.len());
    if let Some(s) = r.skipped_no_home_channel.filter(|s| s.0 > 0) {
        m += &format!(" ({} bot(s) had no home channel set)", s.0);
    }
    if !warnings.is_empty() {
        m += &format!(" — warnings: {}", warnings.join("; "));
    }
    if !failures.is_empty() {
        m += &format!(" — {} failed: {}", failures.len(), failures.join("; "));
    }
    ActionOutcome { ok: warnings.is_empty() && failures.is_empty(), message: m + "." }
}

pub(crate) async fn download(i: &Inner, path: &str) -> Result<Vec<u8>, ApiError> {
    let url = format!("{}{}", i.base_url(), path);
    let mut req = i.http.get(&url).timeout(std::time::Duration::from_secs(600));
    if let Some(t) = i.token() {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let bytes = resp.bytes().await?;
    if (200..300).contains(&status) {
        return Ok(bytes.to_vec());
    }
    if status == 401 {
        return Err(ApiError::Unauthorized);
    }
    let msg = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v.get("detail").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| format!("Download failed ({status})."));
    Err(ApiError::Server { status_code: status, message: msg })
}

pub(crate) async fn raw_document(i: &Inner, path: &str) -> Result<RawDocument, ApiError> {
    let text = i.request("GET", path, &[], None).await?;
    let v: Value = serde_json::from_str(&text).map_err(|e| ApiError::Decoding { message: e.to_string() })?;
    Ok(RawDocument { pretty: serde_json::to_string_pretty(&v).unwrap_or(text) })
}

pub(crate) async fn schemas(i: &Inner) -> Result<SchemasResponse, ApiError> {
    let text = i.request("GET", "/api/databases", &[("include_tables", Some("true".into()))], None).await?;
    let v: Value = serde_json::from_str(&text).map_err(|e| ApiError::Decoding { message: e.to_string() })?;
    let schemas = v
        .get("schemas")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    // Either a bare schema name or {schema, tables: [...]}
                    if let Some(name) = s.as_str() {
                        return Some(SchemaEntry { schema: name.into(), table_names: vec![] });
                    }
                    let name = s.get("schema").and_then(Value::as_str)?.to_string();
                    let tables = s.get("tables").and_then(Value::as_array).map(|t| t.iter().filter_map(table_name).collect()).unwrap_or_default();
                    Some(SchemaEntry { schema: name, table_names: tables })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(SchemasResponse { schemas: Some(schemas) })
}

fn table_name(t: &Value) -> Option<String> {
    match t {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => ["name", "table_name", "TABLE_NAME"].iter().find_map(|k| o.get(*k).and_then(Value::as_str)).map(str::to_string),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

/// Display text for one database cell.
fn cell(v: &Value) -> String {
    match v {
        Value::Null => "—".into(),
        Value::String(s) => {
            let t = s.trim();
            if (t.starts_with('{') || t.starts_with('[')) && t.len() > 2 {
                if let Ok(p) = serde_json::from_str::<Value>(t) {
                    return serde_json::to_string_pretty(&p).unwrap_or_else(|_| s.clone());
                }
            }
            s.clone()
        }
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

pub(crate) async fn table_data(i: &Inner, schema: &str, table: &str, limit: i64) -> Result<TableData, ApiError> {
    let text = i.request("GET", "/api/database/data", &[("schema_name", Some(schema.into())), ("table_name", Some(table.into())), ("limit", Some(limit.to_string()))], None).await?;
    let v: Value = serde_json::from_str(&text).map_err(|e| ApiError::Decoding { message: e.to_string() })?;
    let rows = v.get("rows").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut columns: Vec<String> = Vec::new();
    for r in &rows {
        if let Some(o) = r.as_object() {
            for k in o.keys() {
                if !columns.contains(k) && k != "password_hash" {
                    columns.push(k.clone());
                }
            }
        }
    }
    let out = rows.iter().map(|r| TableRow { cells: columns.iter().map(|c| cell(r.get(c).unwrap_or(&Value::Null))).collect() }).collect();
    Ok(TableData { columns, rows: out, count: v.get("count").and_then(SwiftInt::from_json) })
}

// ---- exported pure helpers ---------------------------------------------------------------------
/// YouTube thumbnail for a video URL (control-state sessions don't carry one).
#[uniffi::export]
pub fn youtube_thumbnail(video_url: String) -> Option<String> {
    let u = url::Url::parse(video_url.trim()).ok()?;
    let host = u.host_str()?.to_lowercase();
    let id = if host.contains("youtu.be") {
        u.path_segments()?.next().map(str::to_string)
    } else if host.contains("youtube.com") {
        u.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned()).or_else(|| {
            let parts: Vec<&str> = u.path_segments()?.filter(|p| !p.is_empty()).collect();
            let idx = parts.iter().position(|p| matches!(*p, "embed" | "shorts" | "live"))?;
            parts.get(idx + 1).map(|s| s.to_string())
        })
    } else {
        None
    }?;
    (!id.is_empty()).then(|| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
}

fn display(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "—".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(o) => o.to_string(),
    }
}

/// `{"before": {...}, "after": {...}}` audit details -> one row per changed field.
#[uniffi::export]
pub fn audit_diff_pairs(details: Option<String>) -> Option<Vec<AuditDiffPair>> {
    let v: Value = serde_json::from_str(details?.trim()).ok()?;
    let (b, a) = (v.get("before")?.as_object()?, v.get("after")?.as_object()?);
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        return None;
    }
    Some(keys.into_iter().map(|k| AuditDiffPair { key: k.clone(), before: display(b.get(k)), after: display(a.get(k)) }).collect())
}

const ACTIONS: &[(&str, bool, bool, bool, bool)] = &[
    // raw, voice, source, loop, filter
    ("PLAY", true, true, false, false),
    ("SMART_RECOMMEND", true, false, false, false),
    ("PAUSE", false, false, false, false),
    ("RESUME", false, false, false, false),
    ("SKIP", false, false, false, false),
    ("STOP", false, false, false, false),
    ("CLEAR", false, false, false, false),
    ("RESET_QUEUE", false, false, false, false),
    ("SHUFFLE", false, false, false, false),
    ("LOOP", false, false, true, false),
    ("FILTER", false, false, false, true),
    ("LEAVE", false, false, false, false),
    ("SET_HOME", true, false, false, false),
    ("RECOVER", false, false, false, false),
];

fn title_case(raw: &str) -> String {
    raw.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Orders the Controls screen offers (RESTART is web/owner-only).
#[uniffi::export]
pub fn control_actions() -> Vec<ControlActionInfo> {
    ACTIONS
        .iter()
        .map(|(r, v, s, l, f)| ControlActionInfo { raw_value: r.to_string(), label: title_case(r), needs_voice_channel: *v, needs_source_url: *s, needs_loop_mode: *l, needs_filter_mode: *f })
        .collect()
}

#[uniffi::export]
pub fn all_loop_modes() -> Vec<String> {
    ["off", "song", "queue"].map(String::from).to_vec()
}

#[uniffi::export]
pub fn all_filter_modes() -> Vec<String> {
    ["none", "nightcore", "bassboost", "vaporwave", "8d", "karaoke", "tremolo", "vibrato", "lowpass", "lofi", "electronic", "party", "radio", "cinema"].map(String::from).to_vec()
}

#[uniffi::export]
pub fn all_alert_rule_types() -> Vec<String> {
    ["bot_offline", "queue_stuck", "stale_metrics", "recovery_pending"].map(String::from).to_vec()
}

#[uniffi::export]
pub fn all_gallery_moderation_statuses() -> Vec<String> {
    ["pending", "approved", "rejected"].map(String::from).to_vec()
}

#[uniffi::export]
pub fn all_gallery_report_statuses() -> Vec<String> {
    ["open", "reviewed", "dismissed"].map(String::from).to_vec()
}

/// Payload for an order from the Controls form (validates required fields).
#[uniffi::export]
pub fn control_payload(action: String, source_url: String, voice_channel_id: String, loop_mode: String, filter_mode: String) -> Result<HashMap<String, String>, ApiError> {
    let mut p = HashMap::new();
    match action.as_str() {
        "PLAY" => {
            if source_url.trim().is_empty() {
                return Err(ApiError::invalid("Enter a URL or search terms to play."));
            }
            p.insert("source_url".into(), source_url.trim().into());
            p.insert("voice_channel_id".into(), voice_channel_id);
        }
        "SET_HOME" | "SMART_RECOMMEND" => {
            if action == "SET_HOME" && voice_channel_id.is_empty() {
                return Err(ApiError::invalid("Select a voice channel before setting the home channel."));
            }
            p.insert("voice_channel_id".into(), voice_channel_id);
        }
        "LOOP" => {
            p.insert("loop_mode".into(), loop_mode);
        }
        "FILTER" => {
            p.insert("filter_mode".into(), filter_mode);
        }
        _ => {}
    }
    Ok(p)
}

/// "GWS will run PLAY with "…" in My Guild."
#[uniffi::export]
pub fn command_preview(bot_label: String, guild_label: String, action: String, source_url: String, loop_mode: String, filter_mode: String) -> String {
    let detail = match action.as_str() {
        "PLAY" if !source_url.is_empty() => format!(" with \"{source_url}\""),
        "LOOP" => format!(" ({loop_mode})"),
        "FILTER" => format!(" ({filter_mode})"),
        _ => String::new(),
    };
    format!("{bot_label} will run {action}{detail} in {guild_label}.")
}

/// The session for `guild_id` on whichever bot has one.
#[uniffi::export]
pub fn own_session_in(dashboard: DashboardResponse, guild_id: String) -> Option<OwnSession> {
    for b in dashboard.bots.unwrap_or_default() {
        if let Some(s) = b.sessions.clone().unwrap_or_default().into_iter().find(|s| s.guild_id.as_deref() == Some(guild_id.as_str())) {
            return Some(OwnSession { bot: b, session: s });
        }
    }
    None
}

/// The guild a bot is most usefully controlled in: its live session, else its first.
#[uniffi::export]
pub fn pick_guild_for_bot(dashboard: DashboardResponse, bot_key: String) -> Option<String> {
    let bot = dashboard.bots?.into_iter().find(|b| b.key == bot_key)?;
    let sessions = bot.sessions.unwrap_or_default();
    let s = sessions.iter().find(|s| s.is_playing == Some(true)).or(sessions.first())?;
    s.guild_id.clone().filter(|g| !g.is_empty())
}

#[uniffi::export]
pub fn bot_is_offline(bot: DashboardBot) -> bool {
    bot.status.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("offline")) || bot.heartbeat_age_seconds.is_some_and(|a| a > 120.0)
}

#[uniffi::export]
pub fn bot_featured_session(bot: DashboardBot) -> Option<DashboardSession> {
    let all = bot.sessions.unwrap_or_default();
    all.iter().find(|s| s.is_playing == Some(true)).cloned().or_else(|| all.first().cloned())
}

#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct FleetSummary {
    pub total: SwiftInt,
    pub online: SwiftInt,
    pub live_sessions: SwiftInt,
    pub queue_depth: SwiftInt,
    pub backup_depth: SwiftInt,
    pub guilds_served: SwiftInt,
}

/// Fleet totals over the music bots (orchestrator excluded).
#[uniffi::export]
pub fn fleet_summary(dashboard: DashboardResponse) -> FleetSummary {
    let bots: Vec<DashboardBot> = dashboard.bots.unwrap_or_default().into_iter().filter(|b| b.kind.as_deref() != Some("orchestrator")).collect();
    let n = |f: &dyn Fn(&DashboardBot) -> i64| SwiftInt(bots.iter().map(f).sum());
    FleetSummary {
        total: SwiftInt(bots.len() as i64),
        online: SwiftInt(bots.iter().filter(|b| !bot_is_offline((*b).clone())).count() as i64),
        live_sessions: n(&|b| b.sessions.as_ref().map(|s| s.iter().filter(|s| s.is_playing == Some(true)).count() as i64).unwrap_or(0)),
        queue_depth: n(&|b| b.queue_depth.map(|q| q.0).unwrap_or(0)),
        backup_depth: n(&|b| b.backup_queue_depth.map(|q| q.0).unwrap_or(0)),
        guilds_served: n(&|b| b.known_guild_count.map(|q| q.0).unwrap_or(0)),
    }
}

/// Postgres-style "YYYY-MM-DD HH:MM:SS[.fff][+00]" (UTC) -> unix seconds.
#[uniffi::export]
pub fn observed_at_unix(raw: Option<String>) -> Option<f64> {
    let mut t = raw?.trim().replacen('T', " ", 1);
    if t.is_empty() {
        return None;
    }
    if let Some(p) = t.rfind('+') {
        t.truncate(p);
    }
    let t = t.trim_end_matches('Z');
    let (whole, frac) = t.split_once('.').unwrap_or((t, ""));
    let dt = chrono::NaiveDateTime::parse_from_str(whole, "%Y-%m-%d %H:%M:%S").ok()?;
    let f: f64 = if frac.is_empty() { 0.0 } else { format!("0.{frac}").parse().unwrap_or(0.0) };
    Some(dt.and_utc().timestamp() as f64 + f)
}

/// Live playback position at `now_unix`, advancing from the observed position while playing.
#[uniffi::export]
pub fn session_position(position_seconds: Option<SwiftInt>, duration_seconds: Option<SwiftInt>, is_playing: Option<bool>, position_observed_at: Option<String>, now_unix: f64) -> f64 {
    let mut pos = position_seconds.map(|p| p.0 as f64).unwrap_or(0.0);
    if is_playing == Some(true) {
        if let Some(at) = observed_at_unix(position_observed_at) {
            pos += now_unix - at;
        }
    }
    if let Some(d) = duration_seconds.filter(|d| d.0 > 0) {
        pos = pos.min(d.0 as f64);
    }
    pos.max(0.0)
}

/// m:ss
#[uniffi::export]
pub fn format_duration(seconds: f64) -> String {
    let t = seconds.max(0.0) as i64;
    format!("{}:{:02}", t / 60, t % 60)
}

/// 45s / 12m / 3h 4m / 2d 5h
#[uniffi::export]
pub fn format_uptime(seconds: f64) -> String {
    let s = seconds.max(0.0) as i64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h {}m", s / 3600, s / 60 % 60),
        _ => format!("{}d {}h", s / 86400, s / 3600 % 24),
    }
}

/// What the home-screen widget and Live Activity show.
#[uniffi::export]
pub fn widget_snapshot(dashboard: DashboardResponse, guild_id: Option<String>) -> WidgetSnapshot {
    let bots = dashboard.bots.clone().unwrap_or_default();
    let sessions: Vec<&DashboardSession> = bots.iter().flat_map(|b| b.sessions.as_deref().unwrap_or(&[])).collect();
    let own = guild_id.and_then(|g| own_session_in(dashboard, g)).map(|o| o.session);
    WidgetSnapshot {
        title: own.as_ref().and_then(|s| s.title.clone()).unwrap_or_default(),
        subtitle: own.as_ref().and_then(|s| s.channel_name.clone().or(s.guild_name.clone())).unwrap_or_default(),
        is_playing: own.as_ref().and_then(|s| s.is_playing).unwrap_or(false),
        is_paused: own.as_ref().and_then(|s| s.is_paused).unwrap_or(false),
        position: own.as_ref().and_then(|s| s.position_seconds).map(|p| p.0 as f64).unwrap_or(0.0),
        duration: own.as_ref().and_then(|s| s.duration_seconds).map(|p| p.0 as f64).unwrap_or(0.0),
        thumbnail: own.as_ref().and_then(|s| s.thumbnail.clone()).filter(|t| !t.is_empty()),
        bots_count: SwiftInt(bots.len() as i64),
        live_count: SwiftInt(sessions.iter().filter(|s| s.is_playing == Some(true)).count() as i64),
        queued_count: SwiftInt(sessions.iter().map(|s| s.queue_count.map(|q| q.0).unwrap_or(0)).sum()),
    }
}

#[derive(uniffi::Record, Clone, Debug, Default, PartialEq)]
pub struct GuildOverlap {
    pub guild_id: String,
    pub guild_label: String,
    pub bot_labels: Vec<String>,
}

/// Guilds served by more than one bot (Fleet Topology).
#[uniffi::export]
pub fn fleet_overlaps(dashboard: DashboardResponse) -> Vec<GuildOverlap> {
    let mut by: std::collections::BTreeMap<String, (String, Vec<String>)> = Default::default();
    for b in dashboard.bots.unwrap_or_default() {
        let bot = b.display_name.clone().filter(|d| !d.is_empty()).unwrap_or(b.key.clone());
        for s in b.sessions.unwrap_or_default() {
            let Some(g) = s.guild_id.clone() else { continue };
            let e = by.entry(g.clone()).or_insert_with(|| (s.guild_name.clone().unwrap_or(format!("Guild {g}")), vec![]));
            if !e.1.contains(&bot) {
                e.1.push(bot.clone());
            }
        }
    }
    let mut out: Vec<GuildOverlap> = by
        .into_iter()
        .filter(|(_, (_, bots))| bots.len() > 1)
        .map(|(g, (label, mut bots))| {
            bots.sort();
            GuildOverlap { guild_id: g, guild_label: label, bot_labels: bots }
        })
        .collect();
    out.sort_by(|a, b| a.guild_label.cmp(&b.guild_label));
    out
}

/// Sessions stuck in "recovering" (bot + session pairs).
#[uniffi::export]
pub fn find_recoverable_sessions(dashboard: DashboardResponse) -> Vec<OwnSession> {
    let mut out = Vec::new();
    for b in dashboard.bots.unwrap_or_default() {
        for s in b.sessions.clone().unwrap_or_default() {
            if s.session_state.as_deref() == Some("recovering") && s.guild_id.is_some() {
                out.push(OwnSession { bot: b.clone(), session: s });
            }
        }
    }
    out
}

/// "3 succeeded, 1 failed." for a bulk account action.
#[uniffi::export]
pub fn bulk_summary(result: SwarmAccountBulkResult) -> String {
    let ok = result.succeeded.map(|v| v.len()).unwrap_or(0);
    let failed = result.failed.map(|v| v.len()).unwrap_or(0);
    if failed > 0 { format!("{ok} succeeded, {failed} failed.") } else { format!("{ok} succeeded.") }
}

/// Discord channel type codes 2/13 are voice/stage.
#[uniffi::export]
pub fn is_voice_channel(channel_type: Option<SwiftInt>) -> bool {
    matches!(channel_type.map(|t| t.0), Some(2) | Some(13))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails() {
        assert_eq!(youtube_thumbnail("https://www.youtube.com/watch?v=abc123".into()).as_deref(), Some("https://i.ytimg.com/vi/abc123/hqdefault.jpg"));
        assert_eq!(youtube_thumbnail("https://youtu.be/xyz".into()).as_deref(), Some("https://i.ytimg.com/vi/xyz/hqdefault.jpg"));
        assert_eq!(youtube_thumbnail("https://youtube.com/shorts/s1".into()).as_deref(), Some("https://i.ytimg.com/vi/s1/hqdefault.jpg"));
        assert_eq!(youtube_thumbnail("https://soundcloud.com/a".into()), None);
    }

    #[test]
    fn diffs() {
        let d = audit_diff_pairs(Some(r#"{"before":{"a":1,"b":"x"},"after":{"a":2}}"#.into())).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].key, "a");
        assert_eq!(d[1].after, "—");
        assert!(audit_diff_pairs(Some("succeeded_ids=3".into())).is_none());
    }

    #[test]
    fn lenient_models() {
        let v: Value = serde_json::from_str(r#"{"bots":[{"key":"gws","queue_depth":"12","sessions":[{"guild_id":1247394560007471134,"is_playing":1}]}]}"#).unwrap();
        let d = DashboardResponse::from_json(&v).unwrap();
        let b = &d.bots.as_ref().unwrap()[0];
        assert_eq!(b.queue_depth, Some(SwiftInt(12)));
        let s = &b.sessions.as_ref().unwrap()[0];
        assert_eq!(s.guild_id.as_deref(), Some("1247394560007471134"));
        assert_eq!(s.is_playing, Some(true));
        let w = widget_snapshot(d.clone(), Some("1247394560007471134".into()));
        assert!(w.is_playing);
        assert_eq!(fleet_summary(d).live_sessions, SwiftInt(1));
    }

    #[test]
    fn times() {
        let t = observed_at_unix(Some("2026-10-27 12:00:00.5+00".into())).unwrap();
        assert_eq!(t, 1_793_102_400.5);
        assert_eq!(format_duration(125.0), "2:05");
        assert_eq!(format_uptime(3700.0), "1h 1m");
        assert_eq!(session_position(Some(SwiftInt(10)), Some(SwiftInt(100)), Some(true), Some("2026-10-27 12:00:00".into()), 1_793_102_405.0), 15.0);
    }

    #[test]
    fn payloads() {
        assert!(control_payload("SET_HOME".into(), "".into(), "".into(), "".into(), "".into()).is_err());
        let p = control_payload("LOOP".into(), "".into(), "".into(), "song".into(), "".into()).unwrap();
        assert_eq!(p.get("loop_mode").map(String::as_str), Some("song"));
        assert_eq!(control_actions().len(), 14);
    }
}
