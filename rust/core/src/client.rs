//! `SwarmCore`: HTTP client, session (token, refresh, 401 handling) and a
//! typed method for every endpoint the Apple apps call.

use crate::error::ApiError;
use crate::lenient::{Lenient, SwiftInt};
use crate::live::LiveSocket;
use crate::models::*;
use crate::store::{PlatformStore, keys};
use crate::telemetry::Telemetry;
use crate::{RT, on_rt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://swarmpanel.xenusanimations.studio";

#[derive(uniffi::Record, Clone)]
pub struct CoreConfig {
    /// "ios" | "tvos"
    pub client: String,
    pub app_version: String,
    pub os_version: String,
    pub device_model: String,
}

#[derive(uniffi::Record, Clone, Debug, Default, PartialEq)]
pub struct SessionState {
    pub is_authenticated: bool,
    pub username: String,
    pub role: String,
    pub guild_id: Option<String>,
    pub is_owner: bool,
    pub is_admin: bool,
    pub is_moderator: bool,
    pub can_gallery: bool,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq)]
pub enum SessionChange {
    SignedIn,
    Refreshed,
    SignedOut,
    Expired,
}

#[uniffi::export(with_foreign)]
pub trait SessionObserver: Send + Sync {
    fn session_changed(&self, state: SessionState, change: SessionChange);
}

pub(crate) struct Inner {
    pub cfg: CoreConfig,
    pub store: Arc<dyn PlatformStore>,
    pub http: reqwest::Client,
    pub token: RwLock<Option<String>>,
    pub session: RwLock<SessionState>,
    observers: Mutex<Vec<Arc<dyn SessionObserver>>>,
    refresh: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub telemetry: Telemetry,
}

impl Inner {
    pub fn base_url(&self) -> String {
        self.store.get(keys::BASE_URL.into(), false).filter(|u| u.starts_with("http")).unwrap_or_else(|| DEFAULT_BASE_URL.into()).trim_end_matches('/').to_string()
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().unwrap().clone()
    }

    fn set_token(&self, t: Option<String>) {
        *self.token.write().unwrap() = t.clone();
        self.store.save_token(t);
    }

    fn notify(&self, change: SessionChange) {
        let state = self.session.read().unwrap().clone();
        let obs = self.observers.lock().unwrap().clone();
        for o in obs {
            o.session_changed(state.clone(), change);
        }
    }

    pub async fn request(&self, method: &str, path: &str, query: &[(&str, Option<String>)], body: Option<Value>) -> Result<String, ApiError> {
        let mut url = format!("{}{}", self.base_url(), path);
        let q: Vec<String> = query.iter().filter_map(|(k, v)| v.as_ref().filter(|v| !v.is_empty()).map(|v| format!("{k}={}", urlencoding::encode(v)))).collect();
        if !q.is_empty() {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&q.join("&"));
        }
        let mut req = if method == "POST" { self.http.post(&url) } else { self.http.get(&url) };
        req = req.header("Accept", "application/json");
        if let Some(t) = self.token() {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(ApiError::from)?;
        if (200..300).contains(&status) {
            return Ok(text);
        }
        if status == 401 {
            self.expire();
            return Err(ApiError::Unauthorized);
        }
        Err(ApiError::Server { status_code: status, message: error_message(&text, status) })
    }

    pub async fn get<T: Lenient + Default>(&self, path: &str, query: &[(&str, Option<String>)]) -> Result<T, ApiError> {
        let text = self.request("GET", path, query, None).await?;
        decode(&text)
    }

    pub async fn post<T: Lenient + Default>(&self, path: &str, body: Value) -> Result<T, ApiError> {
        let text = self.request("POST", path, &[], Some(body)).await?;
        decode(&text)
    }

    /// Server says the token is no longer valid.
    fn expire(&self) {
        let was = self.session.read().unwrap().is_authenticated || self.token().is_some();
        self.set_token(None);
        self.stop_refresh();
        {
            let mut s = self.session.write().unwrap();
            s.is_authenticated = false;
        }
        self.store.set(keys::LAST_GUILD.into(), None, true);
        if was {
            self.telemetry.log("session_expired", None, None, HashMap::new());
            self.notify(SessionChange::Expired);
        }
    }

    fn apply(self: &Arc<Self>, p: &SessionPayload, change: SessionChange) -> SessionState {
        if let Some(t) = p.token.clone().filter(|t| !t.is_empty()) {
            self.set_token(Some(t));
        }
        let state = {
            let mut s = self.session.write().unwrap();
            s.username = p.username.clone().unwrap_or(s.username.clone());
            s.role = p.role.clone().unwrap_or(s.role.clone());
            s.guild_id = p.guild_id.clone().or(p.account_guild_id.clone()).or(s.guild_id.clone());
            s.is_owner = p.site_owner.unwrap_or(s.is_owner);
            s.is_admin = p.admin_mode.unwrap_or(s.is_admin);
            s.is_moderator = p.moderator.unwrap_or(s.is_moderator);
            s.can_gallery = p.image_gallery_owner.unwrap_or(s.can_gallery);
            s.is_authenticated = true;
            s.clone()
        };
        if let Some(g) = &state.guild_id {
            self.store.set(keys::LAST_GUILD.into(), Some(g.clone()), true);
        }
        if self.cfg.client == "tvos" {
            self.store.set(keys::TV_USERNAME.into(), Some(state.username.clone()), false);
        }
        self.start_refresh();
        self.notify(change);
        state
    }

    fn start_refresh(self: &Arc<Self>) {
        let mut slot = self.refresh.lock().unwrap();
        if slot.is_some() {
            return;
        }
        let me = Arc::downgrade(self);
        *slot = Some(RT.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30 * 60)).await;
                let Some(i) = me.upgrade() else { break };
                let _ = i.refresh_session("timer").await;
            }
        }));
    }

    fn stop_refresh(&self) {
        if let Some(h) = self.refresh.lock().unwrap().take() {
            h.abort();
        }
    }

    pub async fn refresh_session(self: &Arc<Self>, reason: &str) -> Result<SessionState, ApiError> {
        let started = std::time::Instant::now();
        match self.get::<SessionPayload>("/api/session", &[]).await {
            Ok(p) if p.authenticated == Some(false) => {
                self.expire();
                Err(ApiError::Unauthorized)
            }
            Ok(p) => {
                let s = self.apply(&p, SessionChange::Refreshed);
                self.telemetry.log("session_refresh", Some(ms(started)), None, HashMap::from([("reason".into(), reason.into())]));
                Ok(s)
            }
            Err(e) => {
                if !matches!(e, ApiError::Unauthorized) {
                    self.telemetry.log("session_refresh_failure", Some(ms(started)), None, HashMap::from([("reason".into(), reason.into()), ("kind".into(), e.kind())]));
                }
                Err(e)
            }
        }
    }
}

fn ms(t: std::time::Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1000.0).round()
}

fn decode<T: Lenient + Default>(text: &str) -> Result<T, ApiError> {
    if text.trim().is_empty() {
        return Ok(T::default());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| ApiError::Decoding { message: e.to_string() })?;
    T::from_json(&v).ok_or_else(|| ApiError::Decoding { message: "unexpected shape".into() })
}

/// `{"detail": "..."}` / `{"detail": [{"msg": ...}]}` -> text.
fn error_message(text: &str, status: u16) -> String {
    let fallback = format!("Request failed ({status}).");
    let Ok(v) = serde_json::from_str::<Value>(text) else { return fallback };
    match v.get("detail") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().map(|x| x.get("msg").and_then(Value::as_str).unwrap_or("Invalid request.").to_string()).collect::<Vec<_>>().join("; "),
        _ => fallback,
    }
}

fn opt(s: &str) -> Option<String> {
    Some(s.to_string())
}

#[derive(uniffi::Object)]
pub struct SwarmCore {
    pub(crate) inner: Arc<Inner>,
    live: Arc<LiveSocket>,
}

macro_rules! call {
    ($self:ident, |$i:ident| $body:expr) => {{
        let $i = $self.inner.clone();
        on_rt(async move { $body }).await
    }};
}

#[uniffi::export]
impl SwarmCore {
    #[uniffi::constructor]
    pub fn new(config: CoreConfig, store: Arc<dyn PlatformStore>) -> Arc<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(12))
            .user_agent(format!("SwarmPanel-{}/{}", config.client, config.app_version))
            .build()
            .expect("http client");
        let token = store.load_token().filter(|t| !t.is_empty());
        let telemetry = Telemetry::new(&config, store.clone());
        let inner = Arc::new(Inner {
            session: RwLock::new(SessionState { username: if config.client == "tvos" { store.get(keys::TV_USERNAME.into(), false).unwrap_or_default() } else { String::new() }, ..Default::default() }),
            cfg: config,
            store,
            http,
            token: RwLock::new(token),
            observers: Mutex::default(),
            refresh: Mutex::default(),
            telemetry,
        });
        inner.telemetry.attach(Arc::downgrade(&inner));
        let live = LiveSocket::new(Arc::downgrade(&inner));
        let has = inner.token().is_some();
        inner.telemetry.log("app_launch", None, None, HashMap::from([("restored_session".into(), has.to_string())]));
        Arc::new(SwarmCore { inner, live })
    }

    // ---- configuration & session ---------------------------------------------------------------
    pub fn base_url(&self) -> String {
        self.inner.base_url()
    }
    pub fn set_base_url(&self, url: String) {
        let u = url.trim().to_string();
        self.inner.store.set(keys::BASE_URL.into(), (!u.is_empty() && u != DEFAULT_BASE_URL).then_some(u), false);
    }
    pub fn default_base_url(&self) -> String {
        DEFAULT_BASE_URL.into()
    }
    pub fn has_token(&self) -> bool {
        self.inner.token().is_some()
    }
    pub fn session(&self) -> SessionState {
        self.inner.session.read().unwrap().clone()
    }
    pub fn last_known_guild_id(&self) -> Option<String> {
        self.inner.store.get(keys::LAST_GUILD.into(), true).filter(|g| !g.is_empty())
    }
    pub fn add_session_observer(&self, observer: Arc<dyn SessionObserver>) {
        self.inner.observers.lock().unwrap().push(observer);
    }
    pub fn live(&self) -> Arc<LiveSocket> {
        self.live.clone()
    }

    pub async fn login(&self, username: String, password: String, guild_id: Option<String>) -> Result<SessionState, ApiError> {
        call!(self, |i| {
            let started = std::time::Instant::now();
            i.telemetry.log("login_attempt", None, None, HashMap::new());
            let tv = i.cfg.client == "tvos";
            let name = username.trim().to_string();
            if name.is_empty() || password.is_empty() {
                return Err(ApiError::invalid("Enter your username and password."));
            }
            let g = guild_id.map(|g| g.trim().to_string()).filter(|g| !g.is_empty());
            let body = json!({ "username": name, "password": password, "guild_id": g, "remember_device": true, "client": i.cfg.client, "disable_admin_mode": tv });
            match i.post::<SessionPayload>("/api/session/login", body).await {
                Ok(p) if p.token.as_deref().is_none_or(str::is_empty) => {
                    i.telemetry.log("login_failure", None, None, HashMap::from([("kind".into(), "no_token".into())]));
                    Err(ApiError::invalid("The server didn't return a session."))
                }
                Ok(p) => {
                    let s = i.apply(&p, SessionChange::SignedIn);
                    i.telemetry.log("login_success", Some(ms(started)), None, HashMap::from([("expires_in_s".into(), p.expires_in.map(|e| e.0.to_string()).unwrap_or("?".into()))]));
                    Ok(s)
                }
                Err(e) => {
                    i.telemetry.log("login_failure", Some(ms(started)), None, HashMap::from([("kind".into(), e.kind())]));
                    Err(e)
                }
            }
        })
    }

    pub async fn register(&self, username: String, guild_id: String, password: String, email: String, verification_webhook_url: String, discord_user_id: String) -> Result<SessionState, ApiError> {
        call!(self, |i| {
            let nz = |s: String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
            let body = json!({
                "username": username.trim(), "guild_id": guild_id.trim(), "password": password, "email": nz(email),
                "verification_webhook_url": nz(verification_webhook_url), "discord_user_id": nz(discord_user_id),
                "remember_device": true, "client": i.cfg.client,
            });
            let p = i.post::<SessionPayload>("/api/session/register", body).await?;
            Ok(i.apply(&p, SessionChange::SignedIn))
        })
    }

    pub async fn set_admin_mode(&self, enabled: bool) -> Result<SessionState, ApiError> {
        call!(self, |i| {
            let p = i.post::<SessionPayload>("/api/session/admin-mode", json!({ "enabled": enabled })).await?;
            Ok(i.apply(&p, SessionChange::Refreshed))
        })
    }

    /// GET /api/session (rolling token renewal). `reason` is for telemetry.
    pub async fn refresh_session(&self, reason: String) -> Result<SessionState, ApiError> {
        call!(self, |i| i.refresh_session(&reason).await)
    }

    /// Restores a saved session at launch (no-op without a token).
    pub async fn bootstrap(&self) -> Option<SessionState> {
        call!(self, |i| {
            if i.token().is_none() {
                return None;
            }
            i.refresh_session("launch").await.ok()
        })
    }

    pub fn logout(&self) {
        let i = &self.inner;
        i.telemetry.log("sign_out", None, None, HashMap::new());
        let t = i.clone();
        RT.spawn(async move { t.telemetry.flush().await });
        self.live.disconnect();
        i.set_token(None);
        i.stop_refresh();
        *i.session.write().unwrap() = SessionState::default();
        i.store.set(keys::LAST_GUILD.into(), None, true);
        i.store.set(keys::DASHBOARD_CACHE.into(), None, false);
        i.store.set(keys::ACCOUNT_CACHE.into(), None, false);
        i.notify(SessionChange::SignedOut);
    }

    // ---- telemetry -----------------------------------------------------------------------------
    pub fn log(&self, name: String, value: Option<f64>, text: Option<String>, metadata: HashMap<String, String>) {
        self.inner.telemetry.log(&name, value, text, metadata);
    }
    pub async fn flush_telemetry(&self) {
        call!(self, |i| i.telemetry.flush().await)
    }

    // ---- fleet ---------------------------------------------------------------------------------
    pub async fn dashboard(&self) -> Result<DashboardResponse, ApiError> {
        call!(self, |i| {
            let d: DashboardResponse = i.get("/api/dashboard", &[]).await?;
            crate::helpers::cache_dashboard(&i, &d);
            Ok(d)
        })
    }
    /// Last dashboard seen (for instant first paint), with its unix timestamp.
    pub fn cached_dashboard(&self) -> Option<CachedDashboard> {
        crate::helpers::cached_dashboard(&self.inner)
    }
    /// Remembers the last account snapshot (Apple TV first paint).
    pub fn save_account_cache(&self, snapshot: AccountSnapshot) {
        if let Ok(t) = serde_json::to_string(&snapshot) {
            self.inner.store.set(keys::ACCOUNT_CACHE.into(), Some(t), false);
        }
    }
    pub fn cached_account(&self) -> Option<AccountSnapshot> {
        let t = self.inner.store.get(keys::ACCOUNT_CACHE.into(), false)?;
        crate::lenient::decode::<AccountSnapshot>(&t).ok()
    }
    /// Saves a dashboard pushed over the live socket (throttled to every 30s).
    pub fn save_dashboard_cache(&self, dashboard: DashboardResponse) {
        crate::helpers::cache_dashboard(&self.inner, &dashboard);
    }
    pub async fn bots(&self) -> Result<BotsResponse, ApiError> {
        call!(self, |i| i.get("/api/bots", &[]).await)
    }
    pub async fn bot_inventory(&self, bot_key: String) -> Result<BotInventoryResponse, ApiError> {
        call!(self, |i| i.get(&format!("/api/bots/{}/inventory", urlencoding::encode(&bot_key)), &[]).await)
    }
    pub async fn control_state(&self, bot_key: String, guild_id: String) -> Result<ControlStateResponse, ApiError> {
        call!(self, |i| i.get(&format!("/api/bots/{}/control-state", urlencoding::encode(&bot_key)), &[("guild_id", Some(guild_id))]).await)
    }
    /// POST /api/bots/control. `payload` values are strings (source_url, voice_channel_id, ...).
    pub async fn control(&self, bot_key: String, guild_id: String, action: String, payload: HashMap<String, String>) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/bots/control", json!({ "bot_key": bot_key, "guild_id": guild_id, "action": action, "payload": payload })).await)
    }
    pub async fn seek(&self, bot_key: String, guild_id: String, position_seconds: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/bots/control", json!({ "bot_key": bot_key, "guild_id": guild_id, "action": "SEEK", "payload": { "position_seconds": position_seconds.0 } })).await)
    }
    pub async fn saved_queues(&self, guild_id: String, bot_key: String) -> Result<SavedQueuesResponse, ApiError> {
        call!(self, |i| i.get("/api/queues", &[("guild_id", Some(guild_id)), ("bot_key", Some(bot_key))]).await)
    }
    pub async fn save_queue(&self, guild_id: String, bot_key: String, name: String, items: Vec<QueueItem>) -> Result<SavedQueueEnvelope, ApiError> {
        call!(self, |i| {
            if items.is_empty() {
                return Err(ApiError::invalid("The live queue is empty — nothing to save."));
            }
            let n = if name.trim().is_empty() { "Saved Queue".to_string() } else { name.trim().to_string() };
            i.post("/api/queues", json!({ "guild_id": guild_id, "bot_key": bot_key, "name": n, "items": items })).await
        })
    }
    pub async fn delete_saved_queue(&self, queue_id: SwiftInt, guild_id: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/queues/{}/delete", queue_id.0), json!({ "guild_id": guild_id })).await)
    }
    pub async fn rename_saved_queue(&self, queue_id: SwiftInt, guild_id: String, name: String) -> Result<SavedQueueEnvelope, ApiError> {
        call!(self, |i| {
            let n = name.trim().to_string();
            if n.is_empty() {
                return Err(ApiError::invalid("Enter a name."));
            }
            i.post(&format!("/api/queues/{}/rename", queue_id.0), json!({ "guild_id": guild_id, "name": n })).await
        })
    }
    /// PLAY every track of a saved queue into a voice channel; stops at the first failure.
    pub async fn load_saved_queue(&self, bot_key: String, guild_id: String, voice_channel_id: String, queue: SavedQueue) -> Result<ActionOutcome, ApiError> {
        call!(self, |i| {
            if voice_channel_id.is_empty() {
                return Err(ApiError::invalid("Select a voice channel before loading a saved queue."));
            }
            let items = queue.items.clone().unwrap_or_default();
            for it in &items {
                i.post::<OkResponse>("/api/bots/control", json!({ "bot_key": bot_key, "guild_id": guild_id, "action": "PLAY", "payload": { "source_url": it.video_url, "voice_channel_id": voice_channel_id } })).await?;
            }
            Ok(ActionOutcome { ok: true, message: format!("Queued {} track(s) from \"{}\".", items.len(), queue.name) })
        })
    }
    /// RECOVER every session stuck in "recovering"; summary like "Recovered 2/3 session(s).".
    pub async fn recover_all_stale(&self) -> Result<ActionOutcome, ApiError> {
        call!(self, |i| {
            let d: DashboardResponse = i.get("/api/dashboard", &[]).await?;
            let targets = crate::helpers::find_recoverable_sessions(d);
            let mut ok = 0;
            for t in &targets {
                let g = t.session.guild_id.clone().unwrap_or_default();
                if i.post::<OkResponse>("/api/bots/control", json!({ "bot_key": t.bot.key, "guild_id": g, "action": "RECOVER", "payload": {} })).await.is_ok() {
                    ok += 1;
                }
            }
            Ok(ActionOutcome { ok: ok == targets.len(), message: format!("Recovered {ok}/{} session(s).", targets.len()) })
        })
    }
    /// RESTART a whole bot process (owner only).
    pub async fn restart_bot(&self, bot_key: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/bots/control", json!({ "bot_key": bot_key, "guild_id": "0", "action": "RESTART", "payload": {} })).await)
    }
    pub async fn control_matrix(&self, guild_id: String) -> Result<ControlMatrixResponse, ApiError> {
        call!(self, |i| i.get(&format!("/api/guilds/{}/control-matrix", urlencoding::encode(&guild_id)), &[]).await)
    }
    /// Converts home channels to stage/voice and summarises the result.
    pub async fn convert_channels(&self, guild_id: String, to_stage: bool) -> Result<ActionOutcome, ApiError> {
        call!(self, |i| {
            if guild_id.is_empty() {
                return Err(ApiError::invalid("Choose a guild before converting channels."));
            }
            let label = if to_stage { "stage" } else { "voice" };
            let r: ChannelConvertResponse = i.post(&format!("/api/guilds/{}/convert-channels", urlencoding::encode(&guild_id)), json!({ "direction": label })).await?;
            Ok(crate::helpers::convert_outcome(&r, label))
        })
    }
    pub async fn guild_leaderboard(&self, guild_id: String, bot_key: String) -> Result<LeaderboardEnvelope, ApiError> {
        call!(self, |i| i.get(&format!("/api/guilds/{}/leaderboard", urlencoding::encode(&guild_id)), &[("bot_key", Some(bot_key))]).await)
    }
    pub async fn swarm_leaderboard(&self, days: Option<SwiftInt>, limit: Option<SwiftInt>) -> Result<SwarmLeaderboardResponse, ApiError> {
        call!(self, |i| i.get("/api/swarm-leaderboard", &[("days", days.map(|d| d.0.to_string())), ("limit", limit.map(|d| d.0.to_string()))]).await)
    }
    pub async fn music_intelligence(&self, guild_id: Option<String>, bot_key: Option<String>) -> Result<MusicIntelligenceEnvelope, ApiError> {
        call!(self, |i| i.get("/api/music-intelligence", &[("guild_id", guild_id), ("bot_key", bot_key)]).await)
    }
    /// Your guild's bot + session (Siri/Shortcuts/widget path; uses the last known guild).
    pub async fn own_session(&self) -> Result<OwnSession, ApiError> {
        call!(self, |i| crate::helpers::own_session(&i).await)
    }
    /// Sends `action` to your guild's bot and returns the session it targeted.
    pub async fn send_own_control(&self, action: String) -> Result<DashboardSession, ApiError> {
        call!(self, |i| {
            let own = crate::helpers::own_session(&i).await?;
            let g = own.session.guild_id.clone().ok_or_else(|| ApiError::invalid("Your guild's bot has no active session right now."))?;
            i.post::<OkResponse>("/api/bots/control", json!({ "bot_key": own.bot.key, "guild_id": g, "action": action, "payload": {} })).await?;
            Ok(own.session)
        })
    }

    // ---- account & profile -----------------------------------------------------------------------
    pub async fn me(&self) -> Result<MeResponse, ApiError> {
        call!(self, |i| i.get("/api/users/me", &[]).await)
    }
    pub async fn update_profile(&self, display_name: Option<String>, bio: Option<String>, public_profile: Option<bool>, theme_accent: Option<String>) -> Result<MeProfileEnvelope, ApiError> {
        call!(self, |i| i.post("/api/users/me", json!({ "display_name": display_name, "bio": bio, "public_profile": public_profile, "theme_accent": theme_accent })).await)
    }
    pub async fn preferences(&self) -> Result<PanelPreferencesEnvelope, ApiError> {
        call!(self, |i| i.get("/api/users/preferences", &[]).await)
    }
    pub async fn change_password(&self, current_password: String, new_password: String) -> Result<MeProfileEnvelope, ApiError> {
        call!(self, |i| {
            if new_password.chars().count() < 8 {
                return Err(ApiError::invalid("The new password must be at least 8 characters."));
            }
            i.post("/api/session/password", json!({ "current_password": current_password, "new_password": new_password })).await
        })
    }
    pub async fn change_email(&self, email: String) -> Result<MeProfileEnvelope, ApiError> {
        call!(self, |i| i.post("/api/session/email", json!({ "email": email.trim() })).await)
    }
    pub async fn set_verification_webhook(&self, url: String) -> Result<VerificationSendResponse, ApiError> {
        call!(self, |i| i.post("/api/session/verification-webhook", json!({ "verification_webhook_url": url.trim() })).await)
    }
    pub async fn set_verification_discord(&self, discord_user_id: String) -> Result<VerificationSendResponse, ApiError> {
        call!(self, |i| i.post("/api/session/verification-discord", json!({ "discord_user_id": discord_user_id.trim() })).await)
    }
    pub async fn verify_code(&self, code: String) -> Result<MeProfileEnvelope, ApiError> {
        call!(self, |i| i.post("/api/session/verification/verify", json!({ "code": code.trim() })).await)
    }
    pub async fn resend_verification(&self) -> Result<ResendVerificationResponse, ApiError> {
        call!(self, |i| i.post("/api/session/resend-verification", json!({})).await)
    }
    /// Downloads the latest Lumisound .ipa (bytes).
    pub async fn lumisound_download(&self) -> Result<Vec<u8>, ApiError> {
        call!(self, |i| crate::helpers::download(&i, "/api/projects/lumisound/download").await)
    }

    // ---- community -----------------------------------------------------------------------------
    pub async fn directory(&self, query: String, online_only: bool) -> Result<UserSearchResponse, ApiError> {
        call!(self, |i| i.get("/api/users/directory", &[("q", Some(query)), ("online", online_only.then(|| "1".to_string())), ("limit", opt("50"))]).await)
    }
    pub async fn search_users(&self, query: String) -> Result<UserSearchResponse, ApiError> {
        call!(self, |i| i.get("/api/users/directory", &[("q", Some(query))]).await)
    }
    pub async fn public_profile(&self, account_id: SwiftInt) -> Result<PublicProfileEnvelope, ApiError> {
        call!(self, |i| i.get(&format!("/api/users/{}/profile", account_id.0), &[]).await)
    }
    pub async fn follow(&self, account_id: SwiftInt, following: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/users/{}/follow", account_id.0), json!({ "following": following })).await)
    }
    pub async fn send_friend_request(&self, account_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/users/{}/friend-request", account_id.0), json!({})).await)
    }
    pub async fn friend_requests(&self) -> Result<FriendRequestsResponse, ApiError> {
        call!(self, |i| i.get("/api/friends/requests", &[]).await)
    }
    /// action: "accept" | "decline" | "cancel"
    pub async fn respond_friend_request(&self, request_id: SwiftInt, action: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/friends/requests/{}", request_id.0), json!({ "action": action })).await)
    }
    pub async fn friends(&self) -> Result<FriendsResponse, ApiError> {
        call!(self, |i| i.get("/api/me/friends", &[]).await)
    }
    pub async fn message_threads(&self) -> Result<MessageThreadsResponse, ApiError> {
        call!(self, |i| i.get("/api/messages/threads", &[]).await)
    }
    pub async fn messages(&self, account_id: SwiftInt) -> Result<MessagesResponse, ApiError> {
        call!(self, |i| i.get(&format!("/api/messages/{}", account_id.0), &[]).await)
    }
    pub async fn send_message(&self, account_id: SwiftInt, body: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| {
            let b = body.trim().to_string();
            if b.is_empty() {
                return Err(ApiError::invalid("Type a message first."));
            }
            i.post(&format!("/api/messages/{}", account_id.0), json!({ "body": b })).await
        })
    }
    pub async fn notifications(&self) -> Result<NotificationsResponse, ApiError> {
        call!(self, |i| i.get("/api/notifications", &[("limit", opt("50"))]).await)
    }
    pub async fn unread_count(&self) -> Result<UnreadCountResponse, ApiError> {
        call!(self, |i| i.get("/api/notifications/unread-count", &[]).await)
    }
    pub async fn mark_notification_read(&self, notification_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/notifications/{}/read", notification_id.0), json!({})).await)
    }
    pub async fn mark_all_notifications_read(&self) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/notifications/read-all", json!({})).await)
    }
    /// Unread notifications newer than the last one handed out here (for
    /// background-refresh local alerts); remembers the newest id it saw.
    pub async fn fresh_notifications(&self) -> Result<Vec<PanelNotification>, ApiError> {
        call!(self, |i| {
            if i.token().is_none() {
                return Ok(vec![]);
            }
            let r: NotificationsResponse = i.get("/api/notifications", &[]).await?;
            let unread: Vec<PanelNotification> = r.notifications.unwrap_or_default().into_iter().filter(|n| n.read_at.is_none()).collect();
            let last: i64 = i.store.get(keys::LAST_NOTIFIED.into(), false).and_then(|s| s.parse().ok()).unwrap_or(0);
            let mut fresh: Vec<PanelNotification> = unread.iter().filter(|n| n.id.0 > last).cloned().collect();
            fresh.sort_by_key(|n| n.id.0);
            if let Some(max) = unread.iter().map(|n| n.id.0).max() {
                i.store.set(keys::LAST_NOTIFIED.into(), Some(max.max(last).to_string()), false);
            }
            let n = fresh.len();
            Ok(fresh.into_iter().skip(n.saturating_sub(5)).collect())
        })
    }
    pub async fn community_counts(&self) -> Result<CommunityCounts, ApiError> {
        call!(self, |i| i.get("/api/community/counts", &[]).await)
    }

    // ---- admin ---------------------------------------------------------------------------------
    pub async fn admin_overview(&self) -> Result<AdminOverviewStats, ApiError> {
        call!(self, |i| i.get("/api/admin/overview", &[]).await)
    }
    pub async fn events(&self) -> Result<EventsResponse, ApiError> {
        call!(self, |i| i.get("/api/events", &[]).await)
    }
    pub async fn stability(&self) -> Result<RawDocument, ApiError> {
        call!(self, |i| crate::helpers::raw_document(&i, "/api/stability").await)
    }
    pub async fn metrics(&self) -> Result<RawDocument, ApiError> {
        call!(self, |i| crate::helpers::raw_document(&i, "/api/metrics").await)
    }
    pub async fn metric_history(&self, metric: String, hours: SwiftInt) -> Result<MetricHistoryResponse, ApiError> {
        call!(self, |i| i.get("/api/metrics/history", &[("metric", Some(metric)), ("hours", Some(hours.0.to_string()))]).await)
    }
    pub async fn metric_anomalies(&self, metric: String, hours: SwiftInt) -> Result<MetricAnomaliesResponse, ApiError> {
        call!(self, |i| i.get("/api/metrics/anomalies", &[("metric", Some(metric)), ("hours", Some(hours.0.to_string()))]).await)
    }
    pub async fn alert_rules(&self) -> Result<AlertRulesResponse, ApiError> {
        call!(self, |i| i.get("/api/alert-rules", &[]).await)
    }
    pub async fn create_alert_rule(&self, rule_type: String, threshold_minutes: SwiftInt, enabled: bool, escalation_minutes: Option<SwiftInt>, escalate_email: Option<bool>) -> Result<AlertRuleEnvelope, ApiError> {
        call!(self, |i| i.post("/api/alert-rules", json!({ "rule_type": rule_type, "threshold_minutes": threshold_minutes, "enabled": enabled, "escalation_minutes": escalation_minutes, "escalate_email": escalate_email })).await)
    }
    pub async fn set_alert_rule_enabled(&self, rule_id: SwiftInt, enabled: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/alert-rules/{}/update", rule_id.0), json!({ "enabled": enabled })).await)
    }
    pub async fn delete_alert_rule(&self, rule_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/alert-rules/{}/delete", rule_id.0), json!({})).await)
    }
    pub async fn audit_log(&self, limit: SwiftInt, action: Option<String>) -> Result<AuditLogEnvelope, ApiError> {
        call!(self, |i| i.get("/api/audit-log", &[("limit", Some(limit.0.to_string())), ("action", action.filter(|a| !a.trim().is_empty()))]).await)
    }
    pub async fn revert_audit_entry(&self, entry_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post(&format!("/api/audit-log/{}/revert", entry_id.0), json!({})).await)
    }
    pub async fn exports(&self) -> Result<ExportsResponse, ApiError> {
        call!(self, |i| i.get("/api/exports", &[]).await)
    }
    pub async fn download_export(&self, date: String, file_name: String) -> Result<Vec<u8>, ApiError> {
        call!(self, |i| crate::helpers::download(&i, &format!("/api/exports/{}/{}", urlencoding::encode(&date), urlencoding::encode(&file_name))).await)
    }

    pub async fn swarm_accounts(&self, query: String) -> Result<SwarmAccountsAdminEnvelope, ApiError> {
        call!(self, |i| i.get("/api/swarm-accounts/admin", &[("query", Some(query)), ("limit", opt("100"))]).await)
    }
    pub async fn set_account_verified(&self, account_id: SwiftInt, verified: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/swarm-accounts/email-verified", json!({ "account_id": account_id, "verified": verified })).await)
    }
    pub async fn set_account_moderator(&self, account_id: SwiftInt, moderator: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/swarm-accounts/moderator", json!({ "account_id": account_id, "moderator": moderator })).await)
    }
    pub async fn reset_account_password(&self, account_id: SwiftInt, new_password: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| {
            if new_password.chars().count() < 8 {
                return Err(ApiError::invalid("The new password must be at least 8 characters."));
            }
            i.post("/api/swarm-accounts/reset-password", json!({ "account_id": account_id, "new_password": new_password })).await
        })
    }
    pub async fn delete_account(&self, account_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/swarm-accounts/delete", json!({ "account_id": account_id })).await)
    }
    pub async fn update_account(&self, account_id: SwiftInt, username: String, display_name: String, email: String, guild_id: String, server_name: String, public_profile: bool) -> Result<SwarmAccountUpdateResponse, ApiError> {
        call!(self, |i| i.post("/api/swarm-accounts/update", json!({ "account_id": account_id, "username": username, "display_name": display_name, "email": email, "guild_id": guild_id, "server_name": server_name, "public_profile": public_profile })).await)
    }
    pub async fn resend_account_verification(&self, account_id: SwiftInt) -> Result<SwarmAccountResendVerificationResponse, ApiError> {
        call!(self, |i| i.post("/api/swarm-accounts/resend-verification", json!({ "account_id": account_id })).await)
    }
    pub async fn bulk_delete_accounts(&self, ids: Vec<SwiftInt>) -> Result<SwarmAccountBulkResult, ApiError> {
        call!(self, |i| i.post("/api/swarm-accounts/bulk-delete", json!({ "ids": ids })).await)
    }
    pub async fn bulk_verify_accounts(&self, ids: Vec<SwiftInt>, verified: bool) -> Result<SwarmAccountBulkResult, ApiError> {
        call!(self, |i| i.post("/api/swarm-accounts/bulk-verify", json!({ "ids": ids, "verified": verified })).await)
    }

    pub async fn lumisound_admin(&self) -> Result<LumisoundAdminEnvelope, ApiError> {
        call!(self, |i| i.get("/api/lumisound/admin", &[]).await)
    }
    pub async fn set_lumisound_user_active(&self, user_id: SwiftInt, active: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/lumisound/users/active", json!({ "user_id": user_id, "active": active })).await)
    }
    pub async fn delete_lumisound_upload(&self, upload_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/lumisound/uploads/delete", json!({ "upload_id": upload_id })).await)
    }
    pub async fn set_lumisound_bug_status(&self, report_id: SwiftInt, status: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/lumisound/bug-reports/status", json!({ "report_id": report_id, "status": status })).await)
    }

    pub async fn gallery_admin(&self) -> Result<GalleryAdminEnvelope, ApiError> {
        call!(self, |i| i.get("/api/image-gallery/admin", &[]).await)
    }
    pub async fn update_gallery_media(&self, media_id: SwiftInt, moderation_status: Option<String>, moderation_reason: Option<String>, is_adult: Option<bool>) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/media/update", json!({ "media_id": media_id, "moderation_status": moderation_status, "moderation_reason": moderation_reason, "is_adult": is_adult })).await)
    }
    pub async fn delete_gallery_media(&self, media_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/media/delete", json!({ "media_id": media_id })).await)
    }
    pub async fn delete_gallery_comment(&self, comment_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/comments/delete", json!({ "comment_id": comment_id })).await)
    }
    pub async fn set_gallery_report_status(&self, report_id: SwiftInt, status: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/reports/status", json!({ "report_id": report_id, "status": status })).await)
    }
    /// kind: "media" | "comments" | "users"
    pub async fn gallery_bulk_delete(&self, kind: String, ids: Vec<SwiftInt>) -> Result<OkResponse, ApiError> {
        call!(self, |i| {
            if !matches!(kind.as_str(), "media" | "comments" | "users") {
                return Err(ApiError::invalid("Unknown bulk delete kind."));
            }
            i.post(&format!("/api/image-gallery/admin/{kind}/bulk-delete"), json!({ "ids": ids })).await
        })
    }
    pub async fn set_gallery_user_email_verified(&self, user_id: SwiftInt, verified: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/users/email-verified", json!({ "user_id": user_id, "verified": verified })).await)
    }
    pub async fn set_gallery_user_age_verified(&self, user_id: SwiftInt, verified: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/users/age-verified", json!({ "user_id": user_id, "verified": verified })).await)
    }
    pub async fn delete_gallery_user(&self, user_id: SwiftInt) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/users/delete", json!({ "user_id": user_id })).await)
    }
    pub async fn resend_gallery_user_verification(&self, user_id: SwiftInt) -> Result<GalleryUserResendVerificationResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/users/resend-verification", json!({ "user_id": user_id })).await)
    }
    pub async fn reset_gallery_user_password(&self, user_id: SwiftInt, new_password: String) -> Result<OkResponse, ApiError> {
        call!(self, |i| {
            if new_password.chars().count() < 8 {
                return Err(ApiError::invalid("New password must be at least 8 characters."));
            }
            i.post("/api/image-gallery/users/reset-password", json!({ "user_id": user_id, "new_password": new_password })).await
        })
    }
    pub async fn update_gallery_user(&self, user_id: SwiftInt, username: String, display_name: String, email: String, public_profile: bool) -> Result<OkResponse, ApiError> {
        call!(self, |i| i.post("/api/image-gallery/users/update", json!({ "user_id": user_id, "username": username.trim(), "display_name": display_name.trim(), "email": email.trim(), "public_profile": public_profile })).await)
    }
    /// kind: "media" | "users"
    pub async fn gallery_export_csv(&self, kind: String) -> Result<Vec<u8>, ApiError> {
        call!(self, |i| {
            if !matches!(kind.as_str(), "media" | "users") {
                return Err(ApiError::invalid("Unknown export."));
            }
            crate::helpers::download(&i, &format!("/api/image-gallery/admin/{kind}/export.csv")).await
        })
    }

    pub async fn schemas(&self) -> Result<SchemasResponse, ApiError> {
        call!(self, |i| crate::helpers::schemas(&i).await)
    }
    pub async fn table_data(&self, schema: String, table: String, limit: SwiftInt) -> Result<TableData, ApiError> {
        call!(self, |i| crate::helpers::table_data(&i, &schema, &table, limit.0).await)
    }
    pub async fn table_csv(&self, schema: String, table: String) -> Result<Vec<u8>, ApiError> {
        call!(self, |i| crate::helpers::download(&i, &format!("/api/database/data?schema_name={}&table_name={}&limit=1000&format=csv", urlencoding::encode(&schema), urlencoding::encode(&table))).await)
    }
    pub async fn truncate_table(&self, schema: String, table: String, confirm_text: String, owner_confirm_text: String) -> Result<TruncateResponse, ApiError> {
        call!(self, |i| i.post("/api/database/truncate-table", json!({ "schema_name": schema, "table_name": table, "confirm_text": confirm_text, "owner_confirm_text": owner_confirm_text })).await)
    }
    pub async fn truncate_schema(&self, schema: String, confirm_text: String, owner_confirm_text: String) -> Result<TruncateResponse, ApiError> {
        call!(self, |i| i.post("/api/database/truncate-schema", json!({ "schema_name": schema, "confirm_text": confirm_text, "owner_confirm_text": owner_confirm_text })).await)
    }
}

#[derive(uniffi::Record, Clone)]
pub struct CachedDashboard {
    pub dashboard: DashboardResponse,
    /// Unix seconds when it was saved.
    pub saved_at: f64,
}
