//! Request/response plumbing (port of httpd.lua's contract): `Req` carries
//! what the Lua handlers got (query, params, headers, cookies, JSON body,
//! client IP); `Reply` is (status, body, headers). CORS + telemetry run as
//! middleware.

use crate::app::{App, AppRef};
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, FromRequest, MatchedPath, RawPathParams, Request};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;

pub struct Req {
    pub app: AppRef,
    pub method: Method,
    pub path: String,
    pub query: HashMap<String, String>,
    pub params: HashMap<String, String>,
    pub headers: HeaderMap,
    pub cookies: HashMap<String, String>,
    pub raw_body: Bytes,
    pub json: Value,
    pub client_ip: String,
}

fn url_decode(s: &str) -> String {
    urlencoding::decode(&s.replace('+', " ")).map(|c| c.into_owned()).unwrap_or_else(|_| s.to_string())
}

pub fn parse_query(qs: &str) -> HashMap<String, String> {
    qs.split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (!k.is_empty()).then(|| (url_decode(k), url_decode(v)))
        })
        .collect()
}

fn parse_cookies(h: Option<&str>) -> HashMap<String, String> {
    h.map(|h| h.split(';').filter_map(|p| p.trim().split_once('=').map(|(k, v)| (k.trim().to_string(), url_decode(v.trim())))).collect()).unwrap_or_default()
}

pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| peer.map(|p| p.ip().to_string()))
        .unwrap_or_else(|| "unknown".into())
}

impl<S: Send + Sync> FromRequest<S> for Req {
    type Rejection = Response;
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let (mut parts, body) = req.into_parts();
        let app = parts.extensions.get::<AppRef>().cloned().expect("app extension");
        let params: HashMap<String, String> = match <RawPathParams as axum::extract::FromRequestParts<S>>::from_request_parts(&mut parts, state).await {
            Ok(p) => p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            Err(_) => HashMap::new(),
        };
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
        let raw_body = axum::body::to_bytes(body, 12 * 1024 * 1024).await.map_err(|_| Reply::err(413, "Request body too large").into_response())?;
        let is_json = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("application/json"));
        let json = if is_json && !raw_body.is_empty() { serde_json::from_slice(&raw_body).unwrap_or_else(|_| json!({})) } else { json!({}) };
        Ok(Req {
            client_ip: client_ip(&parts.headers, peer),
            cookies: parse_cookies(parts.headers.get(header::COOKIE).and_then(|v| v.to_str().ok())),
            query: parse_query(parts.uri.query().unwrap_or("")),
            path: parts.uri.path().to_string(),
            method: parts.method,
            headers: parts.headers,
            params,
            raw_body,
            json,
            app,
        })
    }
}

impl Req {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers.get(k).and_then(|v| v.to_str().ok())
    }
    pub fn q(&self, k: &str) -> Option<String> {
        self.query.get(k).cloned().filter(|v| !v.is_empty())
    }
    pub fn p(&self, k: &str) -> String {
        self.params.get(k).cloned().unwrap_or_default()
    }
    /// Body field as a string (numbers stringified), None for null/missing.
    pub fn js(&self, k: &str) -> Option<String> {
        match self.json.get(k)? {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }
    pub fn jb(&self, k: &str) -> bool {
        self.json.get(k) == Some(&Value::Bool(true))
    }
    pub fn ji(&self, k: &str) -> Option<i64> {
        match self.json.get(k)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    /// Form-encoded body (server-rendered <form method=post>).
    pub fn form(&self) -> HashMap<String, String> {
        parse_query(&String::from_utf8_lossy(&self.raw_body))
    }
}

// ---- replies -------------------------------------------------------------------------------
pub enum Payload {
    Json(Value),
    Html(String),
    Bytes(Vec<u8>, String),
    Empty,
}

pub struct Reply {
    pub status: u16,
    pub payload: Payload,
    pub headers: Vec<(String, String)>,
    pub who: Option<(String, Option<String>)>,
}

#[derive(Clone)]
pub struct Who(pub String, pub Option<String>);

impl Reply {
    pub fn json(status: u16, v: Value) -> Self {
        Self { status, payload: Payload::Json(v), headers: Vec::new(), who: None }
    }
    pub fn ok(v: Value) -> Self {
        Self::json(200, v)
    }
    pub fn err(status: u16, detail: impl Into<String>) -> Self {
        Self::json(status, json!({ "detail": detail.into() }))
    }
    pub fn html(body: String) -> Self {
        Self { status: 200, payload: Payload::Html(body), headers: Vec::new(), who: None }
    }
    pub fn bytes(status: u16, body: Vec<u8>, ctype: &str) -> Self {
        Self { status, payload: Payload::Bytes(body, ctype.to_string()), headers: Vec::new(), who: None }
    }
    pub fn redirect(to: &str) -> Self {
        Self { status: 303, payload: Payload::Empty, headers: vec![("Location".into(), to.into())], who: None }
    }
    pub fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }
    pub fn cookie(self, v: String) -> Self {
        self.header("Set-Cookie", v)
    }
    pub fn status(mut self, s: u16) -> Self {
        self.status = s;
        self
    }
}

impl IntoResponse for Reply {
    fn into_response(self) -> Response {
        let (body, ctype) = match self.payload {
            Payload::Json(v) => (Body::from(v.to_string()), "application/json".to_string()),
            Payload::Html(h) => (Body::from(h), "text/html; charset=utf-8".to_string()),
            Payload::Bytes(b, c) => (Body::from(b), c),
            Payload::Empty => (Body::empty(), "text/plain".to_string()),
        };
        let mut r = Response::new(body);
        *r.status_mut() = StatusCode::from_u16(self.status).unwrap_or(StatusCode::OK);
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(&ctype).unwrap_or(HeaderValue::from_static("application/octet-stream")));
        for (k, v) in self.headers {
            if let (Ok(k), Ok(v)) = (header::HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(&v)) {
                r.headers_mut().append(k, v);
            }
        }
        if let Some((u, g)) = self.who {
            r.extensions_mut().insert(Who(u, g));
        }
        r
    }
}

/// `?`-style early return for handlers: Err carries the Reply.
pub type R<T> = Result<T, Reply>;

// ---- middleware --------------------------------------------------------------------------------
fn origin_allowed(app: &App, origin: &str, host: Option<&str>) -> bool {
    if origin.is_empty() {
        return false;
    }
    if let Some(h) = host {
        let oh = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://")).unwrap_or("");
        if !oh.is_empty() && !oh.contains('/') && oh.eq_ignore_ascii_case(h) {
            return true;
        }
    }
    if app.settings.cors_allowed_origins.iter().any(|o| o == origin) {
        return true;
    }
    app.origin_regex.as_ref().is_some_and(|r| r.is_match(origin))
}

pub fn ws_origin_ok(app: &App, headers: &HeaderMap) -> bool {
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        None | Some("") => true,
        Some(o) => origin_allowed(app, o, headers.get(header::HOST).and_then(|v| v.to_str().ok())),
    }
}

pub async fn middleware(axum::extract::State(app): axum::extract::State<AppRef>, mut req: Request, next: Next) -> Response {
    let start = Instant::now();
    app.count_request();
    req.extensions_mut().insert(app.clone());
    let origin = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).map(str::to_string);
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = client_ip(req.headers(), peer);
    let is_ws = req.headers().get(header::UPGRADE).is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));

    let mut resp = if method == Method::OPTIONS {
        Response::builder().status(204).body(Body::empty()).unwrap()
    } else {
        next.run(req).await
    };
    let h = resp.headers_mut();
    if origin_allowed(&app, &origin, host.as_deref()) {
        h.insert("Access-Control-Allow-Origin", HeaderValue::from_str(&origin).unwrap_or(HeaderValue::from_static("null")));
        h.insert("Access-Control-Allow-Credentials", HeaderValue::from_static("true"));
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    h.insert("Access-Control-Allow-Methods", HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"));
    h.insert("Access-Control-Allow-Headers", HeaderValue::from_static("Authorization, Content-Type"));
    h.insert(header::SERVER, HeaderValue::from_static("SwarmPanel-Rust"));
    if method == Method::OPTIONS || is_ws {
        return resp;
    }
    let status = resp.status().as_u16();
    let who = resp.extensions().get::<Who>().cloned();
    let pattern = resp.extensions().get::<Pattern>().map(|p| p.0.clone());
    let name = if status == 404 && pattern.is_none() { "404".to_string() } else { pattern.unwrap_or_else(|| path.clone()) };
    let mut ev = crate::telemetry::Event::new("http", &name);
    ev.method = Some(method.to_string());
    ev.path = Some(path);
    ev.status_code = Some(status as i32);
    ev.client_ip = Some(ip);
    ev.numeric_value = Some(start.elapsed().as_secs_f64() * 1000.0);
    if let Some(Who(u, g)) = who {
        ev.username = Some(u);
        ev.guild_id = g;
    }
    app.telemetry.record(ev);
    resp
}

#[derive(Clone)]
pub struct Pattern(pub String);

/// route_layer: runs after routing, so the matched pattern is known.
pub async fn tag_pattern(req: Request, next: Next) -> Response {
    let pat = req.extensions().get::<MatchedPath>().map(|m| m.as_str().to_string());
    let mut resp = next.run(req).await;
    if let Some(p) = pat {
        resp.extensions_mut().insert(Pattern(p));
    }
    resp
}

pub async fn not_found() -> Reply {
    Reply::err(404, "Not found")
}
