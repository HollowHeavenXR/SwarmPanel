//! Server-rendered pages (fresh design) + static assets.
//!
//! Every page renders complete HTML on the server. Live parts of a page are
//! "regions": an element with `data-src` (a /frag/... URL that re-renders
//! just that part) and `data-live` (the /ws key whose snapshots mean "this
//! changed"). The WASM module (web/) watches the keys, re-fetches regions,
//! and turns `data-api` forms / `data-post` buttons into API calls. See
//! web/src/lib.rs for the full attribute contract.

pub mod account;
pub mod admin;
pub mod community;
pub mod fleet;
pub mod insights;
pub mod ui;

use crate::app::{AppRef, SESSION_COOKIE};
use crate::auth::Auth;
use crate::db::J;
use crate::http::{Payload, R, Reply, Req};
use axum::Router;
use axum::routing::{get, post};
use maud::{Markup, PreEscaped, html};
use serde_json::Value;
use std::future::Future;
use std::sync::LazyLock;

// ---- information architecture ---------------------------------------------------------------
#[derive(Clone, Copy, PartialEq)]
pub enum When {
    Any,
    Admin,
    Mod,
    Gallery,
    Staff,
}

pub struct Item {
    pub to: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub when: When,
    pub badge: Option<&'static str>,
    pub blurb: &'static str,
}

pub struct Section {
    pub key: &'static str,
    pub label: &'static str,
    pub badge: Option<&'static str>,
    pub items: &'static [Item],
}

const fn it(to: &'static str, label: &'static str, icon: &'static str, when: When, blurb: &'static str) -> Item {
    Item { to, label, icon, when, badge: None, blurb }
}

pub static SECTIONS: &[Section] = &[
    Section {
        key: "fleet",
        label: "Fleet",
        badge: None,
        items: &[
            it("/", "Live wall", "grid", When::Any, "Every bot, live, at a glance."),
            it("/controls", "Controls", "play", When::Any, "Send orders to any bot in any guild."),
            it("/invites", "Invites", "link", When::Any, "Invite links for every bot."),
        ],
    },
    Section {
        key: "insights",
        label: "Insights",
        badge: None,
        items: &[
            it("/leaderboard", "Leaderboard", "trophy", When::Any, "Top tracks and listeners."),
            it("/learning", "Learning", "spark", When::Any, "The recommendation engine's memory."),
        ],
    },
    Section {
        key: "community",
        label: "People",
        badge: Some("community"),
        items: &[
            it("/users", "Directory", "users", When::Any, "Find other operators."),
            Item { to: "/friends", label: "Friends", icon: "smile", when: When::Any, badge: Some("friends"), blurb: "Requests and confirmed friends." },
            Item { to: "/messages", label: "Messages", icon: "mail", when: When::Any, badge: Some("messages"), blurb: "Direct messages." },
        ],
    },
    Section {
        key: "account",
        label: "Account",
        badge: None,
        items: &[
            it("/profile", "Profile", "user", When::Any, "How you appear across the panel."),
            it("/appearance", "Appearance", "palette", When::Any, "Theme, layout, and motion."),
            it("/other-projects", "Other Projects", "rocket", When::Any, "Other things I've built."),
        ],
    },
    Section {
        key: "admin",
        label: "Admin",
        badge: None,
        items: &[
            it("/admin", "Overview", "gear", When::Staff, "Every admin tool in one place."),
            it("/diagnostics", "Diagnostics", "pulse", When::Admin, "Stability, metrics, alert rules, and exports."),
            it("/intel", "Intel", "alert", When::Admin, "Trends, anomalies, and raw events."),
            it("/bot-telemetry", "Bot Telemetry", "antenna", When::Admin, "Raw diagnostic events from a single bot's own database."),
            it("/audit-log", "Audit Log", "list", When::Mod, "Every recorded admin action."),
            it("/accounts", "Accounts", "id", When::Admin, "Recover and manage swarm accounts."),
            it("/databases", "Databases", "db", When::Admin, "Browse raw schema tables."),
            it("/gallery-admin", "Gallery", "image", When::Gallery, "Image Gallery users, media, and reports."),
            it("/lumisound-admin", "Lumisound", "note", When::Mod, "Lumisound iOS app accounts and library."),
        ],
    },
];

pub fn can_see(a: &Auth, w: When) -> bool {
    let (admin, m, gallery) = (a.is_admin(), a.moderator, crate::app::is_gallery_owner(a));
    match w {
        When::Any => true,
        When::Admin => admin,
        When::Mod => admin || m,
        When::Gallery => gallery,
        When::Staff => admin || m || gallery,
    }
}

fn is_active(path: &str, to: &str) -> bool {
    let path = if path == "/dashboard" { "/" } else { path };
    if to == "/" {
        return path == "/";
    }
    path == to || path.starts_with(&format!("{to}/"))
}

/// (section, item) owning a path -- the longest matching prefix wins.
pub fn locate(path: &str) -> Option<(&'static Section, &'static Item)> {
    let mut best: Option<(&Section, &Item)> = None;
    for s in SECTIONS {
        for i in s.items {
            if is_active(path, i.to) && best.is_none_or(|(_, b)| i.to.len() > b.to.len()) {
                best = Some((s, i));
            }
        }
    }
    best
}

/// True when this session may open the screen at `path` (same gate as the sidebar).
pub fn allowed(a: &Auth, path: &str) -> bool {
    locate(path).is_some_and(|(_, i)| can_see(a, i.when))
}

// ---- request context --------------------------------------------------------------------------
pub struct Ctx {
    pub req: Req,
    pub a: Auth,
    pub prefs: Value,
}

impl Ctx {
    pub async fn new(req: Req) -> R<Ctx> {
        let a = req.require_auth_page().await?;
        let prefs = crate::accounts::get_panel_preferences(&req.app, &a.username, a.guild_id.as_deref()).await;
        Ok(Ctx { req, a, prefs })
    }
    /// Fragments: plain 401 instead of a redirect (the WASM side stops on it).
    pub async fn frag(req: Req) -> R<Ctx> {
        let a = req.require_auth().await?;
        Ok(Ctx { req, a, prefs: Value::Null })
    }
    pub fn app(&self) -> &AppRef {
        &self.req.app
    }
    pub fn q(&self, k: &str) -> Option<String> {
        self.req.q(k)
    }
    /// Guild this account is locked to (non-admin guild accounts), if any.
    pub fn own_guild(&self) -> Option<String> {
        (!self.a.is_admin()).then(|| self.a.guild_id.clone()).flatten()
    }

    /// Calls an API handler in-process with this request's credentials, so
    /// pages get exactly the JSON (and permission checks) the apps get.
    pub async fn api<F, Fut>(&self, f: F, query: &[(&str, String)], params: &[(&str, String)]) -> Result<Value, String>
    where
        F: FnOnce(Req) -> Fut,
        Fut: Future<Output = R<Reply>>,
    {
        call(&self.req, f, query, params, serde_json::json!({})).await
    }

    /// One /ws snapshot builder's output (same data the live socket pushes).
    pub async fn live(&self, key: &str, params: Value) -> Result<Value, String> {
        crate::ws::build(self.app(), key, &self.a, &params).await
    }
}

/// Copy of `req` with a different query string.
pub fn with_query(req: &Req, query: std::collections::HashMap<String, String>) -> Req {
    Req {
        app: req.app.clone(),
        method: req.method.clone(),
        path: req.path.clone(),
        query,
        params: req.params.clone(),
        headers: req.headers.clone(),
        cookies: req.cookies.clone(),
        raw_body: Default::default(),
        json: serde_json::json!({}),
        client_ip: req.client_ip.clone(),
    }
}

/// Runs an API handler in-process with `req`'s credentials and the given
/// query / path params / JSON body. Ok(json) on 2xx, Err(detail) otherwise.
pub async fn call<F, Fut>(req: &Req, f: F, query: &[(&str, String)], params: &[(&str, String)], body: Value) -> Result<Value, String>
where
    F: FnOnce(Req) -> Fut,
    Fut: Future<Output = R<Reply>>,
{
    let mut headers = req.headers.clone();
    headers.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("application/json"));
    let sub = Req {
        app: req.app.clone(),
        method: if body.as_object().is_some_and(|o| !o.is_empty()) { axum::http::Method::POST } else { axum::http::Method::GET },
        path: req.path.clone(),
        query: query.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.to_string(), v.clone())).collect(),
        params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        headers,
        cookies: req.cookies.clone(),
        raw_body: Default::default(),
        json: body,
        client_ip: req.client_ip.clone(),
    };
    let reply = match f(sub).await {
        Ok(r) | Err(r) => r,
    };
    match reply.payload {
        Payload::Json(v) if reply.status < 400 => Ok(v),
        Payload::Json(v) => Err(v.s("detail").unwrap_or_else(|| format!("HTTP {}", reply.status))),
        _ if reply.status < 400 => Ok(Value::Null),
        _ => Err(format!("HTTP {}", reply.status)),
    }
}

// ---- page shell -------------------------------------------------------------------------------
pub struct Page<'a> {
    pub path: &'a str,
    pub title: &'a str,
    pub lede: &'a str,
    pub actions: Option<Markup>,
    pub body: Markup,
}

impl<'a> Page<'a> {
    pub fn new(path: &'a str, title: &'a str, lede: &'a str, body: Markup) -> Self {
        Page { path, title, lede, actions: None, body }
    }
    pub fn actions(mut self, m: Markup) -> Self {
        self.actions = Some(m);
        self
    }
}

const PREF_ATTRS: &[&str] = &[
    "theme_mode", "background_mode", "layout_mode", "density", "card_shape", "font_scale", "motion", "roster_layout", "tab_style", "stream_card_style",
    "dashboard_density", "card_hover_effect", "notification_position", "panel_radius", "panel_font_family", "sidebar_style", "bot_card_detail", "operator_layout",
];

fn hex_ok(v: &str) -> bool {
    v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn css_url(v: &str) -> String {
    let v = v.trim();
    if v.starts_with("https://") || v.starts_with("http://") {
        format!("url(\"{}\")", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        "none".into()
    }
}

/// `<html>` attributes + CSS custom properties from the viewer's panel preferences.
pub fn pref_root(prefs: &Value) -> (Vec<(String, String)>, String) {
    let defaults = crate::profiles::default_panel_preferences();
    let get = |k: &str| prefs.s(k).filter(|v| !v.is_empty()).or_else(|| defaults.s(k)).unwrap_or_default();
    let mut attrs: Vec<(String, String)> = PREF_ATTRS.iter().map(|k| (format!("data-p-{k}"), get(k))).collect();
    for k in ["show_bot_uptime", "show_queue_pressure", "compact_sidebar"] {
        let on = match prefs.get(k).or_else(|| defaults.get(k)) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s == "true",
            _ => k != "compact_sidebar",
        };
        attrs.push((format!("data-p-{k}"), on.to_string()));
    }
    let accent = Some(get("accent_color")).filter(|v| hex_ok(v)).unwrap_or_else(|| "#89b4fa".into());
    let second = Some(get("accent_secondary")).filter(|v| hex_ok(v)).unwrap_or_else(|| crate::profiles::auto_secondary(&accent));
    let bg = Some(get("background_color")).filter(|v| hex_ok(v)).unwrap_or_else(|| "#0b0e18".into());
    let clamp = |k: &str, d: f64, lo: f64, hi: f64| prefs.f(k).unwrap_or(d).clamp(lo, hi);
    let image = if get("background_mode") == "custom_image" { css_url(&get("background_image_url")) } else { "none".into() };
    let style = format!(
        "--accent:{accent};--accent-2:{second};--accent-ink:{};--bg-custom:{bg};--bg-image:{image};--surface-alpha:{};--surface-blur:{}px;--backdrop-image:{};--backdrop-strength:{}%",
        crate::profiles::contrast_text(&accent),
        clamp("surface_opacity", 0.92, 0.35, 1.0),
        clamp("surface_blur", 18.0, 0.0, 36.0),
        css_url(&get("profile_backdrop_image_url")),
        clamp("profile_backdrop_strength", 0.18, 0.0, 0.55) * 100.0,
    );
    (attrs, style)
}

fn badge_slot(a: &Auth, name: Option<&str>) -> Markup {
    match name {
        Some(n) if a.guild_id.is_some() => {
            let field = if n == "community" { "unread_messages+pending_friend_requests" } else if n == "friends" { "pending_friend_requests" } else { "unread_messages" };
            html! { span.badge data-bind=(format!("community_counts:{field}")) hidden {} }
        }
        _ => html! {},
    }
}

/// Top command bar: one dropdown per section (only sections with visible items).
fn nav_menus(a: &Auth, path: &str) -> Markup {
    html! {
        @for s in SECTIONS {
            @let items: Vec<&Item> = s.items.iter().filter(|i| can_see(a, i.when)).collect();
            @if !items.is_empty() {
                @let here = items.iter().any(|i| is_active(path, i.to));
                details.navmenu {
                    summary.navtrigger.here[here] { (s.label) (badge_slot(a, s.badge)) svg.chev viewBox="0 0 12 12" aria-hidden="true" { path d="M3 4.5l3 3 3-3" {} } }
                    div.navpanel {
                        @for i in &items {
                            a.navitem.here[is_active(path, i.to)] href=(i.to) aria-current=[is_active(path, i.to).then_some("page")] {
                                span.navicon { (ui::icon(i.icon)) }
                                span.navcopy { strong { (i.label) (badge_slot(a, i.badge)) } small { (i.blurb) } }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Everything the ⌘K palette can jump to (rendered as JSON data for the WASM module).
fn palette_items(ctx: &Ctx) -> Value {
    let mut items: Vec<Value> = Vec::new();
    for s in SECTIONS {
        for i in s.items.iter().filter(|i| can_see(&ctx.a, i.when)) {
            items.push(serde_json::json!({ "label": i.label, "hint": s.label, "href": i.to, "keys": i.blurb }));
        }
    }
    for b in ctx.app().settings.bots.iter().filter(|b| b.is_music()) {
        items.push(serde_json::json!({ "label": format!("Control {}", b.display_name), "hint": "Bot", "href": format!("/controls?bot={}", b.key), "keys": format!("{} play pause skip queue", b.key) }));
    }
    for (label, href) in [("Sign out", "/logout#post"), ("Appearance settings", "/appearance")] {
        items.push(serde_json::json!({ "label": label, "hint": "Account", "href": href, "keys": "" }));
    }
    Value::Array(items)
}

const MOBILE_PRIMARY: &[(&str, &str, &str)] = &[("/", "Wall", "grid"), ("/controls", "Control", "play"), ("/users", "People", "users"), ("/messages", "Inbox", "mail")];

pub static ASSET_VERSION: LazyLock<String> = LazyLock::new(|| {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in APP_CSS.bytes().chain(WEB_JS.bytes()).chain(WEB_WASM.iter().copied()) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{:x}", h & 0xffff_ffff)
});

fn document(title: &str, prefs: &Value, token: Option<&str>, body: Markup) -> String {
    let (attrs, style) = pref_root(prefs);
    let v = ASSET_VERSION.as_str();
    let attr_str: String = attrs.iter().map(|(k, val)| format!(" {k}=\"{}\"", crate::util::esc(val))).collect();
    let head = html! {
        meta charset="utf-8";
        meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cut";
        meta name="color-scheme" content="dark light";
        meta name="theme-color" content="#07080c";
        title { (title) " · SwarmPanel" }
        link rel="icon" href="/favicon.svg" type="image/svg+xml";
        link rel="preconnect" href="https://fonts.googleapis.com";
        link rel="preconnect" href="https://fonts.gstatic.com" crossorigin;
        link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@400;500;600;700&family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;600&display=swap";
        link rel="stylesheet" href=(format!("/static/app.css?v={v}"));
        @if let Some(t) = token { meta name="sp-token" content=(t); }
        // wasm-bindgen's generated loader; the only script on the page.
        script type="module" { (PreEscaped(format!("import init from \"/static/web.js?v={v}\";init({{module_or_path:\"/static/web_bg.wasm?v={v}\"}});"))) }
    };
    format!("<!doctype html><html lang=\"en\"{attr_str} style=\"{}\"><head>{}</head>{}</html>", crate::util::esc(&style), head.into_string(), body.into_string())
}

pub fn render(ctx: &Ctx, p: Page) -> Reply {
    let a = &ctx.a;
    let section = locate(p.path).map(|(s, _)| s.label);
    let name = if a.username.is_empty() { "Operator".to_string() } else { a.username.clone() };
    let initial: String = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let body = html! {
        body {
            a.skip href="#main" { "Skip to content" }
            header.cmdbar {
                div.cmdbar-row {
                    a.logo href="/" aria-label="SwarmPanel home" { span.logo-mark { (ui::icon("hex")) } span.logo-word { "swarm" b { "panel" } } }
                    nav.navmenus aria-label="Main" { (nav_menus(a, p.path)) }
                    button.palette-btn type="button" data-palette-open aria-label="Search and commands" {
                        (ui::icon("search")) span { "Jump to…" } kbd { "⌘K" }
                    }
                    div.cmd-actions {
                        @if a.site_owner {
                            button.mode.on[a.is_admin()] type="button" data-post="/api/session/admin-mode" data-body=(format!("{{\"enabled\":{}}}", !a.is_admin())) data-done="reload" title="Toggle admin mode" {
                                span.mode-dot {} span.mode-label { @if a.is_admin() { "Admin" } @else { "Guild view" } }
                            }
                        }
                        @if a.guild_id.is_some() {
                            div.bell {
                                button.roundbtn type="button" data-toggle="#notif-panel" aria-label="Notifications" {
                                    (ui::icon("bell")) span.badge data-bind="notifications:unread_count" hidden {}
                                }
                                div.notif-panel.pop #notif-panel {
                                    div.pop-head { strong { "Notifications" } button.link-btn type="button" data-post="/api/notifications/read-all" data-done="refresh:#notif-list" { "Mark all read" } }
                                    div #notif-list data-src="/frag/notifications" data-live="notifications" data-lazy { div.empty { "Loading…" } }
                                }
                            }
                        }
                        details.navmenu.me-menu {
                            summary.me { span.avatar.sm { (initial) } span.me-name { (name) } }
                            div.navpanel.right {
                                a.navitem href="/profile" { span.navicon { (ui::icon("user")) } span.navcopy { strong { "Profile" } small { "How you appear to others" } } }
                                a.navitem href="/appearance" { span.navicon { (ui::icon("palette")) } span.navcopy { strong { "Appearance" } small { "Theme, density, motion" } } }
                                form method="post" action="/logout" { button.navitem type="submit" { span.navicon { (ui::icon("logout")) } span.navcopy { strong { "Sign out" } small { (name) } } } }
                            }
                        }
                    }
                }
                div.statusline #statusline data-src="/frag/status" data-live="dashboard" aria-live="off" { (fleet::status_line(ctx)) }
            }
            main.stage #main {
                div.page-head {
                    div {
                        @if let Some(sec) = section { span.kicker { (sec) } }
                        h1 { (p.title) }
                        @if !p.lede.is_empty() { p.lede { (p.lede) } }
                    }
                    @if let Some(act) = &p.actions { div.page-actions { (act) } }
                }
                (p.body)
            }
            footer.footer { span { "SwarmPanel · HollowHeavenXR" } a href="https://discord.com/users/1304564041863266347" target="_blank" rel="noreferrer" { "Discord" } }
            aside.drawer #drawer aria-label="Details" {
                button.roundbtn.drawer-close type="button" data-close="#drawer" aria-label="Close" { (ui::icon("x")) }
                div #drawer-body data-src="" data-live="dashboard" data-lazy {}
            }
            div.veil data-close="#drawer" {}
            div.palette #palette role="dialog" aria-modal="true" aria-label="Jump to" {
                div.palette-box {
                    label.palette-input { (ui::icon("search")) input type="text" placeholder="Jump to a page, bot or action…" autocomplete="off" spellcheck="false" aria-label="Search"; kbd { "esc" } }
                    ul.palette-list role="listbox" {}
                }
                script type="application/json" #palette-data { (PreEscaped(palette_items(ctx).to_string().replace("</", "<\\/"))) }
            }
            nav.tabbar aria-label="Quick" {
                @for (to, label, icon) in MOBILE_PRIMARY {
                    a.here[is_active(p.path, to)] href=(to) { (ui::icon(icon)) span { (label) } }
                }
                button type="button" data-palette-open { (ui::icon("search")) span { "Jump" } }
            }
            div #toast.toast role="status" aria-live="polite" {}
        }
    };
    let token = ctx.req.cookies.get(SESSION_COOKIE).map(String::as_str);
    Reply::html(document(p.title, &ctx.prefs, token, body)).by(a).header("Cache-Control", "no-store")
}

/// Bare page for signed-out screens (login).
pub fn render_public(title: &str, body: Markup) -> Reply {
    let body = html! { body.public { (body) div #toast.toast role="status" aria-live="polite" {} } };
    Reply::html(document(title, &Value::Null, None, body)).header("Cache-Control", "no-store")
}

pub fn denied(ctx: &Ctx, path: &str, title: &str, msg: &str) -> Reply {
    render(ctx, Page::new(path, title, "", ui::notice("error", msg))).status(403)
}

pub fn frag(m: Markup) -> Reply {
    Reply::html(m.into_string()).header("Cache-Control", "no-store")
}

fn page_or_redirect(r: R<Reply>) -> Reply {
    match r {
        Ok(r) | Err(r) => r,
    }
}

macro_rules! page {
    ($f:path) => {
        |req: Req| async move { page_or_redirect($f(req).await) }
    };
}

// ---- static assets ----------------------------------------------------------------------------
pub static APP_CSS: &str = include_str!("../../static/app.css");
pub static WEB_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/web.js"));
pub static WEB_WASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/web_bg.wasm"));
static IMG_GALLERY: &[u8] = include_bytes!("../../static/images/image-gallery.png");
static IMG_LUMISOUND: &[u8] = include_bytes!("../../static/images/lumisound.png");
const FAVICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="#0f1420"/><path d="M16 5l9.5 5.5v11L16 27l-9.5-5.5v-11z" fill="none" stroke="#89b4fa" stroke-width="2.4" stroke-linejoin="round"/><circle cx="16" cy="16" r="3.4" fill="#cba6f7"/></svg>"##;

fn asset(body: Vec<u8>, ctype: &str, req: &Req) -> Reply {
    let cache = if req.q("v").is_some() { "public, max-age=31536000, immutable" } else { "public, max-age=3600" };
    Reply::bytes(200, body, ctype).header("Cache-Control", cache)
}

async fn static_file(req: Req) -> Reply {
    match req.p("file").as_str() {
        "app.css" => asset(APP_CSS.as_bytes().to_vec(), "text/css; charset=utf-8", &req),
        "web.js" => asset(WEB_JS.as_bytes().to_vec(), "text/javascript; charset=utf-8", &req),
        "web_bg.wasm" => asset(WEB_WASM.to_vec(), "application/wasm", &req),
        _ => Reply::err(404, "Not found"),
    }
}

async fn static_image(req: Req) -> Reply {
    match req.p("file").as_str() {
        "image-gallery.png" => asset(IMG_GALLERY.to_vec(), "image/png", &req),
        "lumisound.png" => asset(IMG_LUMISOUND.to_vec(), "image/png", &req),
        _ => Reply::err(404, "Not found"),
    }
}

async fn favicon(req: Req) -> Reply {
    asset(FAVICON.as_bytes().to_vec(), "image/svg+xml", &req)
}

async fn robots() -> Reply {
    Reply::bytes(200, b"User-agent: *\nDisallow: /\n".to_vec(), "text/plain; charset=utf-8")
}

// ---- fragments --------------------------------------------------------------------------------
async fn fragment(req: Req) -> Reply {
    let name = req.p("name");
    let ctx = match Ctx::frag(req).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let m = match name.as_str() {
        "notifications" => community::notifications_frag(&ctx).await,
        "status" => fleet::status_line_frag(&ctx).await,
        "bot" => fleet::bot_panel_frag(&ctx).await,
        "dashboard" => fleet::dashboard_frag(&ctx).await,
        "events" => fleet::events_frag(&ctx).await,
        "control-state" => fleet::control_state_frag(&ctx).await,
        "queues" => fleet::queues_frag(&ctx).await,
        "guild-overview" => fleet::guild_overview_frag(&ctx).await,
        "invites" => fleet::invites_frag(&ctx).await,
        "directory" => community::directory_frag(&ctx).await,
        "friends" => community::friends_frag(&ctx).await,
        "threads" => community::threads_frag(&ctx).await,
        "thread" => community::thread_frag(&ctx).await,
        "people-search" => community::people_search_frag(&ctx).await,
        "admin-stats" => admin::stats_frag(&ctx).await,
        "stability" => admin::stability_frag(&ctx).await,
        "metrics" => admin::metrics_frag(&ctx).await,
        "alert-rules" => admin::alert_rules_frag(&ctx).await,
        "exports" => admin::exports_frag(&ctx).await,
        "trends" => admin::trends_frag(&ctx).await,
        "intel-events" => admin::intel_events_frag(&ctx).await,
        "accounts" => admin::accounts_frag(&ctx).await,
        "gallery-table" => admin::gallery_table_frag(&ctx).await,
        "lumisound" => admin::lumisound_frag(&ctx).await,
        "audit" => admin::audit_frag(&ctx).await,
        _ => return Reply::err(404, "Unknown fragment"),
    };
    frag(m)
}

pub fn routes() -> Router<AppRef> {
    Router::new()
        .route("/", get(page!(fleet::dashboard)))
        .route("/dashboard", get(page!(fleet::dashboard)))
        .route("/controls", get(page!(fleet::controls)))
        .route("/controls/recover-all", post(fleet::recover_all))
        .route("/controls/load-queue", post(fleet::load_queue))
        .route("/invites", get(page!(fleet::invites)))
        .route("/leaderboard", get(page!(insights::leaderboard)))
        .route("/learning", get(page!(insights::learning)))
        .route("/bot-telemetry", get(page!(insights::telemetry)))
        .route("/users", get(page!(community::directory)))
        .route("/users/{id}", get(page!(community::public_profile)))
        .route("/friends", get(page!(community::friends)))
        .route("/messages", get(page!(community::messages)))
        .route("/profile", get(page!(account::profile)))
        .route("/appearance", get(page!(account::appearance)))
        .route("/other-projects", get(page!(account::other_projects)))
        .route("/login", get(account::login_page).post(account::login_submit))
        .route("/register", post(account::register_submit))
        .route("/logout", post(account::logout))
        .route("/admin", get(page!(admin::overview)))
        .route("/diagnostics", get(page!(admin::diagnostics)))
        .route("/intel", get(page!(admin::intel)))
        .route("/accounts", get(page!(admin::accounts)))
        .route("/databases", get(page!(admin::databases)))
        .route("/gallery-admin", get(page!(admin::gallery)))
        .route("/lumisound-admin", get(page!(admin::lumisound)))
        .route("/audit-log", get(page!(admin::audit_log)))
        .route("/frag/{name}", get(fragment))
        .route("/static/{file}", get(static_file))
        .route("/static/images/{file}", get(static_image))
        .route("/favicon.svg", get(favicon))
        .route("/favicon.ico", get(favicon))
        .route("/robots.txt", get(robots))
}
