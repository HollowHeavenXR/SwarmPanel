//! Settings (port of config.lua). Precedence: process env, then SwarmPanel/.env,
//! then ../Music/.env (only fills what's still missing).

use std::collections::HashMap;
use std::sync::OnceLock;

static DOTENV: OnceLock<HashMap<String, String>> = OnceLock::new();

fn read_dotenv(path: &str, out: &mut HashMap<String, String>) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let k = k.trim();
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut v = v.trim();
        if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
            v = &v[1..v.len() - 1];
        }
        out.entry(k.to_string()).or_insert_with(|| v.to_string());
    }
}

pub fn env(name: &str, default: &str) -> String {
    if let Ok(v) = std::env::var(name) {
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(v) = DOTENV.get().and_then(|d| d.get(name)).filter(|v| !v.is_empty()) {
        return v.clone();
    }
    default.to_string()
}
pub fn env_opt(name: &str) -> Option<String> {
    Some(env(name, "")).filter(|v| !v.is_empty())
}
pub fn env_bool(name: &str, default: bool) -> bool {
    match env_opt(name) {
        None => default,
        Some(v) => matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
    }
}
pub fn env_num<T: std::str::FromStr>(name: &str, default: T) -> T {
    env_opt(name).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}
pub fn env_csv(name: &str) -> Vec<String> {
    env(name, "").split(',').map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty()).collect()
}

#[derive(Clone, Debug)]
pub struct Bot {
    pub key: &'static str,
    pub display_name: &'static str,
    pub kind: &'static str, // "music" | "orchestrator"
    pub token_env: String,
    pub db_schema: String,
    pub accent: &'static str,
}

impl Bot {
    pub fn is_music(&self) -> bool {
        self.kind == "music"
    }
}

const MUSIC_BOTS: &[(&str, &str, &str)] = &[
    ("gws", "GWS", "#cba6f7"),
    ("maestro", "Maestro", "#a6e3a1"),
    ("melodic", "Melodic", "#fab387"),
    ("nexus", "Nexus", "#f38ba8"),
    ("rhythm", "Rhythm", "#94e2d5"),
    ("symphony", "Symphony", "#f9e2af"),
    ("tunestream", "Tunestream", "#b4befe"),
    ("alucard", "Alucard", "#e06c75"),
    ("sapphire", "Sapphire", "#4fc3f7"),
    ("strife", "Strife", "#ff6b6b"),
    ("lockhart", "Lockhart", "#f9a8d4"),
    ("glitch", "Glitch", "#00ff9f"),
    ("dazzle", "Dazzle", "#ffd700"),
];

#[derive(Clone, Debug)]
pub struct DbRoute {
    pub host: String,
    pub port: u16,
    pub database: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: String,
    pub db_pool_size_main: usize,
    pub db_pool_size: usize,
    pub db_default_schema: String,
    pub db_routes: HashMap<String, DbRoute>,
    pub admin_username: String,
    pub admin_password: String,
    pub session_secret: String,
    pub cookie_secure: bool,
    pub cors_allowed_origins: Vec<String>,
    pub cors_allow_origin_regex: String,
    pub api_token_ttl_seconds: i64,
    pub device_token_ttl_seconds: i64,
    pub pages_public_url: String,
    pub site_owner_email: String,
    pub owner_email_requires_verification: bool,
    pub api_max_rows: i64,
    pub bot_control_source_max_chars: usize,
    pub destructive_confirmation_phrase: String,
    pub scheduled_exports_enabled: bool,
    pub scheduled_exports_dir: String,
    pub port: u16,
    pub bind_host: String,
    pub panel_root: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub smtp_from_email: String,
    pub smtp_use_tls: bool,
    pub email_verification_ttl_seconds: i64,
    pub discord_bot_token: String,
    pub discord_dm_verification_ttl_seconds: i64,
    pub bots: Vec<Bot>,
    pub aria: Bot,
}

impl Settings {
    pub fn load() -> anyhow::Result<Self> {
        let root = std::env::var("PANEL_ROOT").unwrap_or_else(|_| "..".to_string());
        let mut d = HashMap::new();
        read_dotenv(&format!("{root}/.env"), &mut d);
        read_dotenv(&format!("{root}/../Music/.env"), &mut d);
        let _ = DOTENV.set(d);

        let bots = MUSIC_BOTS
            .iter()
            .map(|(k, n, a)| Bot {
                key: k,
                display_name: n,
                kind: "music",
                token_env: format!("{}_DISCORD_TOKEN", k.to_uppercase()),
                db_schema: env_opt(&format!("{}_DB_NAME", k.to_uppercase())).unwrap_or_else(|| format!("discord_music_{k}")),
                accent: a,
            })
            .collect();
        let aria = Bot { key: "aria", display_name: "Aria", kind: "orchestrator", token_env: "ARIA_DISCORD_TOKEN".into(), db_schema: "discord_aria".into(), accent: "#cba6f7" };

        let mut db_routes = HashMap::new();
        for item in env("PANEL_DB_ROUTES", "").split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()) {
            let Some((key, rest)) = item.split_once('=') else { continue };
            let (hostport, dbname) = rest.split_once('/').unwrap_or((rest, ""));
            let Some((host, port)) = hostport.rsplit_once(':') else { continue };
            let Ok(port) = port.parse() else { continue };
            db_routes.insert(key.to_string(), DbRoute { host: host.to_string(), port, database: Some(dbname.to_string()).filter(|s| !s.is_empty()) });
        }

        let smtp_from = env_opt("PANEL_SMTP_FROM_EMAIL").or_else(|| env_opt("SMTP_FROM_EMAIL")).unwrap_or_else(|| env("PANEL_SMTP_USERNAME", &env("SMTP_USERNAME", "")));
        let s = Self {
            db_host: env("PANEL_DB_HOST", &env("DB_HOST", "127.0.0.1")),
            db_port: env_num("PANEL_DB_PG_PORT", 5432),
            db_user: env("PANEL_DB_USER", &env("DB_USER", "botuser")),
            db_password: env("PANEL_DB_PASSWORD", &env("DB_PASSWORD", "")),
            db_pool_size_main: env_num("PANEL_DB_POOL_SIZE_MAIN", 6usize).max(1),
            db_pool_size: env_num("PANEL_DB_POOL_SIZE", 2usize).max(1),
            db_default_schema: env("PANEL_DB_DEFAULT_SCHEMA", "discord_music_gws"),
            db_routes,
            admin_username: env("PANEL_ADMIN_USERNAME", "admin"),
            admin_password: env("PANEL_ADMIN_PASSWORD", ""),
            session_secret: env("PANEL_SESSION_SECRET", ""),
            cookie_secure: env_bool("PANEL_COOKIE_SECURE", true),
            cors_allowed_origins: env_csv("PANEL_CORS_ALLOWED_ORIGINS"),
            cors_allow_origin_regex: env("PANEL_CORS_ALLOW_ORIGIN_REGEX", ""),
            api_token_ttl_seconds: env_num("PANEL_API_TOKEN_TTL_SECONDS", 43200),
            device_token_ttl_seconds: env_num("PANEL_DEVICE_TOKEN_TTL_SECONDS", 2_592_000),
            pages_public_url: env("PANEL_PAGES_PUBLIC_URL", ""),
            site_owner_email: env("SWARM_PANEL_SITE_OWNER_EMAIL", &env("IMAGE_GALLERY_OWNER_EMAIL", "")).to_lowercase(),
            owner_email_requires_verification: env_bool("SWARM_PANEL_OWNER_EMAIL_REQUIRES_VERIFICATION", true),
            api_max_rows: env_num("PANEL_API_MAX_ROWS", 200),
            bot_control_source_max_chars: env_num("PANEL_BOT_CONTROL_SOURCE_MAX_CHARS", 500),
            destructive_confirmation_phrase: env("PANEL_DESTRUCTIVE_CONFIRMATION_PHRASE", "I understand this permanently deletes SwarmPanel data"),
            scheduled_exports_enabled: env_bool("PANEL_SCHEDULED_EXPORTS_ENABLED", false),
            scheduled_exports_dir: env("PANEL_SCHEDULED_EXPORTS_DIR", &format!("{root}/.runtime/exports")),
            port: env_num("PORT", 8003),
            bind_host: env("PANEL_BIND_HOST", "127.0.0.1"),
            panel_root: root,
            smtp_host: env("PANEL_SMTP_HOST", &env("SMTP_HOST", "")),
            smtp_port: env_num("PANEL_SMTP_PORT", env_num("SMTP_PORT", 587)),
            smtp_username: env("PANEL_SMTP_USERNAME", &env("SMTP_USERNAME", "")),
            smtp_password: env("PANEL_SMTP_PASSWORD", &env("SMTP_PASSWORD", "")),
            smtp_from_email: smtp_from,
            smtp_use_tls: env_bool("PANEL_SMTP_USE_TLS", env_bool("SMTP_USE_TLS", true)),
            email_verification_ttl_seconds: env_num("PANEL_EMAIL_VERIFICATION_TTL_SECONDS", 86400i64).max(300),
            discord_bot_token: env("PANEL_DISCORD_BOT_TOKEN", ""),
            discord_dm_verification_ttl_seconds: env_num("PANEL_DISCORD_DM_VERIFICATION_TTL_SECONDS", 600i64).max(60),
            bots,
            aria,
        };
        if s.session_secret.is_empty() {
            anyhow::bail!("PANEL_SESSION_SECRET is required");
        }
        Ok(s)
    }

    pub fn bot(&self, key: &str) -> Option<&Bot> {
        self.bots.iter().find(|b| b.key == key).or_else(|| (key == "aria").then_some(&self.aria))
    }
    pub fn music_bot(&self, key: &str) -> Option<&Bot> {
        self.bots.iter().find(|b| b.key == key)
    }
    pub fn token(&self, key: &str) -> String {
        self.bot(key).map(|b| env(&b.token_env, "")).unwrap_or_default()
    }
}
