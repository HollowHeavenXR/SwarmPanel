//! Community: Directory, public profiles, Friends, Messages, notifications.

use super::ui::{self, arr, icon, n, s};
use super::{Ctx, Page, render};
use crate::api::community as com;
use crate::db::J;
use crate::http::{R, Reply, Req};
use maud::{Markup, html};
use serde_json::{Value, json};

fn title_case(v: &str) -> String {
    v.replace(['_', '-'], " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn display(u: &Value) -> String {
    u.s("display_name").filter(|d| !d.is_empty()).or(u.s("username")).unwrap_or_else(|| "Unknown operator".into())
}

fn presence(on: bool) -> Markup {
    html! { span.presence-pill.on[on] { span.presence.on[on] {} (if on { "Online" } else { "Inactive" }) } }
}

fn guild_only(ctx: &Ctx, path: &str, title: &str, what: &str) -> Option<Reply> {
    ctx.a.guild_id.is_none().then(|| {
        render(
            ctx,
            Page::new(path, title, "", html! {
                div.empty.big {
                    p { (what) " are tied to a guild account, not the site admin login." }
                    p.muted { "Sign in with an account registered to a guild to use them." }
                }
            }),
        )
    })
}

fn follow_btn(id: &str, following: bool, disabled: bool) -> Markup {
    html! {
        button.btn.sm type="button" disabled[disabled] data-post=(format!("/api/users/{id}/follow")) data-body=(json!({ "following": !following })) data-ok=(if following { "Unfollowed." } else { "Following." }) data-done="refresh" {
            (if following { "Unfollow" } else { "Follow" })
        }
    }
}

fn friend_btn(id: &str, status: &str, can: bool) -> Markup {
    let locked = matches!(status, "friends" | "pending_out" | "self");
    let label = match status {
        "friends" => "Friends",
        "pending_out" => "Pending",
        "pending_in" => "Respond in Friends",
        _ => "Add friend",
    };
    html! {
        @if status == "pending_in" { a.btn.sm href="/friends" { (label) } }
        @else { button.btn.sm type="button" disabled[locked || !can] data-post=(format!("/api/users/{id}/friend-request")) data-body="{}" data-ok="Friend request sent." data-done="refresh" { (label) } }
    }
}

// ---- Directory -----------------------------------------------------------------------------------
pub async fn directory_frag(ctx: &Ctx) -> Markup {
    let q = ctx.q("q").unwrap_or_default();
    let online = ctx.q("online").is_some_and(|v| v == "1" || v == "on" || v == "true");
    let res = ctx.api(com::directory, &[("q", q), ("online", if online { "1".into() } else { String::new() }), ("limit", "48".into())], &[]).await;
    let users = match &res {
        Ok(v) => arr(v, "users").to_vec(),
        Err(e) => return ui::notice("error", &format!("Search failed: {e}")),
    };
    let online_n = users.iter().filter(|u| u.b("is_online")).count();
    html! {
        div.summary { span { (users.len()) " shown" } span { (online_n) " online" } }
        @if users.is_empty() { (ui::empty(if online { "Nobody matching is online right now." } else { "No users found." })) }
        div.user-grid {
            @for u in &users {
                @let id = s(u, "id");
                @let fav = u.s("favorite_bot").filter(|f| !f.is_empty()).map(|f| title_case(&f)).unwrap_or_else(|| "No favorite".into());
                @let me = s(u, "friend_status") == "self";
                article.user-card {
                    a.user-main href=(format!("/users/{id}")) {
                        (ui::avatar(u.s("avatar_url").or(u.s("server_icon_url")), &display(u), Some(u.b("is_online")), "lg"))
                        div.user-copy {
                            div.user-name { h3 { (display(u)) } (presence(u.b("is_online"))) }
                            p.muted { "@" (u.s("username").unwrap_or_else(|| "operator".into())) }
                            p.small { (u.s("server_name").or(u.s("profile_headline")).or(u.s("guild_id").map(|g| format!("Guild {g}"))).unwrap_or_else(|| "Swarm directory".into())) }
                        }
                    }
                    div.user-stats {
                        div { strong { (n(u, "follower_count")) } span { "Followers" } }
                        div { strong { (n(u, "friend_count")) } span { "Friends" } }
                        div { strong { (fav) } span { "Favorite bot" } }
                    }
                    @if !me {
                        div.user-actions {
                            (follow_btn(&id, u.b("followed_by_me"), false))
                            (friend_btn(&id, &s(u, "friend_status"), true))
                            a.btn.sm href=(format!("/messages?to={id}")) { (icon("mail")) "Message" }
                        }
                    }
                }
            }
        }
    }
}

pub async fn directory(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let q = ctx.q("q").unwrap_or_default();
    let online = ctx.q("online").is_some();
    let body = html! {
        form.filter-bar method="get" action="/users" data-get="#directory" {
            label.search { (icon("search")) input type="search" name="q" value=(q) placeholder="Search operators…" autocomplete="off" aria-label="Search operators"; }
            label.toggle { input type="checkbox" name="online" value="1" checked[online]; span { "Online now" } }
        }
        div #directory data-src=(format!("/frag/directory?{}", ctx.req.query.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&"))) { (directory_frag(&ctx).await) }
    };
    Ok(render(&ctx, Page::new("/users", "Directory", "Find other operators in the swarm.", body)))
}

// ---- Profiles ------------------------------------------------------------------------------------
/// The profile hero card (used for public profiles and the editor preview).
pub fn profile_hero(p: &Value, actions: Option<Markup>) -> Markup {
    let one_of = |k: &str, allowed: &[&str], d: &'static str| p.s(k).filter(|v| allowed.contains(&v.as_str())).unwrap_or_else(|| d.into());
    let accent = p.s("theme_accent").filter(|a| a.len() == 7 && a.starts_with('#')).unwrap_or_else(|| "#89b4fa".into());
    let banner = p.s("profile_banner_url").as_deref().and_then(ui::safe_url).map(|u| format!("url(\"{}\")", u.replace('"', "%22"))).unwrap_or_else(|| "none".into());
    let classes = format!(
        "hero card-{} banner-{} border-{} header-{}",
        one_of("profile_card_style", &["solid", "glass", "outline", "terminal"], "solid"),
        one_of("profile_banner_mode", &["gradient", "image", "signal", "minimal"], "gradient"),
        one_of("profile_border_accent", &["none", "glow", "pulse", "neon", "solid"], "none"),
        one_of("profile_header_style", &["solid", "glass", "blur", "transparent", "gradient"], "solid"),
    );
    let tags: Vec<String> = arr(p, "profile_tags").iter().filter_map(|t| t.as_str().map(str::to_string)).take(5).collect();
    let social = match s(p, "profile_social_mode").to_lowercase().as_str() {
        "friends" => "Friends only",
        "quiet" => "Quiet",
        _ => "Open",
    };
    html! {
        section class=(classes) style=(format!("--accent:{accent};--banner:{banner}")) {
            div.hero-banner {}
            div.hero-body {
                (ui::avatar(p.s("avatar_url").or(p.s("server_icon_url")), &display(p), Some(p.b("is_online")), "xl"))
                div.hero-copy {
                    h2 { (display(p)) }
                    @if let Some(h) = p.s("profile_headline").filter(|h| !h.is_empty()) { p.headline { (h) } }
                    @if let Some(b) = p.s("bio").filter(|b| !b.is_empty()) { p { (b) } }
                    @if let Some(q) = p.s("profile_quote").filter(|q| !q.is_empty()) { blockquote { (q) } }
                    div.chips {
                        (presence(p.b("is_online")))
                        @for t in &tags { span.chip { (t) } }
                        @if let Some(f) = p.s("favorite_bot").filter(|f| !f.is_empty()) { span.chip { (title_case(&f)) } }
                        span.chip { (p.s("server_name").filter(|x| !x.is_empty()).unwrap_or_else(|| format!("Guild {}", s(p, "guild_id")))) }
                        span.chip { (social) }
                    }
                }
                @if let Some(a) = actions { div.hero-actions { (a) } }
            }
        }
    }
}

pub fn profile_stats(p: &Value) -> Markup {
    let act = p.get("activity").cloned().unwrap_or(json!({}));
    html! {
        div.metrics {
            (ui::metric("Followers", n(p, "follower_count"), None))
            (ui::metric("Following", n(p, "following_count"), None))
            (ui::metric("Friends", n(p, "friend_count"), None))
            (ui::metric("Plays", n(&act, "total_plays"), None))
        }
    }
}

pub async fn public_profile(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let id = ctx.req.p("id");
    let path = format!("/users/{id}");
    let res = ctx.api(com::profile, &[], &[("account_id", id.clone())]).await;
    let body = match res {
        Err(_) => ui::notice("error", "Profile not found."),
        Ok(v) => {
            let p = v.get("profile").cloned().unwrap_or(json!({}));
            let perms = v.get("social_permissions").cloned().unwrap_or(json!({}));
            let me = s(&p, "friend_status") == "self";
            let actions = (!me).then(|| html! {
                (follow_btn(&id, p.b("followed_by_me"), !p.b("followed_by_me") && !perms.b("can_follow")))
                (friend_btn(&id, &s(&p, "friend_status"), perms.b("can_friend")))
                @if perms.b("can_message") { a.btn.sm.primary href=(format!("/messages?to={id}")) { (icon("mail")) "Message" } }
                @else { button.btn.sm type="button" disabled { "Messages closed" } }
            });
            let act = p.get("activity").cloned().unwrap_or(json!({}));
            let links = arr(&p, "profile_links");
            let friends = arr(&v, "friends");
            html! {
                div.backdrop-skin {
                    (profile_hero(&p, actions))
                    (profile_stats(&p))
                    div.split {
                        section.card {
                            (ui::section("Swarm activity", None, None))
                            @let sess = arr(&act, "active_sessions");
                            @if sess.is_empty() { (ui::empty("No live sessions right now.")) }
                            div.events { @for x in sess.iter().take(4) {
                                article.event { div.event-head { strong { (x.s("title").or(x.s("bot_name")).unwrap_or_default()) } } p.muted { (s(x, "bot_name")) " · " (x.s("channel_name").unwrap_or_else(|| "voice".into())) } }
                            } }
                        }
                        section.card {
                            (ui::section("Links", None, None))
                            div.link-list {
                                @for l in links { @if let Some(u) = l.s("url").as_deref().and_then(ui::safe_url) { a href=(u) target="_blank" rel="noreferrer" { (icon("ext")) (s(l, "label")) } } }
                                @if let Some(u) = p.s("server_invite_url").as_deref().and_then(ui::safe_url) { a href=(u) target="_blank" rel="noreferrer" { (icon("ext")) "Discord server" } }
                                @if links.is_empty() && p.s("server_invite_url").is_none_or(|u| u.is_empty()) { (ui::empty("No links")) }
                            }
                        }
                    }
                    @if !friends.is_empty() {
                        section.card {
                            (ui::section("Friends", None, None))
                            div.friend-strip { @for f in friends { a.friend-chip href=(format!("/users/{}", s(f, "id"))) { (ui::avatar(f.s("avatar_url"), &display(f), Some(f.b("is_online")), "sm")) span { (display(f)) } } } }
                        }
                    }
                }
            }
        }
    };
    Ok(render(&ctx, Page::new(&path, "Profile", "", body)))
}

// ---- Friends -------------------------------------------------------------------------------------
pub async fn friends_frag(ctx: &Ctx) -> Markup {
    let v = match ctx.live("friends", json!({})).await {
        Ok(v) => v,
        Err(e) => return ui::notice("error", &format!("Failed to load friends: {e}")),
    };
    let respond = |id: &str, action: &str, label: &str, danger: bool| {
        html! { button.btn.sm.danger[danger].primary[action == "accept"] type="button" data-post=(format!("/api/friends/requests/{id}")) data-body=(json!({ "action": action })) data-done="refresh" { (label) } }
    };
    let person = |r: &Value| {
        let uid = r.s("user_id").or(r.s("id")).unwrap_or_default();
        html! { a.person href=(format!("/users/{uid}")) { (ui::avatar(r.s("avatar_url"), &display(r), r.get("is_online").map(|_| r.b("is_online")), "sm")) span { (display(r)) small.muted { "@" (s(r, "username")) } } } }
    };
    html! {
        div.columns3 {
            section.card {
                (ui::section("Incoming", None, None))
                ul.rows { @for r in arr(&v, "incoming") { li.row { (person(r)) div.row-actions { (respond(&s(r, "id"), "accept", "Accept", false)) (respond(&s(r, "id"), "decline", "Decline", true)) } } } }
                @if arr(&v, "incoming").is_empty() { (ui::empty("No incoming requests.")) }
            }
            section.card {
                (ui::section("Outgoing", None, None))
                ul.rows { @for r in arr(&v, "outgoing") { li.row { (person(r)) div.row-actions { (respond(&s(r, "id"), "cancel", "Cancel", false)) } } } }
                @if arr(&v, "outgoing").is_empty() { (ui::empty("No pending requests.")) }
            }
            section.card {
                (ui::section("Friends", Some(&ui::plural(arr(&v, "friends").len() as i64, "friend", "friends")), None))
                ul.rows { @for f in arr(&v, "friends") { li.row { (person(f)) div.row-actions { a.btn.sm href=(format!("/messages?to={}", s(f, "id"))) { (icon("mail")) } } } } }
                @if arr(&v, "friends").is_empty() { (ui::empty("No friends yet. Find people in the Directory.")) }
            }
        }
    }
}

pub async fn friends(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    if let Some(r) = guild_only(&ctx, "/friends", "Friends", "Friends") {
        return Ok(r);
    }
    let body = html! { div #friends data-src="/frag/friends" data-live="friends" { (friends_frag(&ctx).await) } };
    Ok(render(&ctx, Page::new("/friends", "Friends", "Requests and confirmed friends.", body)))
}

// ---- Messages ------------------------------------------------------------------------------------
fn thread_link(t: &Value, active: &str) -> Markup {
    let id = t.s("account_id").or(t.s("id")).unwrap_or_default();
    let unread = n(t, "unread_count");
    html! {
        a.thread.active[id == active] href=(format!("/messages?to={id}")) {
            (ui::avatar(t.s("avatar_url"), &display(t), Some(t.b("is_online")), "sm"))
            span.thread-name { (display(t)) }
            @if unread > 0 { span.badge { (if unread > 99 { "99+".to_string() } else { unread.to_string() }) } }
        }
    }
}

pub async fn threads_frag(ctx: &Ctx) -> Markup {
    let active = ctx.q("to").unwrap_or_default();
    match ctx.live("threads", json!({})).await {
        Ok(v) => html! {
            @if arr(&v, "threads").is_empty() { (ui::empty("No conversations yet.")) }
            @for t in arr(&v, "threads") { (thread_link(t, &active)) }
        },
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn thread_frag(ctx: &Ctx) -> Markup {
    let to = ctx.q("to").unwrap_or_default();
    match ctx.live("thread_messages", json!({ "account_id": to })).await {
        Ok(v) => {
            let ms = arr(&v, "messages");
            html! {
                @if ms.is_empty() { (ui::empty("No messages yet. Say hello.")) }
                @for m in ms { div.bubble.mine[m.b("mine")] { (s(m, "body")) (ui::time(m.s("created_at"), "time")) } }
            }
        }
        Err(e) => ui::notice("error", &e),
    }
}

pub async fn people_search_frag(ctx: &Ctx) -> Markup {
    let q = ctx.q("q").unwrap_or_default();
    if q.trim().is_empty() {
        return html! {};
    }
    match ctx.api(com::directory, &[("q", q), ("limit", "8".into())], &[]).await {
        Ok(v) => {
            let people: Vec<Value> = arr(&v, "users")
                .iter()
                .filter(|u| s(u, "friend_status") != "self")
                .map(|u| {
                    let mut t = u.clone();
                    t["account_id"] = u["id"].clone();
                    t
                })
                .collect();
            html! {
                @for t in &people { (thread_link(t, "")) }
                @if people.is_empty() { p.muted.small { "Nobody found." } }
            }
        }
        Err(_) => html! {},
    }
}

pub async fn messages(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    if let Some(r) = guild_only(&ctx, "/messages", "Messages", "Messages") {
        return Ok(r);
    }
    let to = ctx.q("to").unwrap_or_default();
    let other = if to.is_empty() { None } else { com::social_target(&ctx.req, &to, 0).await };
    let to_q = urlencoding::encode(&to).into_owned();
    let body = html! {
        div.messenger {
            aside.threads {
                form method="get" action="/messages" data-get="#people" {
                    @if !to.is_empty() { input type="hidden" name="to" value=(to); }
                    label.search { (icon("search")) input type="search" name="q" placeholder="Find someone…" autocomplete="off" aria-label="Find someone"; }
                }
                div #people data-src="/frag/people-search" {}
                div #threads data-src=(format!("/frag/threads?to={to_q}")) data-live="threads" { (threads_frag(&ctx).await) }
            }
            section.conversation {
                @if let Some(o) = &other {
                    header.conv-head {
                        a.person href=(format!("/users/{to}")) { (ui::avatar(o.s("avatar_url"), &display(o), Some(o.b("is_online")), "sm")) span { (display(o)) small.muted { "@" (s(o, "username")) } } }
                    }
                    div.bubbles #thread data-src=(format!("/frag/thread?to={to_q}")) data-live="thread_messages" data-live-params=(json!({ "account_id": to }).to_string()) data-scroll="bottom" { (thread_frag(&ctx).await) }
                    form.composer data-api=(format!("/api/messages/{to}")) data-done="refresh:#thread" data-reset {
                        input type="text" name="body" placeholder="Message…" maxlength="2000" autocomplete="off" required aria-label="Message" autofocus;
                        button.btn.primary type="submit" { "Send" }
                    }
                } @else if !to.is_empty() {
                    (ui::notice("error", "That account wasn't found."))
                } @else {
                    div.empty.big { (icon("mail")) p { "Select a conversation, or find someone to message." } }
                }
            }
        }
    };
    Ok(render(&ctx, Page::new("/messages", "Messages", "Direct messages with other operators.", body)))
}

// ---- Notifications (topbar bell) -----------------------------------------------------------------
pub async fn notifications_frag(ctx: &Ctx) -> Markup {
    match ctx.live("notifications", json!({})).await {
        Ok(v) => {
            let items = arr(&v, "notifications");
            html! {
                @if items.is_empty() { (ui::empty("No notifications yet.")) }
                ul.notif-list { @for x in items {
                    @let link = x.s("link_path").filter(|l| l.starts_with('/')).unwrap_or_default();
                    li { button.notif.unread[x.get("read_at").is_none_or(Value::is_null)] type="button" data-post=(format!("/api/notifications/{}/read", s(x, "id"))) data-body="{}" data-done=(if link.is_empty() { "refresh:#notif-list".to_string() } else { format!("go:{link}") }) {
                        strong { (s(x, "title")) }
                        @if let Some(b) = x.s("body").filter(|b| !b.is_empty()) { span { (b) } }
                        (ui::time(x.s("created_at"), "ago"))
                    } }
                } }
            }
        }
        Err(e) => ui::notice("error", &e),
    }
}
