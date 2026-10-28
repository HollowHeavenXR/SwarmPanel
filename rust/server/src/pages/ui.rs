//! Shared page components.

use crate::db::J;
use maud::{Markup, html};
use serde_json::Value;

/// 24px stroke icons (inline SVG, inherits currentColor).
pub fn icon(name: &str) -> Markup {
    let d = match name {
        "grid" => "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
        "play" => "M7 4.5v15l12-7.5z",
        "pause" => "M7 5h3.5v14H7zM13.5 5H17v14h-3.5z",
        "skip" => "M5 5l10 7-10 7zM18 5v14",
        "stop" => "M6 6h12v12H6z",
        "link" => "M10 14a4 4 0 0 0 5.6 0l3-3a4 4 0 0 0-5.6-5.6l-1 1M14 10a4 4 0 0 0-5.6 0l-3 3a4 4 0 0 0 5.6 5.6l1-1",
        "trophy" => "M8 4h8v5a4 4 0 0 1-8 0zM8 6H4.5a3 3 0 0 0 3.5 4M16 6h3.5a3 3 0 0 1-3.5 4M12 13v4M8.5 20h7M10 17h4",
        "spark" => "M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5L18 18M6 18l2.5-2.5M15.5 8.5L18 6",
        "users" => "M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7zM2.5 20a6.5 6.5 0 0 1 13 0M16 4.3a3.5 3.5 0 0 1 0 6.4M18 14a6.5 6.5 0 0 1 3.5 6",
        "smile" => "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8.5 14.5a4.5 4.5 0 0 0 7 0M9 9.5h.01M15 9.5h.01",
        "mail" => "M3.5 6h17v12h-17zM3.5 7l8.5 6 8.5-6",
        "user" => "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4.5 20.5a7.5 7.5 0 0 1 15 0",
        "palette" => "M12 3a9 9 0 0 0 0 18c1.2 0 1.8-.9 1.8-1.8 0-1.4-1.2-1.6-1.2-2.9 0-1 .8-1.6 1.8-1.6H17a4 4 0 0 0 4-4C21 6.5 17 3 12 3zM7.5 11h.01M10 7h.01M14.5 7h.01",
        "rocket" => "M14 4c3.5 0 6 2.5 6 6l-7 7-6-6zM7 11l-3 1 2 2M13 17l-1 3-2-2M15.5 8.5h.01M6.5 17.5L4 20",
        "gear" => "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19.4 13.5l1.6 1.2-2 3.4-1.9-.7a7 7 0 0 1-2.1 1.2L14.7 21h-4l-.3-2.4a7 7 0 0 1-2.1-1.2l-1.9.7-2-3.4 1.6-1.2a7 7 0 0 1 0-2.6L4.4 9.7l2-3.4 1.9.7a7 7 0 0 1 2.1-1.2L10.7 3h4l.3 2.4a7 7 0 0 1 2.1 1.2l1.9-.7 2 3.4-1.6 1.2a7 7 0 0 1 0 2.6z",
        "pulse" => "M3 12h4l2.5-6 4 12 2.5-6h5",
        "alert" => "M12 4l9 16H3zM12 10v4M12 17h.01",
        "antenna" => "M12 12v9M8.5 8.5a5 5 0 0 1 7 0M5.5 5.5a9 9 0 0 1 13 0M12 12h.01",
        "list" => "M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01",
        "id" => "M3.5 5h17v14h-17zM9 12a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM5.5 16a3.5 3.5 0 0 1 7 0M15 9h3M15 13h3",
        "db" => "M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
        "image" => "M3.5 4.5h17v15h-17zM3.5 16l5-5 4 4 3-3 5 5M15.5 9h.01",
        "note" => "M9 18V5l11-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM20 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
        "hex" => "M12 2.8l8 4.6v9.2l-8 4.6-8-4.6V7.4zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z",
        "menu" => "M4 7h16M4 12h16M4 17h16",
        "bell" => "M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20.5a2 2 0 0 0 4 0",
        "logout" => "M15 4h4v16h-4M10 8l-4 4 4 4M6 12h10",
        "search" => "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
        "x" => "M6 6l12 12M18 6L6 18",
        "check" => "M5 12.5l4.5 4.5L19 7",
        "refresh" => "M20 11a8 8 0 0 0-14.5-4.5L4 8M4 4v4h4M4 13a8 8 0 0 0 14.5 4.5L20 16M20 20v-4h-4",
        "edit" => "M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4",
        "download" => "M12 4v11M7.5 10.5L12 15l4.5-4.5M4.5 19.5h15",
        "ext" => "M14 4h6v6M20 4l-9 9M18 14v6H4V6h6",
        _ => "M12 12h.01",
    };
    html! { svg.ico viewBox="0 0 24 24" aria-hidden="true" { path d=(d) {} } }
}

pub fn notice(kind: &str, text: &str) -> Markup {
    html! { div class=(format!("notice notice-{kind}")) role=[(kind == "error").then_some("alert")] { (text) } }
}

pub fn empty(text: &str) -> Markup {
    html! { div.empty { (text) } }
}

pub fn section(title: &str, sub: Option<&str>, actions: Option<Markup>) -> Markup {
    html! {
        div.section-head {
            div { h2 { (title) } @if let Some(s) = sub { p { (s) } } }
            @if let Some(a) = actions { div.section-actions { (a) } }
        }
    }
}

pub fn metric(label: &str, value: impl std::fmt::Display, sub: Option<&str>) -> Markup {
    html! { div.metric { span.metric-label { (label) } strong.metric-value { (value) } @if let Some(s) = sub { small { (s) } } } }
}

pub fn metric_warn(label: &str, value: impl std::fmt::Display, warn: bool) -> Markup {
    html! { div.metric.warn[warn] { span.metric-label { (label) } strong.metric-value { (value) } } }
}

pub fn pill(tone: &str, text: &str) -> Markup {
    html! { span class=(format!("pill pill-{tone}")) { (text) } }
}

pub fn json(v: &Value) -> Markup {
    html! { pre.code { (serde_json::to_string_pretty(v).unwrap_or_default()) } }
}

pub fn n(v: &Value, k: &str) -> i64 {
    v.i(k).unwrap_or(0)
}

pub fn s(v: &Value, k: &str) -> String {
    v.s(k).unwrap_or_default()
}

pub fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

pub fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// m:ss
pub fn dur(secs: f64) -> String {
    let t = secs.max(0.0) as i64;
    format!("{}:{:02}", t / 60, t % 60)
}

pub fn uptime(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h {}m", s / 3600, s / 60 % 60),
        _ => format!("{}d {}h", s / 86400, s / 3600 % 24),
    }
}

pub fn bytes(n: f64) -> String {
    if n <= 0.0 {
        return "0 B".into();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let e = ((n.ln() / 1024f64.ln()).floor() as usize).min(units.len() - 1);
    if e == 0 { format!("{n:.0} B") } else { format!("{:.1} {}", n / 1024f64.powi(e as i32), units[e]) }
}

/// Postgres-ish "YYYY-MM-DD HH:MM:SS[.f][+00]" (UTC) -> ISO-8601 with an offset.
pub fn iso(raw: &str) -> String {
    let mut s = raw.trim().replacen(' ', "T", 1);
    if s.is_empty() {
        return s;
    }
    let tail = s.rsplit('T').next().unwrap_or("");
    let has_zone = s.ends_with('Z') || tail.contains('+') || tail.matches('-').count() > 0;
    if s.ends_with("+00") {
        s.push_str(":00");
    } else if !has_zone {
        s.push('Z');
    }
    s
}

/// `<time>` the WASM module rewrites to the viewer's local time.
/// mode: "datetime" | "time" | "ago" | "date"
pub fn time(raw: Option<String>, mode: &str) -> Markup {
    match raw.filter(|r| !r.is_empty()) {
        Some(r) => {
            let iso = iso(&r);
            let shown: String = iso.replace('T', " ").chars().take(16).collect();
            html! { time datetime=(iso) data-local=(mode) { (shown) " UTC" } }
        }
        None => html! { span.muted { "—" } },
    }
}

const MONO_SUFFIXES: [&str; 4] = ["_id", "_hash", "_token", "_uuid"];

/// One cell of a raw schema table (numbers right-aligned, ids monospace, JSON pretty).
pub fn cell(col: &str, v: &Value) -> Markup {
    let text = match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let is_num = matches!(v, Value::Number(_)) || (!text.is_empty() && matches!(v, Value::String(_)) && text.parse::<f64>().is_ok());
    if is_num {
        return html! { td.num { (text) } };
    }
    let c = col.to_lowercase();
    if c == "id" || c == "uuid" || MONO_SUFFIXES.iter().any(|s| c.ends_with(s)) {
        return html! { td.mono { (text) } };
    }
    let t = text.trim();
    if t.len() > 2 && (t.starts_with('{') || t.starts_with('[')) {
        if let Ok(parsed) = serde_json::from_str::<Value>(t) {
            if let Some(items) = parsed.as_array().filter(|a| a.iter().all(|x| x.is_string() || x.is_number())) {
                return html! { td.wide { div.chips { @for x in items { span.chip { (x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())) } } } } };
            }
            return html! { td.wide { pre.code.small { (serde_json::to_string_pretty(&parsed).unwrap_or_default()) } } };
        }
    }
    if let Value::Bool(b) = v {
        return html! { td { (if *b { pill("live", "true") } else { pill("off", "false") }) } };
    }
    html! { td { (text) } }
}

/// Raw rows -> table with the first `max_cols` columns.
pub fn raw_table(rows: &[Value], max_cols: usize, select: Option<&str>, actions: impl Fn(&Value) -> Option<Markup>) -> Markup {
    let Some(first) = rows.first().and_then(Value::as_object) else { return empty("No rows.") };
    let cols: Vec<&String> = first.keys().filter(|k| k.as_str() != "password_hash").take(max_cols).collect();
    let has_actions = rows.iter().any(|r| actions(r).is_some());
    html! {
        div.table-wrap {
            table.table #(select.unwrap_or("raw-table")) {
                thead { tr {
                    @if select.is_some() { th.sel { input type="checkbox" data-select-all aria-label="Select all"; } }
                    @for c in &cols { th { (c) } }
                    @if has_actions { th { "Actions" } }
                } }
                tbody {
                    @for r in rows {
                        tr {
                            @if select.is_some() { td.sel { input type="checkbox" data-select-row value=(r.s("id").unwrap_or_default()) aria-label="Select row"; } }
                            @for c in &cols { (cell(c, r.get(c.as_str()).unwrap_or(&Value::Null))) }
                            @if has_actions { td.actions { @if let Some(m) = actions(r) { (m) } } }
                        }
                    }
                }
            }
        }
    }
}

pub fn initials(label: &str) -> String {
    let parts: Vec<&str> = label.split_whitespace().collect();
    match parts.as_slice() {
        [] => "SP".into(),
        [one] => one.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
        [a, b, ..] => format!("{}{}", a.chars().next().unwrap_or(' '), b.chars().next().unwrap_or(' ')).to_uppercase(),
    }
}

pub fn safe_url(u: &str) -> Option<&str> {
    let t = u.trim();
    (t.starts_with("https://") || t.starts_with("http://")).then_some(t)
}

pub fn avatar(url: Option<String>, label: &str, online: Option<bool>, size: &str) -> Markup {
    let img = url.as_deref().and_then(safe_url).map(str::to_string);
    html! {
        span class=(format!("avatar {size}")) {
            @if let Some(u) = img { img src=(u) alt="" loading="lazy" decoding="async"; } @else { (initials(label)) }
            @if let Some(on) = online { span.presence.on[on] {} }
        }
    }
}

/// Line chart for one metric series ({captured_at, metric_value}).
pub fn trend_chart(label: &str, points: &[Value], anomalies: &[Value], color: &str) -> Markup {
    let pts: Vec<(i64, f64, String)> = points
        .iter()
        .filter_map(|p| {
            let at = p.s("captured_at")?;
            let t = chrono::DateTime::parse_from_rfc3339(&iso(&at)).ok()?.timestamp();
            Some((t, p.f("metric_value").unwrap_or(0.0), at))
        })
        .collect();
    if pts.is_empty() {
        return html! { div.chart.chart-empty { div.chart-head { span { (label) } } (empty(&format!("No {label} history yet"))) } };
    }
    let (w, h, pl, pr, pt, pb) = (560.0, 160.0, 14.0, 14.0, 12.0, 22.0);
    let (iw, ih) = (w - pl - pr, h - pt - pb);
    let (t0, t1) = (pts.iter().map(|p| p.0).min().unwrap_or(0), pts.iter().map(|p| p.0).max().unwrap_or(1));
    let lo = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let hi = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let pad = ((hi - lo) * 0.12).max(1.0);
    let (vmin, vmax) = ((lo - pad).max(0.0), hi + pad);
    let x = |t: i64| pl + (t - t0) as f64 / ((t1 - t0).max(1) as f64) * iw;
    let y = |v: f64| pt + ih - (v - vmin) / ((vmax - vmin).max(1e-9)) * ih;
    let line: String = pts.iter().enumerate().map(|(i, p)| format!("{}{:.1},{:.1}", if i == 0 { "M" } else { "L" }, x(p.0), y(p.1))).collect::<Vec<_>>().join(" ");
    let area = format!("{line} L{:.1},{:.1} L{:.1},{:.1} Z", x(pts[pts.len() - 1].0), pt + ih, x(pts[0].0), pt + ih);
    let flagged: std::collections::HashSet<String> = anomalies.iter().filter_map(|a| a.s("captured_at")).collect();
    let gid = format!("g{}", label.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>());
    let latest = pts[pts.len() - 1].1;
    html! {
        div.chart {
            div.chart-head { span { (label) } strong { (latest) } }
            svg.chart-svg viewBox=(format!("0 0 {w} {h}")) role="img" aria-label=(format!("{label}, latest {latest}")) {
                defs { linearGradient id=(gid) x1="0" y1="0" x2="0" y2="1" {
                    stop offset="0%" stop-color=(color) stop-opacity="0.3" {}
                    stop offset="100%" stop-color=(color) stop-opacity="0" {}
                } }
                line.axis x1=(pl) x2=(w - pr) y1=(pt + ih) y2=(pt + ih) {}
                path d=(area) fill=(format!("url(#{gid})")) {}
                path d=(line) fill="none" stroke=(color) stroke-width="2" stroke-linejoin="round" stroke-linecap="round" {}
                @for p in &pts {
                    @let bad = flagged.contains(&p.2);
                    circle.pt.bad[bad] cx=(format!("{:.1}", x(p.0))) cy=(format!("{:.1}", y(p.1))) r=(if bad { "4.5" } else { "6" }) {
                        title { (p.1) " at " (iso(&p.2).replace('T', " ").chars().take(16).collect::<String>()) " UTC" @if bad { " (anomaly)" } }
                    }
                }
            }
        }
    }
}


