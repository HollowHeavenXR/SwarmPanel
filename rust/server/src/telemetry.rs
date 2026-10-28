//! Panel telemetry (port of telemetry.lua): sanitized events into
//! swarmpanel_telemetry_events (batched every 2s), a process snapshot row.

use crate::db::{Db, P};
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::sync::mpsc;

const SECRET_FRAGMENTS: &[&str] = &["token", "password", "passwd", "secret", "authorization", "api_key", "apikey", "cookie", "credential", "access_key", "private_key", "client_secret", "webhook", "dsn", "auth"];
static TOKENISH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([\w-]{15,60})\.([\w-]{4,20})\.([\w-]{20,80})").unwrap());
static BEARER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[Bb]earer\s+\S+").unwrap());

pub fn redact(s: &str) -> String {
    let s = TOKENISH.replace_all(s, "[redacted-token-shaped]");
    let s = BEARER.replace_all(&s, "Bearer [redacted]").into_owned();
    if s.len() > 500 { format!("{}...[truncated]", crate::util::cut(&s, 500)) } else { s }
}

fn sanitize(v: &Value, depth: usize) -> Option<Value> {
    match v {
        Value::String(s) => Some(Value::String(redact(s))),
        Value::Number(_) | Value::Bool(_) => Some(v.clone()),
        Value::Object(m) if depth < 4 => {
            let mut out = Map::new();
            for (k, val) in m {
                let lk = k.to_lowercase();
                if SECRET_FRAGMENTS.iter().any(|f| lk.contains(f)) {
                    continue;
                }
                if let Some(sv) = sanitize(val, depth + 1) {
                    out.insert(k.clone(), sv);
                }
            }
            Some(Value::Object(out))
        }
        Value::Array(a) if depth < 4 => Some(Value::Array(a.iter().filter_map(|x| sanitize(x, depth + 1)).collect())),
        _ => None,
    }
}

#[derive(Default, Debug)]
pub struct Event {
    pub category: String,
    pub name: String,
    pub method: Option<String>,
    pub path: Option<String>,
    pub status_code: Option<i32>,
    pub username: Option<String>,
    pub guild_id: Option<String>,
    pub client_ip: Option<String>,
    pub numeric_value: Option<f64>,
    pub text_value: Option<String>,
    pub metadata: Option<Value>,
}

impl Event {
    pub fn new(category: &str, name: &str) -> Self {
        Self { category: category.into(), name: name.into(), ..Default::default() }
    }
    pub fn user(mut self, u: Option<&str>) -> Self {
        self.username = u.map(str::to_string);
        self
    }
    pub fn guild(mut self, g: Option<&str>) -> Self {
        self.guild_id = g.map(str::to_string);
        self
    }
    pub fn ip(mut self, ip: &str) -> Self {
        self.client_ip = Some(ip.to_string());
        self
    }
    pub fn path(mut self, p: &str) -> Self {
        self.path = Some(p.to_string());
        self
    }
    pub fn num(mut self, n: Option<f64>) -> Self {
        self.numeric_value = n;
        self
    }
    pub fn text(mut self, t: impl Into<String>) -> Self {
        self.text_value = Some(t.into());
        self
    }
    pub fn meta(mut self, m: Value) -> Self {
        self.metadata = Some(m);
        self
    }
}

pub struct Telemetry {
    tx: mpsc::UnboundedSender<Event>,
    pub ws_active: AtomicI64,
}

impl Telemetry {
    pub fn start(db: Arc<Db>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
        tokio::spawn(async move {
            let _ = db
                .batch(
                    "accountlogins",
                    "CREATE TABLE IF NOT EXISTS swarmpanel_telemetry_events (id BIGSERIAL PRIMARY KEY, event_category VARCHAR(40) NOT NULL, event_name VARCHAR(120) NOT NULL, method VARCHAR(10), path VARCHAR(200), status_code INTEGER, username VARCHAR(80), guild_id BIGINT, client_ip VARCHAR(64), numeric_value DOUBLE PRECISION, text_value TEXT, metadata JSONB, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
                     CREATE INDEX IF NOT EXISTS swarmpanel_telemetry_events_category_idx ON swarmpanel_telemetry_events (event_category, event_name, created_at);
                     CREATE INDEX IF NOT EXISTS swarmpanel_telemetry_events_created_idx ON swarmpanel_telemetry_events (created_at);
                     CREATE INDEX IF NOT EXISTS swarmpanel_telemetry_events_username_idx ON swarmpanel_telemetry_events (username, created_at);
                     CREATE INDEX IF NOT EXISTS swarmpanel_telemetry_events_path_idx ON swarmpanel_telemetry_events (path, created_at);
                     CREATE TABLE IF NOT EXISTS swarmpanel_process_stats (id INTEGER PRIMARY KEY DEFAULT 1, memory_kb BIGINT, uptime_seconds BIGINT, active_websocket_count INTEGER, requests_last_interval INTEGER, updated_at TIMESTAMPTZ NOT NULL DEFAULT now());",
                )
                .await;
            let mut buf: Vec<Event> = Vec::new();
            loop {
                let first = rx.recv().await;
                let Some(first) = first else { break };
                buf.push(first);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                while let Ok(e) = rx.try_recv() {
                    if buf.len() < 5000 {
                        buf.push(e);
                    }
                }
                for chunk in buf.chunks(200) {
                    let mut sql = String::from("INSERT INTO swarmpanel_telemetry_events (event_category, event_name, method, path, status_code, username, guild_id, client_ip, numeric_value, text_value, metadata) VALUES ");
                    let mut params: Vec<P> = Vec::with_capacity(chunk.len() * 11);
                    for (i, e) in chunk.iter().enumerate() {
                        let b = i * 11;
                        if i > 0 {
                            sql.push_str(", ");
                        }
                        sql.push_str(&format!("(${},${},${},${},${},${},${},${},${},${},${}::jsonb)", b + 1, b + 2, b + 3, b + 4, b + 5, b + 6, b + 7, b + 8, b + 9, b + 10, b + 11));
                        let meta = e.metadata.as_ref().and_then(|m| sanitize(m, 0)).filter(|m| m.to_string().len() <= 4000);
                        params.extend([
                            P::Text(crate::util::cut(&e.category, 40)),
                            P::Text(crate::util::cut(&e.name, 120)),
                            e.method.clone().into(),
                            e.path.as_deref().map(|p| crate::util::cut(p, 200)).into(),
                            e.status_code.into(),
                            e.username.as_deref().map(|u| crate::util::cut(u, 80)).into(),
                            e.guild_id.as_deref().and_then(|g| g.parse::<i64>().ok()).into(),
                            e.client_ip.as_deref().map(|c| crate::util::cut(c, 64)).into(),
                            e.numeric_value.into(),
                            e.text_value.as_deref().map(redact).into(),
                            meta.map(P::Json).unwrap_or(P::Null),
                        ]);
                    }
                    if let Err(err) = db.exec("accountlogins", &sql, &params).await {
                        println!("[swarmpanel] telemetry flush failed ({} rows dropped): {err}", chunk.len());
                    }
                }
                buf.clear();
            }
        });
        Self { tx, ws_active: AtomicI64::new(0) }
    }

    pub fn record(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    pub fn ws_connected(&self) {
        self.ws_active.fetch_add(1, Ordering::Relaxed);
    }
    pub fn ws_disconnected(&self) {
        self.ws_active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn memory_kb() -> i64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}
