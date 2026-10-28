//! Shared state + the auth/scoping helpers from routes.lua.

use crate::auth::{self, Auth, IssueOpts};
use crate::config::Settings;
use crate::db::Db;
use crate::http::{Reply, Req, R};
use crate::telemetry::{Event, Telemetry};
use regex::Regex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub type AppRef = Arc<App>;

pub const SESSION_COOKIE: &str = "swarm_session";
pub const OWNER_SCOPE: &str = "__site_owner_only__";

pub struct App {
    pub settings: Arc<Settings>,
    pub db: Arc<Db>,
    pub telemetry: Telemetry,
    pub http: reqwest::Client,
    pub origin_regex: Option<Regex>,
    pub limiter: crate::ratelimit::Limiter,
    pub started: Instant,
    pub requests: AtomicI64,
    presence: Mutex<HashMap<String, Instant>>,
    token_rejects: Mutex<HashMap<String, i64>>,
    pub cache: crate::cache::Caches,
}

impl App {
    pub fn new(settings: Settings) -> AppRef {
        let settings = Arc::new(settings);
        let db = Arc::new(Db::new(settings.clone()));
        let origin_regex = Some(settings.cors_allow_origin_regex.clone()).filter(|r| !r.is_empty()).and_then(|r| Regex::new(&format!("^(?:{r})$")).ok());
        Arc::new(Self {
            telemetry: Telemetry::start(db.clone()),
            http: reqwest::Client::builder().pool_idle_timeout(std::time::Duration::from_secs(90)).timeout(std::time::Duration::from_secs(20)).build().expect("http client"),
            origin_regex,
            limiter: Default::default(),
            started: Instant::now(),
            requests: AtomicI64::new(0),
            presence: Mutex::default(),
            token_rejects: Mutex::default(),
            cache: Default::default(),
            settings,
            db,
        })
    }

    pub fn count_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }
    pub fn pop_requests(&self) -> i64 {
        self.requests.swap(0, Ordering::Relaxed)
    }

    pub fn token_ttl(&self, device: bool) -> i64 {
        if device { self.settings.device_token_ttl_seconds } else { self.settings.api_token_ttl_seconds }
    }

    pub fn session_cookie(&self, token: &str) -> String {
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{}; Max-Age={}", if self.settings.cookie_secure { "; Secure" } else { "" }, self.settings.api_token_ttl_seconds)
    }
    pub fn clear_cookie(&self) -> String {
        format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax{}; Max-Age=0", if self.settings.cookie_secure { "; Secure" } else { "" })
    }

    pub fn issue(&self, username: &str, o: IssueOpts) -> String {
        auth::issue_api_token(&self.settings.session_secret, username, o)
    }

    pub fn record_token_rejected(&self, path: &str, ip: &str, reason: &str, via: &str) {
        let key = format!("{ip}:{reason}");
        let now = crate::util::now();
        {
            let mut m = self.token_rejects.lock().unwrap();
            if m.get(&key).is_some_and(|t| *t > now - 60) {
                return;
            }
            m.insert(key, now);
            if m.len() > 500 {
                m.retain(|_, t| *t > now - 60);
            }
        }
        self.telemetry.record(Event::new("auth", "token_rejected").path(path).ip(ip).text(reason).meta(serde_json::json!({ "reason": reason, "via": via })));
    }

    /// Throttled last_seen_at refresh (one UPDATE per ~30s per account).
    pub async fn touch_presence(self: &AppRef, a: &Auth) {
        let username = a.username.trim().to_string();
        let Some(g) = a.guild_id.clone() else { return };
        if username.is_empty() {
            return;
        }
        let key = format!("{}\0{g}", username.to_lowercase());
        {
            let mut m = self.presence.lock().unwrap();
            if m.get(&key).is_some_and(|t| t.elapsed().as_secs() < 30) {
                return;
            }
            m.insert(key, Instant::now());
        }
        crate::accounts::touch_account_seen(self, &username, &g, 30).await;
    }
}

// ---- auth on requests -------------------------------------------------------------------------
pub fn is_gallery_owner(a: &Auth) -> bool {
    a.is_admin() && a.site_owner
}

/// None = unrestricted (admin); Some(OWNER_SCOPE) = signed in without a guild (sees nothing).
pub fn scoped_guild_id(a: &Auth) -> Option<String> {
    if !a.site_owner && a.guild_id.is_none() {
        return Some(OWNER_SCOPE.into());
    }
    if a.is_admin() {
        return None;
    }
    a.guild_id.clone()
}

pub fn account_guild_id(a: &Auth) -> Option<String> {
    a.guild_id.clone()
}

pub fn client_name(v: Option<String>) -> Option<String> {
    let c = v.unwrap_or_default().to_lowercase();
    matches!(c.as_str(), "web" | "ios" | "tvos" | "widget" | "linux").then_some(c)
}

impl Req {
    pub fn token(&self) -> Option<(String, &'static str)> {
        if let Some(t) = auth::bearer(self.header("authorization")) {
            return Some((t, "bearer"));
        }
        self.cookies.get(SESSION_COOKIE).filter(|t| !t.is_empty()).map(|t| (t.clone(), "cookie"))
    }

    pub fn auth(&self) -> Option<Auth> {
        let (token, via) = self.token()?;
        match auth::verify_api_token(&self.app.settings.session_secret, &token) {
            Ok(a) => Some(a),
            Err(reason) => {
                self.app.record_token_rejected(&self.path, &self.client_ip, reason, via);
                None
            }
        }
    }

    pub async fn require_auth(&self) -> R<Auth> {
        let a = self.auth().ok_or_else(|| Reply::err(401, "Authentication required"))?;
        self.app.touch_presence(&a).await;
        Ok(a)
    }

    pub async fn require_admin(&self) -> R<Auth> {
        let a = self.require_auth().await?;
        if !a.is_admin() {
            return Err(Reply::err(403, "Admin access required"));
        }
        Ok(a)
    }

    pub async fn require_admin_or_mod(&self) -> R<Auth> {
        let a = self.require_auth().await?;
        if !(a.is_admin() || a.moderator) {
            return Err(Reply::err(403, "Admin or moderator access required"));
        }
        Ok(a)
    }

    pub async fn require_gallery_owner(&self) -> R<Auth> {
        let a = self.require_auth().await?;
        if !is_gallery_owner(&a) {
            return Err(Reply::err(403, "Image Gallery owner access required"));
        }
        Ok(a)
    }

    pub async fn require_gallery_admin_or_mod(&self) -> R<Auth> {
        let a = self.require_auth().await?;
        if !(is_gallery_owner(&a) || a.moderator) {
            return Err(Reply::err(403, "Image Gallery admin or moderator access required"));
        }
        Ok(a)
    }

    /// Page routes: 303 to /login instead of a 401.
    pub async fn require_auth_page(&self) -> R<Auth> {
        let a = self.auth().ok_or_else(|| Reply::redirect(&format!("/login?next={}", urlencoding::encode(&self.path))))?;
        self.app.touch_presence(&a).await;
        Ok(a)
    }

    /// The "api-read" per-account limiter (60/min).
    pub fn read_limit(&self, a: &Auth) -> R<()> {
        self.app.limiter.check(&format!("api-read:{}", a.username.to_lowercase()), 60, 60)
    }

    pub fn require_guild_scope(&self, a: &Auth, guild_id: &str) -> R<()> {
        match scoped_guild_id(a) {
            Some(s) if s != guild_id => Err(Reply::err(403, "This account can only control its registered guild")),
            _ => Ok(()),
        }
    }
}

/// Account row id for this token's (username, guild).
pub async fn account_id_for_auth(app: &AppRef, a: &Auth) -> Result<i64, String> {
    let username = a.username.trim();
    let Some(g) = a.guild_id.as_deref() else { return Err("Guild account access required".into()) };
    if username.is_empty() {
        return Err("Guild account access required".into());
    }
    use crate::db::J;
    crate::accounts::get_account_profile(app, username, g).await.and_then(|p| p.i("id")).ok_or_else(|| "Account profile not found".into())
}

impl Reply {
    pub fn by(mut self, a: &Auth) -> Self {
        self.who = Some((a.username.clone(), a.guild_id.clone()));
        self
    }
}
