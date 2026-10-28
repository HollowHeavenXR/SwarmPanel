//! Profile + panel-preference validation (port of profiles.lua, colorutil.lua, looks.lua).

use serde_json::{Map, Value, json};

fn opt_text(v: Option<&Value>, field: &str, max: usize) -> Result<Option<String>, String> {
    let Some(v) = v else { return Ok(None) };
    let t = match v {
        Value::Null => return Ok(None),
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    if t.is_empty() {
        return Ok(None);
    }
    if t.len() > max {
        return Err(format!("{field} must be {max} characters or fewer"));
    }
    Ok(Some(t))
}

pub fn public_url(v: Option<&Value>, field: &str, max: usize) -> Result<Option<String>, String> {
    let Some(u) = opt_text(v, field, max)? else { return Ok(None) };
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).ok_or_else(|| format!("{field} must be a public http or https URL"))?;
    if rest.split('/').next().unwrap_or("").is_empty() {
        return Err(format!("{field} must be a public http or https URL"));
    }
    Ok(Some(u))
}

fn server_invite(v: Option<&Value>) -> Result<Option<String>, String> {
    let Some(mut u) = opt_text(v, "Server invite URL", 600)? else { return Ok(None) };
    if u.starts_with("discord.gg/") {
        u = format!("https://{u}");
    }
    let bad = || Err("Server invite URL must be a Discord invite link".to_string());
    let Some(rest) = u.strip_prefix("https://") else { return bad() };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_lowercase();
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let ok = match host.as_str() {
        "discord.gg" => true,
        "discord.com" | "www.discord.com" => parts.len() >= 2 && parts[0] == "invite",
        _ => false,
    };
    if ok { Ok(Some(u)) } else { bad() }
}

pub fn accent(v: Option<&Value>, field: &str) -> Result<Option<String>, String> {
    let Some(a) = opt_text(v, field, 20)? else { return Ok(None) };
    let hex = a.strip_prefix('#').unwrap_or("");
    if a.starts_with('#') && (hex.len() == 6 || hex.len() == 3) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(Some(a.to_lowercase()))
    } else {
        Err(format!("{field} must be a hex color like #89b4fa"))
    }
}

fn choice(v: Option<&Value>, field: &str, allowed: &[&str], default: &str, aliases: &[(&str, &str)]) -> Result<String, String> {
    let raw = match v {
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(Value::Null) | None => default.to_string(),
        Some(o) => o.to_string().to_lowercase(),
    };
    let mut c = if raw.is_empty() { default.to_string() } else { raw };
    if let Some((_, to)) = aliases.iter().find(|(a, _)| *a == c) {
        c = to.to_string();
    }
    if allowed.contains(&c.as_str()) {
        Ok(c)
    } else {
        let mut names = allowed.to_vec();
        names.sort();
        Err(format!("{field} must be one of: {}", names.join(", ")))
    }
}

fn or_null(v: Option<String>) -> Value {
    v.map(Value::String).unwrap_or(Value::Null)
}

/// Validated column updates for a profile edit. Null = clear the column.
pub fn clean_profile_updates(p: &Value, bot_keys: &[&str]) -> Result<Map<String, Value>, String> {
    let mut u = Map::new();
    let has = |k: &str| p.get(k).is_some();
    let g = |k: &str| p.get(k);
    if has("display_name") {
        u.insert("display_name".into(), or_null(opt_text(g("display_name"), "Display name", 80)?));
    }
    if has("avatar_url") {
        u.insert("avatar_url".into(), or_null(public_url(g("avatar_url"), "Avatar URL", 600)?));
    }
    if has("bio") {
        u.insert("bio".into(), or_null(opt_text(g("bio"), "Bio", 280)?));
    }
    if has("profile_headline") {
        u.insert("profile_headline".into(), or_null(opt_text(g("profile_headline"), "Profile headline", 140)?));
    }
    if has("profile_tags") {
        let mut out: Vec<String> = Vec::new();
        for t in g("profile_tags").and_then(Value::as_array).cloned().unwrap_or_default() {
            let raw = t.as_str().map(str::to_string).unwrap_or_else(|| t.to_string());
            let tag: String = raw.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
            let tag = crate::util::cut(&tag, 32);
            if !tag.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&tag)) {
                out.push(tag);
                if out.len() >= 12 {
                    break;
                }
            }
        }
        u.insert("profile_tags".into(), json!(json!(out).to_string()));
    }
    if has("profile_links") {
        let mut out = Vec::new();
        for l in g("profile_links").and_then(Value::as_array).cloned().unwrap_or_default() {
            let label = opt_text(l.get("label"), "Link label", 32)?;
            let url = public_url(l.get("url"), "Profile link", 600)?;
            if let (Some(label), Some(url)) = (label, url) {
                out.push(json!({ "label": label, "url": url }));
                if out.len() >= 5 {
                    break;
                }
            }
        }
        u.insert("profile_links".into(), json!(json!(out).to_string()));
    }
    if has("profile_banner_url") {
        u.insert("profile_banner_url".into(), or_null(public_url(g("profile_banner_url"), "Profile banner URL", 600)?));
    }
    if has("profile_banner_mode") {
        u.insert("profile_banner_mode".into(), json!(choice(g("profile_banner_mode"), "Profile banner", &["gradient", "image", "signal", "quiet", "contrast"], "gradient", &[])?));
    }
    if has("profile_card_style") {
        u.insert("profile_card_style".into(), json!(choice(g("profile_card_style"), "Profile card style", &["solid", "glass", "outline", "terminal"], "solid", &[])?));
    }
    if has("profile_social_mode") {
        u.insert("profile_social_mode".into(), json!(choice(g("profile_social_mode"), "Profile social mode", &["open", "friends", "quiet"], "open", &[])?));
    }
    if has("favorite_bot") {
        let f = opt_text(g("favorite_bot"), "Favorite bot", 50)?;
        if let Some(f) = &f {
            if !bot_keys.contains(&f.as_str()) {
                return Err("Favorite bot must be one of the known swarm bots".into());
            }
        }
        u.insert("favorite_bot".into(), or_null(f));
    }
    if has("theme_accent") {
        u.insert("theme_accent".into(), or_null(accent(g("theme_accent"), "Theme accent")?));
    }
    if has("public_profile") {
        u.insert("public_profile".into(), json!(g("public_profile").is_some_and(|v| v.as_bool().unwrap_or(!v.is_null()))));
    }
    if has("server_invite_url") {
        u.insert("server_invite_url".into(), or_null(server_invite(g("server_invite_url"))?));
    }
    if has("server_name") {
        u.insert("server_name".into(), or_null(opt_text(g("server_name"), "Server name", 120)?));
    }
    if has("server_icon_url") {
        u.insert("server_icon_url".into(), or_null(public_url(g("server_icon_url"), "Server icon URL", 600)?));
    }
    if has("profile_quote") {
        u.insert("profile_quote".into(), or_null(opt_text(g("profile_quote"), "Profile quote", 160)?));
    }
    if has("profile_layout_mode") {
        u.insert("profile_layout_mode".into(), json!(choice(g("profile_layout_mode"), "Profile layout", &["default", "sidebar", "stacked", "split"], "default", &[])?));
    }
    if has("profile_header_style") {
        u.insert("profile_header_style".into(), json!(choice(g("profile_header_style"), "Profile header style", &["solid", "glass", "blur", "transparent", "gradient"], "solid", &[])?));
    }
    if has("profile_border_accent") {
        u.insert("profile_border_accent".into(), json!(choice(g("profile_border_accent"), "Profile border accent", &["none", "glow", "pulse", "neon", "solid"], "none", &[])?));
    }
    Ok(u)
}

pub fn default_panel_preferences() -> Value {
    json!({
        "theme_mode": "dark", "accent_color": "#89b4fa", "background_mode": "default", "background_color": "#0b0e18",
        "background_image_url": null, "profile_backdrop_image_url": null, "profile_backdrop_strength": 0.18,
        "layout_mode": "standard", "density": "comfortable", "card_shape": "soft", "font_scale": "normal", "motion": "standard",
        "operator_layout": "command", "roster_layout": "cards", "tab_style": "rail", "surface_opacity": 0.92, "surface_blur": 18,
        "stream_card_style": "telemetry", "dashboard_density": "command",
    })
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Validated preferences on top of `base`. Errors with a field message.
pub fn try_clean_panel_preferences(raw: Option<&Value>, base: &Value) -> Result<Value, String> {
    let mut p = base.clone();
    let empty = json!({});
    let r = raw.unwrap_or(&empty);
    let g = |k: &str| r.get(k);
    let has = |k: &str| r.get(k).is_some();
    if has("theme_mode") {
        p["theme_mode"] = json!(choice(g("theme_mode"), "Theme mode", &["dark", "light", "system"], "dark", &[])?);
    }
    if has("accent_color") {
        p["accent_color"] = json!(accent(g("accent_color"), "Theme accent")?.unwrap_or_else(|| "#89b4fa".into()));
    }
    if has("background_mode") {
        p["background_mode"] = json!(choice(g("background_mode"), "Background mode", &["default", "midnight", "aurora", "ember", "custom_color", "custom_image"], "default", &[])?);
    }
    if has("background_color") {
        p["background_color"] = json!(accent(g("background_color"), "Theme accent")?.unwrap_or_else(|| "#0b0e18".into()));
    }
    if has("background_image_url") {
        p["background_image_url"] = or_null(public_url(g("background_image_url"), "Background image URL", 600)?);
    }
    if has("profile_backdrop_image_url") {
        p["profile_backdrop_image_url"] = or_null(public_url(g("profile_backdrop_image_url"), "Profile backdrop image URL", 600)?);
    }
    if let Some(v) = g("profile_backdrop_strength") {
        let n = num(v).ok_or("Profile backdrop strength must be a number between 0.0 and 0.55")?;
        p["profile_backdrop_strength"] = json!(n.clamp(0.0, 0.55));
    }
    let simple: &[(&str, &str, &[&str], &str)] = &[
        ("layout_mode", "Layout mode", &["standard", "focused", "wide"], "standard"),
        ("density", "Density", &["comfortable", "compact"], "comfortable"),
        ("card_shape", "Card shape", &["soft", "crisp"], "soft"),
        ("font_scale", "Font scale", &["normal", "large", "dense"], "normal"),
        ("motion", "Motion", &["standard", "reduced"], "standard"),
        ("sidebar_style", "Sidebar style", &["full", "compact", "hidden"], "full"),
        ("panel_font_family", "Panel font", &["system", "serif", "mono", "rounded"], "system"),
        ("card_hover_effect", "Card hover", &["lift", "glow", "border", "none"], "lift"),
        ("notification_position", "Notification position", &["br", "bl", "tr", "tc"], "br"),
        ("bot_card_detail", "Bot card detail", &["full", "compact", "minimal"], "full"),
        ("panel_radius", "Panel radius", &["sharp", "medium", "soft"], "medium"),
    ];
    for (k, f, allowed, d) in simple {
        if has(k) {
            p[*k] = json!(choice(g(k), f, allowed, d, &[])?);
        }
    }
    if has("operator_layout") || has("profile_layout") {
        p["operator_layout"] = json!(choice(g("operator_layout").or(g("profile_layout")), "Operator layout", &["command", "console", "compact"], "command", &[("spotlight", "command"), ("studio", "console")])?);
    }
    if has("roster_layout") || has("directory_layout") {
        p["roster_layout"] = json!(choice(g("roster_layout").or(g("directory_layout")), "Roster layout", &["cards", "signals", "ledger"], "cards", &[("grid", "cards"), ("magazine", "signals"), ("stack", "ledger")])?);
    }
    if has("tab_style") {
        p["tab_style"] = json!(choice(g("tab_style"), "Tab style", &["rail", "underline", "minimal"], "rail", &[("pills", "rail")])?);
    }
    if let Some(v) = g("surface_opacity") {
        let n = num(v).ok_or("Surface opacity must be a number between 0.35 and 1.0")?;
        p["surface_opacity"] = json!(n.clamp(0.35, 1.0));
    }
    if let Some(v) = g("surface_blur") {
        let n = num(v).ok_or("Surface blur must be an integer between 0 and 36")?;
        p["surface_blur"] = json!((n.floor() as i64).clamp(0, 36));
    }
    if has("stream_card_style") {
        p["stream_card_style"] = json!(choice(g("stream_card_style"), "Bot card style", &["telemetry", "compact", "cinematic"], "telemetry", &[("editorial", "telemetry")])?);
    }
    if has("dashboard_density") {
        p["dashboard_density"] = json!(choice(g("dashboard_density"), "Dashboard density", &["command", "dense"], "command", &[("comfortable", "command"), ("compact", "dense")])?);
    }
    if has("accent_secondary") {
        p["accent_secondary"] = or_null(accent(g("accent_secondary"), "Theme accent")?);
    }
    for k in ["show_bot_uptime", "show_queue_pressure", "compact_sidebar"] {
        if let Some(v) = g(k) {
            p[k] = json!(v.as_bool().unwrap_or(!v.is_null()));
        }
    }
    if let Some(v) = g("admin_mode_enabled") {
        p["admin_mode_enabled"] = json!(v == &Value::Bool(true));
    }
    Ok(p)
}

/// Lenient version for stored data: falls back to `base` on any error.
pub fn clean_panel_preferences(raw: Option<&Value>, base: &Value) -> Value {
    try_clean_panel_preferences(raw, base).unwrap_or_else(|_| base.clone())
}

// ---- colors ---------------------------------------------------------------------------------
fn hex_rgb(hex: &str) -> Option<(f64, f64, f64)> {
    let h = hex.trim_start_matches('#');
    let h = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect::<String>() } else { h.to_string() };
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(f64::from);
    Some((p(0)?, p(2)?, p(4)?))
}

fn rgb_hex(r: f64, g: f64, b: f64) -> String {
    let c = |x: f64| ((x / 255.0).clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}

pub fn contrast_text(hex: &str) -> &'static str {
    let Some((r, g, b)) = hex_rgb(hex) else { return "#ffffff" };
    let lin = |c: f64| {
        let c = c / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    if (l + 0.05) / 0.05 > 1.05 / (l + 0.05) { "#0b0e18" } else { "#ffffff" }
}

pub fn auto_secondary(hex: &str) -> String {
    let Some((r, g, b)) = hex_rgb(hex) else { return "#7c3aed".into() };
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let (mut h, mut s, l) = (0.0, 0.0, (mx + mn) / 2.0);
    let d = mx - mn;
    if d != 0.0 {
        s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
        h = if mx == r { (g - b) / d + if g < b { 6.0 } else { 0.0 } } else if mx == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
        h /= 6.0;
    }
    let h = (h + 150.0 / 360.0) % 1.0;
    let l = (l * 0.7 + 0.15).clamp(0.0, 1.0);
    let s = s.max(0.45);
    let hue = |p: f64, q: f64, mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    rgb_hex(hue(p, q, h + 1.0 / 3.0) * 255.0, hue(p, q, h) * 255.0, hue(p, q, h - 1.0 / 3.0) * 255.0)
}

pub fn gradient(a: &str, b: &str) -> String {
    format!("linear-gradient(135deg, {a}, {b})")
}

/// accent_contrast_text / accent_gradient / derived accent_secondary.
pub fn with_derived_accent(mut prefs: Value, accent_key: &str, secondary_key: &str) -> Value {
    let Some(a) = prefs.get(accent_key).and_then(Value::as_str).filter(|a| !a.is_empty()).map(str::to_string) else { return prefs };
    let sec = prefs.get(secondary_key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| auto_secondary(&a));
    prefs["accent_contrast_text"] = json!(contrast_text(&a));
    prefs["accent_gradient"] = json!(gradient(&a, &sec));
    if prefs.get(secondary_key).and_then(Value::as_str).is_none_or(str::is_empty) {
        prefs[secondary_key] = json!(sec);
    }
    prefs
}

// ---- looks ----------------------------------------------------------------------------------
pub fn panel_looks() -> Value {
    json!([
        { "id": "daybreak_console", "title": "Daybreak Console", "note": "Light theme built for bright rooms, without losing the deck's density.",
          "patch": { "theme_mode": "light", "accent_color": "#3b82f6", "background_mode": "default", "layout_mode": "standard", "density": "comfortable", "motion": "reduced" } },
        { "id": "blackout_ops", "title": "Blackout Ops", "note": "High-density, low-noise incident-response look.",
          "patch": { "theme_mode": "dark", "accent_color": "#94a3b8", "accent_secondary": "#e2e8f0", "stream_card_style": "compact", "dashboard_density": "dense", "tab_style": "minimal" } },
        { "id": "neon_arcade", "title": "Neon Arcade", "note": "Loud, glowy, and built for showing the panel off.",
          "patch": { "theme_mode": "dark", "accent_color": "#f472b6", "accent_secondary": "#a855f7", "background_mode": "aurora", "stream_card_style": "cinematic", "card_hover_effect": "glow", "surface_blur": 28 } },
        { "id": "terminal_green", "title": "Terminal Green", "note": "Retro CRT/server-room read: one accent, telemetry cards, ledger roster.",
          "patch": { "theme_mode": "dark", "accent_color": "#22c55e", "accent_secondary": "", "stream_card_style": "telemetry", "roster_layout": "ledger", "panel_font_family": "mono" } },
    ])
}

pub fn profile_looks() -> Value {
    json!([
        { "id": "signal_card", "title": "Signal Card", "note": "Minimal, data-forward -- your bot stats do the talking.", "patch": { "profile_banner_mode": "signal", "profile_card_style": "glass", "profile_border_accent": "pulse" } },
        { "id": "terminal_operator", "title": "Terminal Operator", "note": "Matches the Terminal Green panel look for admins/power users.", "patch": { "profile_card_style": "terminal", "profile_header_style": "solid", "profile_border_accent": "none" } },
        { "id": "quiet_lurker", "title": "Quiet Lurker", "note": "A profile that exists, but doesn't broadcast.", "patch": { "profile_social_mode": "quiet", "profile_banner_mode": "quiet", "profile_border_accent": "none" } },
        { "id": "glass_showcase", "title": "Glass Showcase", "note": "Translucency and depth for a 'look at my setup' profile.", "patch": { "profile_card_style": "glass", "profile_header_style": "blur", "profile_layout_mode": "split" } },
        { "id": "neon_border", "title": "Neon Border", "note": "High-contrast flex look.", "patch": { "profile_border_accent": "neon", "profile_header_style": "transparent" } },
        { "id": "contrast_mode", "title": "Contrast Mode", "note": "Accessibility-first: safe contrast banner and outline card.", "patch": { "profile_banner_mode": "contrast", "profile_card_style": "outline" } },
    ])
}
