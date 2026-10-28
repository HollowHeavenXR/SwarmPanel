//! Account: sign-in/register, Profile editor, Appearance, Other Projects.

use super::community::{profile_hero, profile_stats};
use super::ui::{self, arr, icon, s};
use super::{Ctx, Page, call, render, render_public};
use crate::api::{community as com, session as ses};
use crate::db::J;
use crate::http::{R, Reply, Req};
use maud::{Markup, html};
use serde_json::{Value, json};

// ---- Sign in / register --------------------------------------------------------------------------
fn safe_next(n: Option<&String>) -> String {
    n.filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/login")).cloned().unwrap_or_else(|| "/".into())
}

fn login_view(next: &str, mode: &str, err: Option<&str>, form: &std::collections::HashMap<String, String>) -> Reply {
    let v = |k: &str| form.get(k).cloned().unwrap_or_default();
    let reg = mode == "register";
    let dm = v("proof") == "discord";
    let body = html! {
        main.auth {
            div.auth-card {
                div.auth-brand { span.brand-mark { (ui::icon("hex")) } div { strong { "SwarmPanel" } small { "Fleet Command" } } }
                input.tab-radio type="radio" name="auth-tab" #tab-login checked[!reg];
                input.tab-radio type="radio" name="auth-tab" #tab-register checked[reg];
                div.segmented role="tablist" { label for="tab-login" { "Sign in" } label for="tab-register" { "Register" } }
                @if let Some(e) = err { (ui::notice("error", e)) }
                form.auth-form.login-pane method="post" action="/login" {
                    p.lede { "Sign in to reach fleet command." }
                    input type="hidden" name="next" value=(next);
                    label.field { span { "Username" } input type="text" name="username" value=(v("username")) autocomplete="username" required autofocus[!reg]; }
                    label.field { span { "Password" } input type="password" name="password" autocomplete="current-password" required; }
                    button.btn.primary.block type="submit" { "Sign in" }
                }
                form.auth-form.register-pane method="post" action="/register" {
                    p.lede { "Register your guild, then prove you own it with a webhook or a Discord DM code." }
                    input type="hidden" name="next" value=(next);
                    label.field { span { "Username" } input type="text" name="username" value=(v("username")) autocomplete="username" required; }
                    label.field { span { "Password" } input type="password" name="password" autocomplete="new-password" minlength="8"; }
                    label.field { span { "Guild ID" } input type="text" name="guild_id" value=(v("guild_id")) inputmode="numeric" required; }
                    label.field { span { "Email (optional)" } input type="email" name="email" value=(v("email")) autocomplete="email"; }
                    fieldset.proof {
                        legend { "Ownership proof" }
                        label.radio { input type="radio" name="proof" value="webhook" checked[!dm]; span { "Server webhook" } }
                        label.radio { input type="radio" name="proof" value="discord" checked[dm]; span { "Discord DM" } }
                        div.proof-webhook {
                            label.field { span { "Discord webhook URL" } input type="url" name="verification_webhook_url" value=(v("verification_webhook_url")) placeholder="https://discord.com/api/webhooks/…"; }
                            ol.steps {
                                li { "In a channel you manage: Channel Settings → Integrations → Webhooks → New Webhook." }
                                li { "Copy the URL and paste it here. SwarmPanel checks it belongs to the same server, then sends your code there." }
                                li { "Delete the webhook once you've verified." }
                            }
                        }
                        div.proof-discord {
                            label.field { span { "Your Discord user ID" } input type="text" name="discord_user_id" value=(v("discord_user_id")) inputmode="numeric" placeholder="1234567890123456"; }
                            ol.steps {
                                li { "Discord Settings → Advanced → turn on Developer Mode." }
                                li { "Right-click your name → Copy User ID." }
                                li { "You must share a server with the bot and allow DMs from server members." }
                            }
                        }
                    }
                    button.btn.primary.block type="submit" { "Create account" }
                }
            }
        }
    };
    render_public("Sign in", body)
}

pub async fn login_page(req: Req) -> Reply {
    if req.auth().is_some() {
        return Reply::redirect("/");
    }
    login_view(&safe_next(req.query.get("next")), &req.q("mode").unwrap_or_default(), None, &Default::default())
}

pub async fn login_submit(req: Req) -> Reply {
    let f = req.form();
    let next = safe_next(f.get("next"));
    let g = |k: &str| f.get(k).cloned().unwrap_or_default();
    match ses::do_login(&req, g("username").trim(), &g("password"), None, Some("web".into()), false, false).await {
        Ok(r) => Reply::redirect(&next).cookie(req.app.session_cookie(&r.token)),
        Err(e) => {
            let msg = match &e.payload {
                crate::http::Payload::Json(v) => v.s("detail").unwrap_or_else(|| "Sign-in failed".into()),
                _ => "Sign-in failed".into(),
            };
            login_view(&next, "login", Some(&msg), &f).status(e.status)
        }
    }
}

pub async fn register_submit(req: Req) -> Reply {
    let f = req.form();
    let next = safe_next(f.get("next"));
    let g = |k: &str| f.get(k).cloned().unwrap_or_default();
    let dm = g("proof") == "discord";
    let body = json!({
        "username": g("username").trim(), "password": g("password"), "guild_id": g("guild_id").trim(), "email": g("email").trim(),
        "verification_webhook_url": if dm { String::new() } else { g("verification_webhook_url").trim().to_string() },
        "discord_user_id": if dm { g("discord_user_id").trim().to_string() } else { String::new() }, "client": "web",
    });
    match call(&req, ses::register, &[], &[], body).await {
        Ok(v) => {
            let to = if next == "/" { "/profile?welcome=1".to_string() } else { next };
            Reply::redirect(&to).cookie(req.app.session_cookie(&v.s("token").unwrap_or_default()))
        }
        Err(e) => login_view(&next, "register", Some(&e), &f).status(400),
    }
}

pub async fn logout(req: Req) -> Reply {
    Reply::redirect("/login").cookie(req.app.clear_cookie())
}

// ---- Profile -------------------------------------------------------------------------------------
fn sel(name: &str, label: &str, cur: &str, opts: &[(&str, &str)]) -> Markup {
    html! { label.field { span { (label) } select name=(name) { @for (v, l) in opts { option value=(v) selected[*v == cur] { (l) } } } } }
}

fn text(name: &str, label: &str, val: &str, max: usize, ph: &str) -> Markup {
    html! { label.field { span { (label) } input type="text" name=(name) value=(val) maxlength=(max) placeholder=(ph); } }
}

pub async fn profile(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let me = ctx.api(com::me, &[], &[]).await.map_err(|e| Reply::err(500, e))?;
    let p = me.get("profile").cloned().unwrap_or(json!({}));
    if !me.b("editable") {
        let body = html! {
            (ui::notice("info", "The built-in admin login has no profile. Registered guild accounts own public profiles."))
            (profile_hero(&p, None))
        };
        return Ok(render(&ctx, Page::new("/profile", "Profile", "", body)));
    }
    let looks = ctx.api(com::presets, &[], &[]).await.unwrap_or(json!({}));
    let v = |k: &str| s(&p, k);
    let tags = arr(&p, "profile_tags").iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", ");
    let links = arr(&p, "profile_links");
    let verified = p.b("verification_verified") || p.b("webhook_verified") || p.b("email_verified") || p.b("discord_dm_verified");
    let checklist = [
        ("Account verified", verified),
        ("Display name set", p.s("display_name").is_some_and(|d| !d.is_empty() && Some(&d) != p.s("username").as_ref())),
        ("Avatar set", p.s("avatar_url").is_some_and(|a| !a.is_empty())),
        ("Bio written", p.s("bio").is_some_and(|b| !b.is_empty())),
        ("Profile is public", p.b("public_profile")),
    ];
    let body = html! {
        @if ctx.q("welcome").is_some() { (ui::notice("success", "Welcome aboard. Check your webhook channel or Discord DMs for the verification code, then enter it below.")) }
        div.backdrop-skin { (profile_hero(&p, Some(html! { a.btn.sm href=(format!("/users/{}", v("id"))) { "View public page" } }))) }
        (profile_stats(&p))
        div.editor-grid {
            form.card.stack #profile-form data-api="/api/users/me" data-ok="Profile saved." data-done="reload" {
                @let presets = arr(&looks, "profile");
                @if !presets.is_empty() {
                    div {
                        (ui::section("Quick looks", Some("Fill the style fields in one tap, then save."), None))
                        div.preset-grid { @for pr in presets {
                            button.preset type="button" data-fill=(pr.get("patch").cloned().unwrap_or(json!({})).to_string()) data-fill-form="#profile-form" { strong { (s(pr, "title")) } span { (s(pr, "note")) } }
                        } }
                    }
                }
                h3 { "Identity" }
                (text("display_name", "Display name", &v("display_name"), 80, ""))
                (text("profile_headline", "Headline", &v("profile_headline"), 140, ""))
                label.field { span { "Bio" } textarea name="bio" rows="3" maxlength="280" { (v("bio")) } }
                (text("profile_quote", "Signature / quote", &v("profile_quote"), 160, "A short motto or vibe"))
                label.field { span { "Tags (comma separated, up to 5)" } input type="text" name="profile_tags" data-t="csv" value=(tags) placeholder="tag1, tag2"; }
                label.field { span { "Favorite bot" } select name="favorite_bot" {
                    option value="" { "None" }
                    @for b in arr(&me, "favorite_bot_options") { option value=(s(b, "key")) selected[s(b, "key") == v("favorite_bot")] { (s(b, "display_name")) } }
                } }
                h3 { "Links" }
                @for i in 0..5 {
                    @let l = links.get(i).cloned().unwrap_or(json!({}));
                    div.two-col {
                        label.field { span { "Label " (i + 1) } input type="text" name=(format!("profile_links[{i}].label")) value=(s(&l, "label")) maxlength="32"; }
                        label.field { span { "URL " (i + 1) } input type="url" name=(format!("profile_links[{i}].url")) value=(s(&l, "url")) placeholder="https://…"; }
                    }
                }
                h3 { "Server" }
                (text("server_name", "Server name", &v("server_name"), 120, ""))
                (text("server_invite_url", "Discord invite", &v("server_invite_url"), 300, "https://discord.gg/…"))
                (text("server_icon_url", "Server icon URL", &v("server_icon_url"), 500, "https://…"))
                h3 { "Look" }
                (text("avatar_url", "Avatar URL", &v("avatar_url"), 500, "https://…"))
                (text("profile_banner_url", "Banner URL", &v("profile_banner_url"), 500, "https://…"))
                label.field.color { span { "Accent colour" } input type="color" name="theme_accent" value=(p.s("theme_accent").unwrap_or_else(|| "#89b4fa".into())); }
                div.two-col {
                    (sel("profile_banner_mode", "Banner", &v("profile_banner_mode"), &[("gradient", "Gradient"), ("image", "Image"), ("signal", "Signal"), ("minimal", "Minimal")]))
                    (sel("profile_card_style", "Card", &v("profile_card_style"), &[("solid", "Solid"), ("glass", "Glass"), ("outline", "Outline"), ("terminal", "Terminal")]))
                    (sel("profile_layout_mode", "Layout", &v("profile_layout_mode"), &[("default", "Default"), ("sidebar", "Sidebar"), ("stacked", "Stacked"), ("split", "Split")]))
                    (sel("profile_header_style", "Header", &v("profile_header_style"), &[("solid", "Solid"), ("glass", "Glass"), ("blur", "Blur"), ("transparent", "Transparent"), ("gradient", "Gradient")]))
                    (sel("profile_border_accent", "Border", &v("profile_border_accent"), &[("none", "None"), ("solid", "Solid"), ("glow", "Glow"), ("pulse", "Pulse"), ("neon", "Neon")]))
                    (sel("profile_social_mode", "Who can reach you", &v("profile_social_mode"), &[("open", "Open"), ("friends", "Friends only"), ("quiet", "Quiet")]))
                }
                label.check { input type="checkbox" name="public_profile" checked[p.b("public_profile")]; span { "Public profile" } }
                div.form-foot { button.btn.primary type="submit" { "Save profile" } }
            }
            div.stack {
                section.card {
                    (ui::section("Getting started", None, None))
                    ul.checklist { @for (l, done) in checklist { li.done[done] { (icon(if done { "check" } else { "x" })) (l) } } }
                }
                section.card.stack {
                    (ui::section("Verification", None, None))
                    @if verified { (ui::notice("success", "Verified.")) }
                    @else if p.b("verification_pending") { (ui::notice("info", "Verification pending: enter the code you received.")) }
                    @else { (ui::notice("warning", "Not verified yet.")) }
                    @if !verified {
                        form.inline-form data-api="/api/session/verification/verify" data-ok="Verified." data-done="reload" {
                            label.field { span { "Verification code" } input type="text" name="code" inputmode="numeric" placeholder="123456" required; }
                            button.btn.primary type="submit" { "Verify" }
                        }
                        button.btn.sm type="button" data-post="/api/session/resend-verification" data-body="{}" data-ok="Code resent (if a delivery method is set)." { "Resend code" }
                        details.more {
                            summary { "Change how the code is delivered" }
                            form.inline-form data-api="/api/session/verification-webhook" data-ok="Saved. A code was sent if the webhook is valid." data-done="reload" {
                                label.field { span { "Server webhook URL" } input type="url" name="verification_webhook_url" value=(v("verification_webhook_url")) placeholder="https://discord.com/api/webhooks/…"; }
                                button.btn type="submit" { "Save & send" }
                            }
                            form.inline-form data-api="/api/session/verification-discord" data-ok="Saved. A code was sent by DM if possible." data-done="reload" {
                                label.field { span { "Discord user ID" } input type="text" name="discord_user_id" value=(v("discord_user_id")) inputmode="numeric"; }
                                button.btn type="submit" { "Save & send" }
                            }
                        }
                    }
                }
                form.card.stack data-api="/api/session/email" data-ok="Email updated." {
                    (ui::section("Email", None, None))
                    label.field { span { "Email" } input type="email" name="email" value=(v("email")) autocomplete="email"; }
                    div.form-foot { button.btn type="submit" { "Update email" } }
                }
                form.card.stack data-api="/api/session/password" data-ok="Password changed." data-reset {
                    (ui::section("Password", None, None))
                    label.field { span { "Current password" } input type="password" name="current_password" autocomplete="current-password"; }
                    label.field { span { "New password" } input type="password" name="new_password" minlength="8" autocomplete="new-password" required; }
                    div.form-foot { button.btn type="submit" { "Change password" } }
                }
            }
        }
    };
    Ok(render(&ctx, Page::new("/profile", "Profile", "Edit how you appear across the panel.", body)))
}

// ---- Appearance ----------------------------------------------------------------------------------
fn pref_sel(prefs: &Value, name: &str, label: &str, opts: &[(&str, &str)]) -> Markup {
    let cur = prefs.s(name).unwrap_or_default();
    html! { label.field { span { (label) } select name=(name) data-pref { @for (v, l) in opts { option value=(v) selected[*v == cur] { (l) } } } } }
}

fn pref_range(prefs: &Value, name: &str, label: &str, min: f64, max: f64, step: f64, var: &str, unit: &str, d: f64) -> Markup {
    let cur = prefs.f(name).unwrap_or(d);
    html! { label.field { span { (label) output { (cur) (unit) } } input type="range" name=(name) data-t="num" min=(min) max=(max) step=(step) value=(cur) data-var=(var) data-var-unit=(unit); } }
}

fn pref_check(prefs: &Value, name: &str, label: &str, d: bool) -> Markup {
    let on = match prefs.get(name) {
        Some(Value::Bool(b)) => *b,
        _ => d,
    };
    html! { label.check { input type="checkbox" name=(name) checked[on] data-pref; span { (label) } } }
}

pub async fn appearance(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let res = ctx.api(com::preferences, &[], &[]).await.unwrap_or(json!({}));
    let p = res.get("preferences").cloned().unwrap_or_else(crate::profiles::default_panel_preferences);
    let editable = res.b("editable");
    let looks = ctx.api(com::presets, &[], &[]).await.unwrap_or(json!({}));
    let mut presets = vec![
        json!({ "title": "Midnight Ops", "note": "Deep blue-black, cool accent, soft cards.", "patch": { "theme_mode": "dark", "background_mode": "midnight", "accent_color": "#89b4fa", "card_shape": "soft", "card_hover_effect": "lift" } }),
        json!({ "title": "Aurora Glass", "note": "Aurora backdrop, green accent, glow hover.", "patch": { "theme_mode": "dark", "background_mode": "aurora", "accent_color": "#7ee787", "card_shape": "soft", "card_hover_effect": "glow" } }),
        json!({ "title": "Ember Signal", "note": "Warm ember backdrop, red accent, crisp cards.", "patch": { "theme_mode": "dark", "background_mode": "ember", "accent_color": "#f38ba8", "card_shape": "crisp", "card_hover_effect": "border" } }),
    ];
    presets.extend(arr(&looks, "panel").iter().cloned());
    let defaults = crate::profiles::default_panel_preferences();
    let color = |k: &str, d: &str| p.s(k).filter(|c| c.len() == 7 && c.starts_with('#')).unwrap_or_else(|| d.into());
    let body = html! {
        @if !editable { (ui::notice("info", "The built-in admin login can't save preferences. Sign in with a guild account to customise the panel.")) }
        @if ctx.a.site_owner {
            section.card.row-card {
                div { h3 { "Admin mode" } p.muted { "Switch between your guild-scoped view and the unrestricted admin view." } }
                button.btn.on[ctx.a.is_admin()] type="button" data-post="/api/session/admin-mode" data-body=(json!({ "enabled": !ctx.a.is_admin() })) data-done="reload" { (if ctx.a.is_admin() { "Turn admin off" } else { "Turn admin on" }) }
            }
        }
        form.card.stack #appearance-form data-api="/api/users/preferences" data-ok="Saved." data-done="reload" data-pref-preview {
            p.muted.small { "Changes preview live across this whole page. Nothing is kept until you save." }
            div {
                (ui::section("Presets", None, None))
                div.preset-grid { @for pr in &presets {
                    button.preset type="button" data-fill=(pr.get("patch").cloned().unwrap_or(json!({})).to_string()) data-fill-form="#appearance-form" { strong { (s(pr, "title")) } span { (s(pr, "note")) } }
                } }
            }
            div.pref-groups {
                fieldset { legend { "Theme" }
                    (pref_sel(&p, "theme_mode", "Theme", &[("dark", "Dark"), ("light", "Light"), ("system", "Match system")]))
                    label.field.color { span { "Accent" } input type="color" name="accent_color" value=(color("accent_color", "#89b4fa")) data-var="--accent"; }
                    label.field.color { span { "Secondary accent" } input type="color" name="accent_secondary" value=(color("accent_secondary", &crate::profiles::auto_secondary(&color("accent_color", "#89b4fa")))) data-var="--accent-2"; }
                    (pref_sel(&p, "font_scale", "Text size", &[("normal", "Normal"), ("large", "Large"), ("dense", "Small")]))
                    (pref_sel(&p, "panel_font_family", "Font", &[("system", "System"), ("rounded", "Rounded"), ("serif", "Serif"), ("mono", "Monospace")]))
                }
                fieldset { legend { "Background" }
                    (pref_sel(&p, "background_mode", "Background", &[("default", "Default"), ("midnight", "Midnight"), ("aurora", "Aurora"), ("ember", "Ember"), ("custom_color", "Custom colour"), ("custom_image", "Custom image")]))
                    label.field.color { span { "Custom colour" } input type="color" name="background_color" value=(color("background_color", "#0b0e18")) data-var="--bg-custom"; }
                    label.field { span { "Image URL (custom image)" } input type="url" name="background_image_url" value=(s(&p, "background_image_url")) placeholder="https://…"; }
                    (pref_range(&p, "surface_opacity", "Surface opacity", 0.35, 1.0, 0.01, "--surface-alpha", "", 0.92))
                    (pref_range(&p, "surface_blur", "Surface blur", 0.0, 36.0, 1.0, "--surface-blur", "px", 18.0))
                }
                fieldset { legend { "Layout" }
                    (pref_sel(&p, "layout_mode", "Page width", &[("standard", "Standard"), ("focused", "Focused"), ("wide", "Wide")]))
                    (pref_sel(&p, "density", "Density", &[("comfortable", "Comfortable"), ("compact", "Compact")]))
                    (pref_sel(&p, "sidebar_style", "Sidebar", &[("full", "Full"), ("compact", "Icons only"), ("hidden", "Hidden (menu button)")]))
                    (pref_sel(&p, "tab_style", "Section tabs", &[("rail", "Pills"), ("underline", "Underline"), ("minimal", "Minimal")]))
                    (pref_sel(&p, "roster_layout", "Directory", &[("cards", "Cards"), ("signals", "Compact cards"), ("ledger", "List")]))
                    (pref_sel(&p, "dashboard_density", "Dashboard", &[("command", "Roomy"), ("dense", "Dense")]))
                    (pref_sel(&p, "operator_layout", "Profile layout", &[("command", "Command"), ("console", "Console"), ("compact", "Compact")]))
                    (pref_sel(&p, "notification_position", "Toast position", &[("br", "Bottom right"), ("bl", "Bottom left"), ("tr", "Top right"), ("tc", "Top centre")]))
                }
                fieldset { legend { "Cards & motion" }
                    (pref_sel(&p, "card_shape", "Card shape", &[("soft", "Soft"), ("crisp", "Crisp")]))
                    (pref_sel(&p, "panel_radius", "Corner radius", &[("sharp", "Sharp"), ("medium", "Medium"), ("soft", "Round")]))
                    (pref_sel(&p, "card_hover_effect", "Hover", &[("lift", "Lift"), ("glow", "Glow"), ("border", "Border"), ("none", "None")]))
                    (pref_sel(&p, "stream_card_style", "Bot cards", &[("telemetry", "Telemetry"), ("compact", "Compact"), ("cinematic", "Cinematic")]))
                    (pref_sel(&p, "bot_card_detail", "Bot card detail", &[("full", "Full"), ("compact", "Compact")]))
                    (pref_sel(&p, "motion", "Motion", &[("standard", "Standard"), ("reduced", "Reduced")]))
                    (pref_check(&p, "show_bot_uptime", "Show heartbeat and uptime", true))
                    (pref_check(&p, "show_queue_pressure", "Show queue depth", true))
                    (pref_check(&p, "compact_sidebar", "Tighter sidebar spacing", false))
                }
                fieldset { legend { "Profile backdrop" }
                    p.muted.small { "A personal skin behind every profile you view." }
                    label.field { span { "Backdrop image URL" } input type="url" name="profile_backdrop_image_url" value=(s(&p, "profile_backdrop_image_url")) placeholder="https://…"; }
                    (pref_range(&p, "profile_backdrop_strength", "Strength", 0.0, 0.55, 0.01, "", "", 0.18))
                }
            }
            div.form-foot {
                button.btn type="button" data-fill=(defaults.to_string()) data-fill-form="#appearance-form" { "Reset to defaults" }
                button.btn.primary type="submit" disabled[!editable] { "Save" }
            }
        }
    };
    Ok(render(&ctx, Page::new("/appearance", "Appearance", "Theme, layout, and motion.", body)))
}

// ---- Other projects ------------------------------------------------------------------------------
pub async fn other_projects(req: Req) -> R<Reply> {
    let ctx = Ctx::new(req).await?;
    let body = html! {
        div.project-grid {
            a.project href="https://gallery.xenusanimations.studio" target="_blank" rel="noopener noreferrer" {
                img src="/static/images/image-gallery.png" alt="" loading="lazy";
                div { h3 { "Image Gallery" } p { "Curated media deck for uploads, collections, and social feeds." } }
                span.btn { (icon("ext")) "Open" }
            }
            a.project href="/api/projects/lumisound/download" download {
                img src="/static/images/lumisound.png" alt="" loading="lazy";
                div { h3 { "Lumisound" } p { "iOS music app. Downloads the latest build straight from GitHub." } }
                span.btn { (icon("download")) "Download latest" }
            }
        }
    };
    Ok(render(&ctx, Page::new("/other-projects", "Other Projects", "A couple of other things I've built.", body)))
}
