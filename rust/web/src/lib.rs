//! SwarmPanel browser module. Pages are rendered on the server; this module
//! adds behaviour through data attributes. Contract:
//!
//! Live regions
//!   [data-src="/frag/x?.."]        re-fetchable HTML for this element (needs an id)
//!   [data-live="key"]              refresh when /ws pushes a snapshot for `key`
//!   [data-live-params='{..}']      params sent with the watch
//!   [data-poll="30"]               refresh every N seconds while visible
//!   [data-lazy]                    don't refresh on snapshots until loaded once
//!   [data-scroll="bottom"]         keep scrolled to the bottom (chat)
//!   [data-bind="key:a+b"]          text = sum of snapshot fields; hidden when 0
//!
//! Actions
//!   [data-post="/api/.."]          POST JSON: data-body, data-confirm, data-prompt +
//!                                  data-prompt-key (+ data-prompt-value), data-ids="#table"
//!   form[data-api="/api/{field}"]  POST the form as JSON (names may be a.b / a[0].b;
//!                                  data-t=int|num|csv; checkboxes are booleans;
//!                                  data-omit-empty; data-reset)
//!   data-ok / data-result="#el" / data-done=reload|refresh|refresh:#a,#b|go:/path
//!   form[data-get="#region"]       GET filters that re-render a region (debounced)
//!   form[data-autosubmit]          submit on change
//!   [data-refresh="#id"|"all"], [data-toggle="#id"], [data-close="#id"]
//!   [data-fill='{..}' data-fill-form="#form"]   set form fields
//!   form[data-pref-preview]        live-preview appearance fields ([data-pref], [data-var])
//!   [data-select-all] / [data-select-row] / [data-bulk-for="#table"]
//!   [data-playback] / [data-seek]  ticking position + drag-to-seek
//!   time[data-local]               rendered in the viewer's local time

use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Document, Element, Event, HtmlElement, HtmlFormElement, HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement, MessageEvent, PointerEvent, Request, RequestInit, Response, WebSocket};

#[derive(Default)]
struct Region {
    busy: bool,
    dirty: bool,
    last: f64,
    queued: bool,
}

struct Seek {
    el: Element,
    left: f64,
    width: f64,
    dur: f64,
    frac: f64,
}

#[derive(Default)]
struct State {
    token: Option<String>,
    ws: Option<WebSocket>,
    attempt: u32,
    stopped: bool,
    reconnect_pending: bool,
    watchdog: Option<i32>,
    regions: HashMap<String, Region>,
    debounce: HashMap<String, i32>,
    seek: Option<Seek>,
    toast_timer: Option<i32>,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

fn win() -> web_sys::Window {
    web_sys::window().expect("window")
}
fn doc() -> Document {
    win().document().expect("document")
}
fn now() -> f64 {
    js_sys::Date::now()
}
fn attr(el: &Element, k: &str) -> Option<String> {
    el.get_attribute(k)
}
fn all(root: &Element, sel: &str) -> Vec<Element> {
    let list = root.query_selector_all(sel).ok();
    let mut out = Vec::new();
    if let Some(l) = list {
        for i in 0..l.length() {
            if let Some(e) = l.item(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                out.push(e);
            }
        }
    }
    out
}
fn all_doc(sel: &str) -> Vec<Element> {
    doc().document_element().map(|r| all(&r, sel)).unwrap_or_default()
}
fn by_sel(sel: &str) -> Option<Element> {
    doc().query_selector(sel).ok().flatten()
}
fn closest(target: &Option<web_sys::EventTarget>, sel: &str) -> Option<Element> {
    target.as_ref()?.dyn_ref::<Element>()?.closest(sel).ok().flatten()
}
fn timeout(ms: i32, f: impl FnOnce() + 'static) -> i32 {
    let cb = Closure::once_into_js(f);
    win().set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms).unwrap_or(0)
}
fn interval(ms: i32, f: impl FnMut() + 'static) {
    let cb = Closure::<dyn FnMut()>::new(f).into_js_value();
    let _ = win().set_interval_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms);
}
fn on(target: &web_sys::EventTarget, ev: &str, f: impl FnMut(Event) + 'static) {
    let cb = Closure::<dyn FnMut(Event)>::new(f).into_js_value();
    let _ = target.add_event_listener_with_callback(ev, cb.unchecked_ref());
}
fn hidden_page() -> bool {
    doc().hidden()
}
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---- toast ----------------------------------------------------------------------------------------
fn toast(msg: &str, kind: &str) {
    let Some(el) = by_sel("#toast") else { return };
    el.set_text_content(Some(msg));
    el.set_class_name(&format!("toast show toast-{kind}"));
    let id = timeout(3600, move || {
        if let Some(el) = by_sel("#toast") {
            el.set_class_name("toast");
        }
    });
    ST.with(|s| {
        if let Some(old) = s.borrow_mut().toast_timer.replace(id) {
            win().clear_timeout_with_handle(old);
        }
    });
}

// ---- fetch ----------------------------------------------------------------------------------------
async fn http(method: &str, url: &str, body: Option<String>) -> Result<(u16, String), String> {
    let opts = RequestInit::new();
    opts.set_method(method);
    if let Some(b) = &body {
        opts.set_body(&JsValue::from_str(b));
    }
    let req = Request::new_with_str_and_init(url, &opts).map_err(|_| "Bad request".to_string())?;
    let h = req.headers();
    if body.is_some() {
        let _ = h.set("Content-Type", "application/json");
    }
    if let Some(t) = ST.with(|s| s.borrow().token.clone()) {
        let _ = h.set("Authorization", &format!("Bearer {t}"));
    }
    let resp: Response = JsFuture::from(win().fetch_with_request(&req)).await.map_err(|_| "Network error".to_string())?.dyn_into().map_err(|_| "Network error".to_string())?;
    let text = match resp.text() {
        Ok(p) => JsFuture::from(p).await.ok().and_then(|v| v.as_string()).unwrap_or_default(),
        Err(_) => String::new(),
    };
    Ok((resp.status(), text))
}

fn detail(text: &str, status: u16) -> String {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v.get("detail").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| format!("Request failed ({status})"))
}

// ---- regions ----------------------------------------------------------------------------------------
fn open_summaries(el: &Element) -> Vec<String> {
    all(el, "details[open] > summary").iter().filter_map(|s| s.text_content()).collect()
}

fn swap(el: &Element, html: &str) {
    let keep = open_summaries(el);
    let bottom = attr(el, "data-scroll").as_deref() == Some("bottom");
    let near_bottom = bottom && (el.scroll_height() - el.scroll_top() - el.client_height() < 60 || attr(el, "data-swapped").is_none());
    el.set_inner_html(html);
    let _ = el.set_attribute("data-swapped", "1");
    for d in all(el, "details") {
        if let Some(s) = d.query_selector("summary").ok().flatten().and_then(|s| s.text_content()) {
            if keep.contains(&s) {
                let _ = d.set_attribute("open", "");
            }
        }
    }
    if near_bottom {
        el.set_scroll_top(el.scroll_height());
    }
    after_render(el);
}

/// Something inside is being interacted with; wait before replacing it.
fn busy_inside(el: &Element) -> bool {
    if el.query_selector(".seeking").ok().flatten().is_some() {
        return true;
    }
    if let Some(a) = doc().active_element() {
        if el.contains(Some(&a)) && matches!(a.tag_name().as_str(), "INPUT" | "SELECT" | "TEXTAREA") {
            return true;
        }
    }
    false
}

fn refresh(id: String, force: bool) {
    let Some(el) = doc().get_element_by_id(&id) else { return };
    let Some(src) = attr(&el, "data-src") else { return };
    let wait = ST.with(|s| {
        let mut s = s.borrow_mut();
        let r = s.regions.entry(id.clone()).or_default();
        if r.busy {
            r.dirty = true;
            return None;
        }
        let since = now() - r.last;
        if !force && since < 1000.0 {
            if r.queued {
                return None;
            }
            r.queued = true;
            return Some((1000.0 - since) as i32);
        }
        r.busy = true;
        r.queued = false;
        r.last = now();
        Some(0)
    });
    match wait {
        None => {}
        Some(ms) if ms > 0 => {
            let id2 = id.clone();
            timeout(ms, move || {
                ST.with(|s| {
                    if let Some(r) = s.borrow_mut().regions.get_mut(&id2) {
                        r.queued = false;
                    }
                });
                refresh(id2, true);
            });
        }
        Some(_) => spawn_local(async move {
            let res = http("GET", &src, None).await;
            let mut again = false;
            if let Ok((status, text)) = res {
                if status == 401 {
                    ST.with(|s| s.borrow_mut().stopped = true);
                } else if status < 400 {
                    if let Some(el) = doc().get_element_by_id(&id) {
                        if busy_inside(&el) {
                            again = true;
                        } else {
                            swap(&el, &text);
                            let _ = el.set_attribute("data-loaded", "1");
                        }
                    }
                }
            }
            let dirty = ST.with(|s| {
                let mut s = s.borrow_mut();
                let r = s.regions.entry(id.clone()).or_default();
                r.busy = false;
                std::mem::take(&mut r.dirty)
            });
            if dirty || again {
                let id2 = id.clone();
                timeout(if again { 1500 } else { 1000 }, move || refresh(id2, true));
            }
        }),
    }
}

fn refresh_all() {
    for el in all_doc("[data-src]") {
        if attr(&el, "data-lazy").is_some() && attr(&el, "data-loaded").is_none() {
            continue;
        }
        if !el.id().is_empty() {
            refresh(el.id(), true);
        }
    }
}

fn refresh_spec(spec: &str) {
    if spec == "all" || spec.is_empty() {
        return refresh_all();
    }
    for sel in spec.split(',') {
        if let Some(el) = by_sel(sel.trim()) {
            refresh(el.id(), true);
        }
    }
}

fn done(spec: Option<String>) {
    match spec.as_deref() {
        Some("reload") => {
            let _ = win().location().reload();
        }
        Some(s) if s.starts_with("go:") => {
            let _ = win().location().assign(&s[3..]);
        }
        Some("refresh") => refresh_all(),
        Some(s) if s.starts_with("refresh:") => refresh_spec(&s[8..]),
        _ => {}
    }
}

// ---- snapshots / binds ------------------------------------------------------------------------------
fn on_snapshot(key: &str, data: &Value) {
    for el in all_doc("[data-bind]") {
        let Some(spec) = attr(&el, "data-bind") else { continue };
        let Some((k, fields)) = spec.split_once(':') else { continue };
        if k != key {
            continue;
        }
        let n: i64 = fields.split('+').map(|f| data.get(f).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|x| x as i64))).unwrap_or(0)).sum();
        el.set_text_content(Some(&if n > 99 { "99+".into() } else { n.to_string() }));
        if let Some(h) = el.dyn_ref::<HtmlElement>() {
            h.set_hidden(n <= 0);
        }
    }
    for el in all_doc(&format!("[data-live=\"{key}\"]")) {
        if attr(&el, "data-lazy").is_some() && attr(&el, "data-loaded").is_none() {
            continue;
        }
        if !el.id().is_empty() {
            refresh(el.id(), false);
        }
    }
}

// ---- websocket ---------------------------------------------------------------------------------------
fn watched() -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();
    for el in all_doc("[data-live]") {
        let k = attr(&el, "data-live").unwrap_or_default();
        let p = attr(&el, "data-live-params").and_then(|p| serde_json::from_str(&p).ok()).unwrap_or(Value::Null);
        if !out.iter().any(|(x, _)| *x == k) {
            out.push((k, p));
        }
    }
    for el in all_doc("[data-bind]") {
        if let Some(k) = attr(&el, "data-bind").and_then(|s| s.split_once(':').map(|(k, _)| k.to_string())) {
            if !out.iter().any(|(x, _)| *x == k) {
                out.push((k, Value::Null));
            }
        }
    }
    out
}

fn send(ws: &WebSocket, v: Value) {
    if ws.ready_state() == WebSocket::OPEN {
        let _ = ws.send_with_str(&v.to_string());
    }
}

fn arm_watchdog() {
    let id = timeout(100_000, || {
        if let Some(ws) = ST.with(|s| s.borrow().ws.clone()) {
            let _ = ws.close();
        }
    });
    ST.with(|s| {
        if let Some(old) = s.borrow_mut().watchdog.replace(id) {
            win().clear_timeout_with_handle(old);
        }
    });
}

fn live() -> bool {
    ST.with(|s| s.borrow().ws.as_ref().is_some_and(|w| w.ready_state() == WebSocket::OPEN || w.ready_state() == WebSocket::CONNECTING))
}

fn schedule_reconnect() {
    let delay = ST.with(|s| {
        let mut s = s.borrow_mut();
        if s.reconnect_pending || s.stopped {
            return None;
        }
        s.reconnect_pending = true;
        let base = (1000.0 * 2f64.powi(s.attempt.min(4) as i32)).min(15000.0);
        s.attempt += 1;
        Some((base / 2.0 + js_sys::Math::random() * base / 2.0) as i32)
    });
    if let Some(ms) = delay {
        timeout(ms, || {
            ST.with(|s| s.borrow_mut().reconnect_pending = false);
            if !hidden_page() {
                connect();
            }
        });
    }
}

fn connect() {
    let token = ST.with(|s| if s.borrow().stopped { None } else { s.borrow().token.clone() });
    let Some(token) = token else { return };
    if live() {
        return;
    }
    let loc = win().location();
    let proto = if loc.protocol().unwrap_or_default() == "https:" { "wss:" } else { "ws:" };
    let Ok(ws) = WebSocket::new(&format!("{proto}//{}/ws", loc.host().unwrap_or_default())) else { return };
    ST.with(|s| s.borrow_mut().ws = Some(ws.clone()));
    let w = ws.clone();
    on(&ws, "open", move |_| {
        ST.with(|s| s.borrow_mut().attempt = 0);
        send(&w, json!({ "type": "auth", "token": token }));
        for (k, p) in watched() {
            send(&w, if p.is_null() { json!({ "type": "watch", "key": k }) } else { json!({ "type": "watch", "key": k, "params": p }) });
        }
        arm_watchdog();
    });
    let w = ws.clone();
    on(&ws, "message", move |e| {
        let Some(m) = e.dyn_ref::<MessageEvent>().and_then(|m| m.data().as_string()) else { return };
        arm_watchdog();
        let Ok(v) = serde_json::from_str::<Value>(&m) else { return };
        match v.get("type").and_then(Value::as_str) {
            Some("ping") => send(&w, json!({ "type": "pong" })),
            Some("snapshot") => {
                if let Some(k) = v.get("key").and_then(Value::as_str) {
                    on_snapshot(k, v.get("data").unwrap_or(&Value::Null));
                }
            }
            _ => {}
        }
    });
    let w = ws.clone();
    on(&ws, "close", move |e| {
        let same = ST.with(|s| s.borrow().ws.as_ref().is_some_and(|x| *x == w));
        if !same {
            return;
        }
        if e.dyn_ref::<web_sys::CloseEvent>().is_some_and(|c| c.code() == 4401) {
            ST.with(|s| s.borrow_mut().stopped = true);
            return;
        }
        if !hidden_page() {
            schedule_reconnect();
        }
    });
    let w = ws.clone();
    on(&ws, "error", move |_| {
        let _ = w.close();
    });
}

// ---- forms -------------------------------------------------------------------------------------------
enum Seg {
    Key(String),
    Idx(usize),
}

fn parse_path(name: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    for part in name.split('.') {
        let mut rest = part;
        if let Some(i) = rest.find('[') {
            out.push(Seg::Key(rest[..i].to_string()));
            rest = &rest[i..];
            while let Some(stripped) = rest.strip_prefix('[') {
                let end = stripped.find(']').unwrap_or(stripped.len());
                out.push(Seg::Idx(stripped[..end].parse().unwrap_or(0)));
                rest = stripped.get(end + 1..).unwrap_or("");
            }
        } else {
            out.push(Seg::Key(rest.to_string()));
        }
    }
    out
}

fn put(root: &mut Value, path: &[Seg], v: Value) {
    let Some((head, tail)) = path.split_first() else { return };
    let next_is_idx = matches!(tail.first(), Some(Seg::Idx(_)));
    let blank = || if next_is_idx { json!([]) } else { json!({}) };
    match head {
        Seg::Key(k) => {
            if !root.is_object() {
                *root = json!({});
            }
            let m = root.as_object_mut().unwrap();
            if tail.is_empty() {
                m.insert(k.clone(), v);
            } else {
                put(m.entry(k.clone()).or_insert_with(blank), tail, v);
            }
        }
        Seg::Idx(i) => {
            if !root.is_array() {
                *root = json!([]);
            }
            let a = root.as_array_mut().unwrap();
            while a.len() <= *i {
                a.push(Value::Null);
            }
            if tail.is_empty() {
                a[*i] = v;
            } else {
                if a[*i].is_null() {
                    a[*i] = blank();
                }
                put(&mut a[*i], tail, v);
            }
        }
    }
}

/// Arrays of objects lose entries with any empty string (half-filled link rows).
fn prune(v: &mut Value) {
    match v {
        Value::Array(a) => {
            a.retain(|x| !x.is_null() && !x.as_object().is_some_and(|o| o.values().any(|f| f.as_str().is_some_and(|s| s.trim().is_empty()))));
            a.iter_mut().for_each(prune);
        }
        Value::Object(o) => o.values_mut().for_each(prune),
        _ => {}
    }
}

fn rendered(el: &Element) -> bool {
    match el.closest(".field, .check").ok().flatten() {
        Some(f) => f.dyn_ref::<HtmlElement>().is_some_and(|h| h.offset_parent().is_some() || h.get_client_rects().length() > 0),
        None => true,
    }
}

fn field_value(el: &Element) -> Option<(String, Value)> {
    let name = attr(el, "name").filter(|n| !n.is_empty())?;
    if el.has_attribute("disabled") || !rendered(el) {
        return None;
    }
    let t = attr(el, "data-t").unwrap_or_default();
    let raw = if let Some(i) = el.dyn_ref::<HtmlInputElement>() {
        match i.type_().as_str() {
            "checkbox" => return Some((name, Value::Bool(i.checked()))),
            "radio" if !i.checked() => return None,
            "submit" | "button" | "file" => return None,
            _ => i.value(),
        }
    } else if let Some(s) = el.dyn_ref::<HtmlSelectElement>() {
        s.value()
    } else if let Some(t) = el.dyn_ref::<HtmlTextAreaElement>() {
        t.value()
    } else {
        return None;
    };
    let v = match t.as_str() {
        "int" => raw.trim().parse::<i64>().map(Value::from).unwrap_or(Value::Null),
        "num" => raw.trim().parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::Null),
        "csv" => Value::Array(raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| Value::String(s.into())).collect()),
        _ => Value::String(raw),
    };
    Some((name, v))
}

fn serialize(form: &Element) -> Value {
    let omit = form.has_attribute("data-omit-empty");
    let mut root = Value::Object(Map::new());
    for el in all(form, "input, select, textarea") {
        if let Some((name, v)) = field_value(&el) {
            if omit && (v.as_str().is_some_and(|s| s.trim().is_empty()) || v.is_null()) {
                continue;
            }
            put(&mut root, &parse_path(&name), v);
        }
    }
    prune(&mut root);
    root
}

fn fill_url(tpl: &str, body: &Value) -> String {
    let mut out = tpl.to_string();
    while let (Some(a), Some(b)) = (out.find('{'), out.find('}')) {
        if b < a {
            break;
        }
        let k = &out[a + 1..b];
        let v = body.get(k).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
        out = format!("{}{}{}", &out[..a], js_sys::encode_uri_component(&v), &out[b + 1..]);
    }
    out
}

fn set_disabled(el: &Element, on: bool) {
    if on {
        let _ = el.set_attribute("disabled", "");
        let _ = el.set_attribute("aria-busy", "true");
    } else {
        let _ = el.remove_attribute("disabled");
        let _ = el.remove_attribute("aria-busy");
    }
}

fn show_result(src: &Element, kind: &str, msg: &str) {
    if let Some(box_) = attr(src, "data-result").and_then(|s| by_sel(&s)) {
        box_.set_inner_html(&format!("<div class=\"notice notice-{kind}\">{}</div>", esc(msg)));
    }
}

async fn post(src: Element, url: String, body: Value, lock: Element) {
    set_disabled(&lock, true);
    let res = http("POST", &url, Some(body.to_string())).await;
    set_disabled(&lock, false);
    match res {
        Ok((status, text)) if status < 400 => {
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let msg = attr(&src, "data-ok").or_else(|| v.get("message").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| "Done.".into());
            show_result(&src, "success", &msg);
            toast(&msg, "success");
            if src.has_attribute("data-reset") {
                if let Some(f) = src.dyn_ref::<HtmlFormElement>() {
                    f.reset();
                }
            }
            done(attr(&src, "data-done"));
        }
        Ok((status, text)) => {
            if status == 401 {
                let _ = win().location().assign(&format!("/login?next={}", js_sys::encode_uri_component(&win().location().pathname().unwrap_or_default())));
                return;
            }
            let d = detail(&text, status);
            show_result(&src, "error", &d);
            toast(&d, "error");
        }
        Err(e) => toast(&e, "error"),
    }
}

fn click_post(el: Element) {
    let mut body: Value = attr(&el, "data-body").and_then(|b| serde_json::from_str(&b).ok()).unwrap_or(json!({}));
    if let Some(c) = attr(&el, "data-confirm") {
        if !win().confirm_with_message(&c).unwrap_or(false) {
            return;
        }
    }
    if let Some(label) = attr(&el, "data-prompt") {
        let def = attr(&el, "data-prompt-value").unwrap_or_default();
        match win().prompt_with_message_and_default(&label, &def) {
            Ok(Some(v)) if !v.trim().is_empty() => body[attr(&el, "data-prompt-key").unwrap_or_else(|| "value".into())] = json!(v.trim()),
            _ => return,
        }
    }
    if let Some(t) = attr(&el, "data-ids").and_then(|s| by_sel(&s)) {
        let ids: Vec<Value> = all(&t, "[data-select-row]:checked")
            .iter()
            .filter_map(|b| b.dyn_ref::<HtmlInputElement>().map(|i| i.value()))
            .map(|v| v.parse::<i64>().map(Value::from).unwrap_or(Value::String(v)))
            .collect();
        if ids.is_empty() {
            return toast("Select at least one row.", "error");
        }
        body["ids"] = Value::Array(ids);
    }
    let url = attr(&el, "data-post").unwrap_or_default();
    spawn_local(post(el.clone(), url, body, el));
}

fn submit_api(form: Element) {
    let body = serialize(&form);
    let url = fill_url(&attr(&form, "data-api").unwrap_or_default(), &body);
    if let Some(c) = attr(&form, "data-confirm") {
        if !win().confirm_with_message(&c).unwrap_or(false) {
            return;
        }
    }
    let lock = form.query_selector("[type=submit]").ok().flatten().unwrap_or_else(|| form.clone());
    spawn_local(post(form.clone(), url, body, lock));
}

fn query_string(form: &Element) -> String {
    let mut parts = Vec::new();
    for el in all(form, "input, select") {
        let Some(name) = attr(&el, "name") else { continue };
        let v = if let Some(i) = el.dyn_ref::<HtmlInputElement>() {
            match i.type_().as_str() {
                "checkbox" | "radio" if !i.checked() => continue,
                _ => i.value(),
            }
        } else if let Some(s) = el.dyn_ref::<HtmlSelectElement>() {
            s.value()
        } else {
            continue;
        };
        if v.is_empty() {
            continue;
        }
        parts.push(format!("{}={}", js_sys::encode_uri_component(&name), js_sys::encode_uri_component(&v)));
    }
    parts.join("&")
}

fn run_get(form: &Element) {
    let Some(target) = attr(form, "data-get").and_then(|s| by_sel(&s)) else { return };
    let qs = query_string(form);
    let base = attr(&target, "data-src").unwrap_or_default().split('?').next().unwrap_or("").to_string();
    let _ = target.set_attribute("data-src", &if qs.is_empty() { base } else { format!("{base}?{qs}") });
    if target.id() != "people" {
        let path = win().location().pathname().unwrap_or_default();
        let url = if qs.is_empty() { path } else { format!("{path}?{qs}") };
        let _ = win().history().and_then(|h| h.replace_state_with_url(&JsValue::NULL, "", Some(&url)));
    }
    refresh(target.id(), true);
}

fn debounce_get(form: Element) {
    let key = attr(&form, "data-get").unwrap_or_default();
    let f2 = form.clone();
    let id = timeout(250, move || run_get(&f2));
    ST.with(|s| {
        if let Some(old) = s.borrow_mut().debounce.insert(key, id) {
            win().clear_timeout_with_handle(old);
        }
    });
}

// ---- appearance preview -------------------------------------------------------------------------------
fn preview_field(el: &Element) {
    let Some(name) = attr(el, "name") else { return };
    let root = doc().document_element().unwrap();
    let (val, checked) = if let Some(i) = el.dyn_ref::<HtmlInputElement>() { (i.value(), i.type_() == "checkbox").clone() } else if let Some(s) = el.dyn_ref::<HtmlSelectElement>() { (s.value(), false) } else { return };
    if el.has_attribute("data-pref") {
        let v = if checked { el.dyn_ref::<HtmlInputElement>().map(|i| i.checked().to_string()).unwrap_or_default() } else { val.clone() };
        let _ = root.set_attribute(&format!("data-p-{name}"), &v);
    }
    if let Some(var) = attr(el, "data-var").filter(|v| !v.is_empty()) {
        let unit = attr(el, "data-var-unit").unwrap_or_default();
        if let Some(h) = root.dyn_ref::<HtmlElement>() {
            let _ = h.style().set_property(&var, &format!("{val}{unit}"));
        }
    }
    if let Some(out) = el.closest(".field").ok().flatten().and_then(|f| f.query_selector("output").ok().flatten()) {
        out.set_text_content(Some(&format!("{val}{}", attr(el, "data-var-unit").unwrap_or_default())));
    }
}

fn fill(btn: &Element) {
    let Some(form) = attr(btn, "data-fill-form").and_then(|s| by_sel(&s)) else { return };
    let Some(patch) = attr(btn, "data-fill").and_then(|p| serde_json::from_str::<Map<String, Value>>(&p).ok()) else { return };
    for (k, v) in patch {
        let Some(el) = form.query_selector(&format!("[name=\"{k}\"]")).ok().flatten() else { continue };
        let text = match &v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            o => o.to_string(),
        };
        if let Some(i) = el.dyn_ref::<HtmlInputElement>() {
            if i.type_() == "checkbox" {
                i.set_checked(v.as_bool().unwrap_or(text == "true"));
            } else if i.type_() == "color" && text.is_empty() {
                continue;
            } else {
                i.set_value(&text);
            }
        } else if let Some(s) = el.dyn_ref::<HtmlSelectElement>() {
            s.set_value(&text);
        } else if let Some(t) = el.dyn_ref::<HtmlTextAreaElement>() {
            t.set_value(&text);
        }
        if form.has_attribute("data-pref-preview") {
            preview_field(&el);
        }
    }
    toast("Applied. Save to keep it.", "success");
}

// ---- playback + seek ----------------------------------------------------------------------------------
fn fmt_dur(secs: f64) -> String {
    let t = secs.max(0.0) as i64;
    format!("{}:{:02}", t / 60, t % 60)
}

fn paint(pb: &Element, frac: f64, dur: f64) {
    let pct = format!("{:.2}%", (frac * 100.0).clamp(0.0, 100.0));
    for (sel, prop) in [("[data-fill]", "width"), ("[data-thumb]", "left")] {
        if let Some(h) = pb.query_selector(sel).ok().flatten().and_then(|e| e.dyn_into::<HtmlElement>().ok()) {
            let _ = h.style().set_property(prop, &pct);
        }
    }
    if let Some(c) = pb.query_selector("[data-cur]").ok().flatten() {
        c.set_text_content(Some(&fmt_dur(frac * dur)));
    }
}

fn tick(root: &Element) {
    for pb in all(root, "[data-playback]") {
        if pb.query_selector(".seeking").ok().flatten().is_some() {
            continue;
        }
        let num = |k: &str| attr(&pb, k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let (mut pos, dur) = (num("data-pos"), num("data-dur"));
        if attr(&pb, "data-playing").as_deref() == Some("true") {
            let at = js_sys::Date::parse(&attr(&pb, "data-at").unwrap_or_default());
            if at.is_finite() {
                pos += (now() - at) / 1000.0;
            }
        }
        if dur > 0.0 {
            pos = pos.min(dur);
            paint(&pb, pos / dur, dur);
        } else if let Some(c) = pb.query_selector("[data-cur]").ok().flatten() {
            c.set_text_content(Some(&fmt_dur(pos)));
        }
    }
}

fn seek_frac(s: &Seek, x: f64) -> f64 {
    if s.width > 0.0 { ((x - s.left) / s.width).clamp(0.0, 1.0) } else { 0.0 }
}

fn seek_start(e: &PointerEvent) {
    if e.button() != 0 {
        return;
    }
    let Some(bar) = closest(&e.target(), "[data-seek]") else { return };
    let Some(pb) = bar.closest("[data-playback]").ok().flatten() else { return };
    let dur = attr(&pb, "data-dur").and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0);
    if dur <= 0.0 {
        return;
    }
    e.prevent_default();
    let r = bar.get_bounding_client_rect();
    let _ = bar.class_list().add_1("seeking");
    let mut s = Seek { el: bar, left: r.left(), width: r.width(), dur, frac: 0.0 };
    s.frac = seek_frac(&s, e.client_x() as f64);
    paint(&pb, s.frac, dur);
    ST.with(|st| st.borrow_mut().seek = Some(s));
}

fn seek_move(e: &PointerEvent) {
    ST.with(|st| {
        if let Some(s) = st.borrow_mut().seek.as_mut() {
            s.frac = seek_frac(s, e.client_x() as f64);
            if let Some(pb) = s.el.closest("[data-playback]").ok().flatten() {
                paint(&pb, s.frac, s.dur);
            }
        }
    });
}

fn seek_end(e: &PointerEvent, commit: bool) {
    let Some(mut s) = ST.with(|st| st.borrow_mut().seek.take()) else { return };
    let _ = s.el.class_list().remove_1("seeking");
    if !commit {
        return;
    }
    s.frac = seek_frac(&s, e.client_x() as f64);
    let pos = (s.frac * s.dur).round();
    if let Some(pb) = s.el.closest("[data-playback]").ok().flatten() {
        let _ = pb.set_attribute("data-pos", &pos.to_string());
        let _ = pb.set_attribute("data-at", &js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default());
    }
    let body = json!({ "bot_key": attr(&s.el, "data-seek"), "guild_id": attr(&s.el, "data-guild"), "action": "SEEK", "payload": { "position_seconds": pos } });
    spawn_local(async move {
        match http("POST", "/api/bots/control", Some(body.to_string())).await {
            Ok((st, _)) if st < 400 => toast(&format!("Seeking to {}.", fmt_dur(pos)), "success"),
            Ok((st, t)) => toast(&detail(&t, st), "error"),
            Err(e) => toast(&e, "error"),
        }
    });
}

// ---- local times --------------------------------------------------------------------------------------
fn localize(root: &Element) {
    for t in all(root, "time[data-local]") {
        let ms = js_sys::Date::parse(&attr(&t, "datetime").unwrap_or_default());
        if !ms.is_finite() {
            continue;
        }
        let d = js_sys::Date::new(&JsValue::from_f64(ms));
        let text = match attr(&t, "data-local").as_deref() {
            Some("ago") => {
                let s = ((now() - ms) / 1000.0).max(0.0) as i64;
                match s {
                    0..60 => "just now".to_string(),
                    60..3600 => format!("{}m ago", s / 60),
                    3600..86400 => format!("{}h ago", s / 3600),
                    _ => format!("{}d ago", s / 86400),
                }
            }
            Some("time") => {
                let today = js_sys::Date::new_0().to_date_string() == d.to_date_string();
                if today {
                    d.to_locale_time_string("default").as_string().unwrap_or_default().rsplitn(2, ':').last().unwrap_or("").to_string()
                } else {
                    d.to_locale_date_string("default", &JsValue::UNDEFINED).as_string().unwrap_or_default()
                }
            }
            Some("date") => d.to_locale_date_string("default", &JsValue::UNDEFINED).as_string().unwrap_or_default(),
            _ => d.to_locale_string("default", &JsValue::UNDEFINED).as_string().unwrap_or_default(),
        };
        let _ = t.set_attribute("title", &d.to_locale_string("default", &JsValue::UNDEFINED).as_string().unwrap_or_default());
        t.set_text_content(Some(&text));
    }
}

fn after_render(root: &Element) {
    localize(root);
    tick(root);
}

// ---- bulk select ----------------------------------------------------------------------------------------
fn update_bulk(table: &Element) {
    let any = !all(table, "[data-select-row]:checked").is_empty();
    if table.id().is_empty() {
        return;
    }
    for bar in all_doc(&format!("[data-bulk-for=\"#{}\"]", table.id())) {
        if let Some(h) = bar.dyn_ref::<HtmlElement>() {
            h.set_hidden(!any);
        }
    }
}

// ---- popovers / drawer ----------------------------------------------------------------------------------
fn toggle(sel: &str, force: Option<bool>) {
    let Some(t) = by_sel(sel) else { return };
    let open = t.class_list().toggle_with_force("open", force.unwrap_or(!t.class_list().contains("open"))).unwrap_or(false);
    for b in all_doc(&format!("[data-toggle=\"{sel}\"]")) {
        let _ = b.set_attribute("aria-expanded", if open { "true" } else { "false" });
    }
    if t.id() == "sidebar" || t.id() == "drawer" || t.id() == "palette" {
        let _ = doc().body().map(|b| b.class_list().toggle_with_force("drawer-open", open));
    }
    if open {
        for r in all(&t, "[data-src]") {
            refresh(r.id(), true);
        }
    }
}

fn close_pops(except: Option<&Element>) {
    for p in all_doc(".pop.open, .sidebar.open, .drawer.open, .palette.open") {
        if except.is_some_and(|e| p.contains(Some(e))) {
            continue;
        }
        toggle(&format!("#{}", p.id()), Some(false));
    }
}

// ---- slide-over drawer ----------------------------------------------------------------------------
fn open_drawer(src: &str) {
    let Some(body) = doc().get_element_by_id("drawer-body") else { return };
    let _ = body.set_attribute("data-src", src);
    let _ = body.remove_attribute("data-loaded");
    body.set_inner_html("<div class=\"panel-loading\"><span></span><span></span><span></span></div>");
    toggle("#drawer", Some(true));
    refresh("drawer-body".into(), true);
}

// ---- ⌘K palette -----------------------------------------------------------------------------------
#[derive(Clone)]
struct Item {
    label: String,
    hint: String,
    href: String,
    keys: String,
}

thread_local! {
    static PAL: RefCell<(Vec<Item>, Vec<usize>, usize)> = RefCell::new((Vec::new(), Vec::new(), 0));
}

fn palette_items() -> Vec<Item> {
    let raw = by_sel("#palette-data").and_then(|e| e.text_content()).unwrap_or_default();
    serde_json::from_str::<Vec<Value>>(&raw)
        .unwrap_or_default()
        .iter()
        .map(|v| {
            let g = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            Item { label: g("label"), hint: g("hint"), href: g("href"), keys: g("keys") }
        })
        .collect()
}

/// Subsequence match score (lower is better); None when it doesn't match.
fn score(q: &str, item: &Item) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let label = item.label.to_lowercase();
    if label.starts_with(q) {
        return Some(0);
    }
    if label.contains(q) {
        return Some(1);
    }
    let hay = format!("{} {} {}", label, item.hint.to_lowercase(), item.keys.to_lowercase());
    if hay.split_whitespace().any(|w| w.starts_with(q)) {
        return Some(2);
    }
    // Loose subsequence only against the label itself (e.g. "dgn" -> Diagnostics).
    let mut it = label.chars();
    (q.len() >= 2 && q.chars().all(|c| it.any(|h| h == c))).then_some(3)
}

fn palette_render() {
    let q = by_sel("#palette input").and_then(|i| i.dyn_into::<HtmlInputElement>().ok()).map(|i| i.value().trim().to_lowercase()).unwrap_or_default();
    PAL.with(|p| {
        let mut p = p.borrow_mut();
        let mut hits: Vec<(i32, usize)> = p.0.iter().enumerate().filter_map(|(i, it)| score(&q, it).map(|s| (s, i))).collect();
        hits.sort();
        p.1 = hits.into_iter().map(|(_, i)| i).take(12).collect();
        if p.2 >= p.1.len() {
            p.2 = 0;
        }
        let html: String = p
            .1
            .iter()
            .enumerate()
            .map(|(n, &i)| {
                let it = &p.0[i];
                format!("<li role=\"option\" data-pal=\"{n}\"{}><span>{}</span><small>{}</small></li>", if n == p.2 { " class=\"sel\" aria-selected=\"true\"" } else { "" }, esc(&it.label), esc(&it.hint))
            })
            .collect();
        if let Some(list) = by_sel("#palette .palette-list") {
            list.set_inner_html(if html.is_empty() { "<li class=\"none\">No matches</li>" } else { &html });
        }
    });
}

fn palette_open() {
    PAL.with(|p| {
        let mut p = p.borrow_mut();
        if p.0.is_empty() {
            p.0 = palette_items();
        }
        p.2 = 0;
    });
    toggle("#palette", Some(true));
    if let Some(i) = by_sel("#palette input").and_then(|i| i.dyn_into::<HtmlInputElement>().ok()) {
        i.set_value("");
        let _ = i.focus();
    }
    palette_render();
}

fn palette_go(n: usize) {
    let href = PAL.with(|p| {
        let p = p.borrow();
        p.1.get(n).map(|&i| p.0[i].href.clone())
    });
    let Some(href) = href else { return };
    toggle("#palette", Some(false));
    if href == "/logout#post" {
        if let Some(f) = by_sel("form[action='/logout']").and_then(|f| f.dyn_into::<HtmlFormElement>().ok()) {
            let _ = f.submit();
        }
        return;
    }
    let _ = win().location().assign(&href);
}

fn palette_key(e: &web_sys::KeyboardEvent) -> bool {
    let open = by_sel("#palette.open").is_some();
    let typing = doc().active_element().is_some_and(|a| matches!(a.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT"));
    if !open {
        if (e.key() == "k" && (e.meta_key() || e.ctrl_key())) || (e.key() == "/" && !typing) {
            e.prevent_default();
            palette_open();
            return true;
        }
        return false;
    }
    match e.key().as_str() {
        "ArrowDown" | "ArrowUp" => {
            e.prevent_default();
            PAL.with(|p| {
                let mut p = p.borrow_mut();
                let len = p.1.len().max(1);
                p.2 = if e.key() == "ArrowDown" { (p.2 + 1) % len } else { (p.2 + len - 1) % len };
            });
            palette_render();
            true
        }
        "Enter" => {
            e.prevent_default();
            palette_go(PAL.with(|p| p.borrow().2));
            true
        }
        _ => false,
    }
}

fn close_menus(except: Option<&Element>) {
    for d in all_doc("details.navmenu[open], details.menu[open]") {
        if except.is_some_and(|e| d.contains(Some(e))) {
            continue;
        }
        let _ = d.remove_attribute("open");
    }
}

// ---- wiring ---------------------------------------------------------------------------------------------
fn wire() {
    let d: web_sys::EventTarget = doc().into();
    on(&d, "click", |e| {
        let t = e.target();
        close_menus(closest(&t, "details.navmenu, details.menu").as_ref());
        if let Some(el) = closest(&t, "[data-pal]") {
            palette_go(attr(&el, "data-pal").and_then(|v| v.parse().ok()).unwrap_or(0));
            return;
        }
        if closest(&t, "[data-palette-open]").is_some() {
            e.prevent_default();
            palette_open();
            return;
        }
        if let Some(p) = closest(&t, "#palette") {
            if closest(&t, ".palette-box").is_none() {
                toggle(&format!("#{}", p.id()), Some(false));
            }
            return;
        }
        if let Some(el) = closest(&t, "[data-drawer]") {
            e.prevent_default();
            open_drawer(&attr(&el, "data-drawer").unwrap_or_default());
            return;
        }
        if let Some(el) = closest(&t, "[data-post]") {
            e.prevent_default();
            if !el.has_attribute("disabled") {
                click_post(el);
            }
            return;
        }
        if let Some(el) = closest(&t, "[data-toggle]") {
            e.prevent_default();
            let sel = attr(&el, "data-toggle").unwrap_or_default();
            if let Some(target) = by_sel(&sel) {
                close_pops(Some(&target));
            }
            toggle(&sel, None);
            return;
        }
        if let Some(el) = closest(&t, "[data-close]") {
            toggle(&attr(&el, "data-close").unwrap_or_default(), Some(false));
            return;
        }
        if let Some(el) = closest(&t, "[data-refresh]") {
            e.prevent_default();
            let spec = attr(&el, "data-refresh").unwrap_or_default();
            refresh_spec(&spec);
            if spec == "all" {
                toast("Refreshed.", "success");
            }
            return;
        }
        if let Some(el) = closest(&t, "[data-fill]") {
            e.prevent_default();
            fill(&el);
            return;
        }
        if closest(&t, ".pop, .sidebar, .drawer, .palette").is_none() {
            close_pops(None);
        }
    });
    on(&d, "submit", |e| {
        let Some(form) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else { return };
        if form.has_attribute("data-api") {
            e.prevent_default();
            submit_api(form);
        } else if form.has_attribute("data-get") {
            e.prevent_default();
            run_get(&form);
        }
    });
    on(&d, "input", |e| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else { return };
        if el.closest("#palette").ok().flatten().is_some() {
            PAL.with(|p| p.borrow_mut().2 = 0);
            palette_render();
            return;
        }
        if let Some(form) = el.closest("form[data-get]").ok().flatten() {
            if el.dyn_ref::<HtmlInputElement>().is_some_and(|i| matches!(i.type_().as_str(), "search" | "text")) {
                debounce_get(form);
            }
        }
        if el.closest("form[data-pref-preview]").ok().flatten().is_some() {
            preview_field(&el);
        }
    });
    on(&d, "change", |e| {
        let Some(el) = e.target().and_then(|t| t.dyn_into::<Element>().ok()) else { return };
        if el.has_attribute("data-select-all") {
            let checked = el.dyn_ref::<HtmlInputElement>().is_some_and(HtmlInputElement::checked);
            if let Some(table) = el.closest("table").ok().flatten() {
                for b in all(&table, "[data-select-row]") {
                    if let Some(i) = b.dyn_ref::<HtmlInputElement>() {
                        i.set_checked(checked);
                    }
                }
                update_bulk(&table);
            }
            return;
        }
        if el.has_attribute("data-select-row") {
            if let Some(table) = el.closest("table").ok().flatten() {
                update_bulk(&table);
            }
            return;
        }
        if let Some(form) = el.closest("form[data-get]").ok().flatten() {
            if !el.dyn_ref::<HtmlInputElement>().is_some_and(|i| matches!(i.type_().as_str(), "search" | "text")) {
                run_get(&form);
            }
            return;
        }
        if let Some(form) = el.closest("form[data-autosubmit]").ok().flatten() {
            if el.tag_name() == "SELECT" {
                if let Some(f) = form.dyn_ref::<HtmlFormElement>() {
                    let _ = f.submit();
                }
            }
            return;
        }
        if el.closest("form[data-pref-preview]").ok().flatten().is_some() {
            preview_field(&el);
        }
    });
    on(&d, "keydown", |e| {
        let Some(k) = e.dyn_ref::<web_sys::KeyboardEvent>() else { return };
        if palette_key(k) {
            return;
        }
        if k.key() == "Escape" {
            close_pops(None);
            close_menus(None);
        }
    });
    on(&d, "pointerdown", |e| {
        if let Some(p) = e.dyn_ref::<PointerEvent>() {
            seek_start(p);
        }
    });
    on(&d, "pointermove", |e| {
        if let Some(p) = e.dyn_ref::<PointerEvent>() {
            seek_move(p);
        }
    });
    on(&d, "pointerup", |e| {
        if let Some(p) = e.dyn_ref::<PointerEvent>() {
            seek_end(p, true);
        }
    });
    on(&d, "pointercancel", |e| {
        if let Some(p) = e.dyn_ref::<PointerEvent>() {
            seek_end(p, false);
        }
    });
    on(&d, "visibilitychange", |_| {
        if hidden_page() {
            return;
        }
        if !live() {
            ST.with(|s| s.borrow_mut().attempt = 0);
            connect();
        } else if let Some(ws) = ST.with(|s| s.borrow().ws.clone()) {
            send(&ws, json!({ "type": "pong" }));
        }
        if let Some(r) = doc().document_element() {
            after_render(&r);
        }
    });
    on(&win().into(), "online", |_| {
        if !live() {
            ST.with(|s| s.borrow_mut().attempt = 0);
            connect();
        }
    });
}

#[wasm_bindgen(start)]
pub fn start() {
    let token = by_sel("meta[name=sp-token]").and_then(|m| attr(&m, "content")).filter(|t| !t.is_empty());
    ST.with(|s| s.borrow_mut().token = token);
    wire();
    if let Some(r) = doc().document_element() {
        after_render(&r);
    }
    interval(1000, || {
        if !hidden_page() {
            if let Some(r) = doc().document_element() {
                tick(&r);
            }
        }
    });
    interval(30_000, || {
        if !hidden_page() {
            if let Some(r) = doc().document_element() {
                localize(&r);
            }
        }
    });
    for el in all_doc("[data-poll]") {
        let secs = attr(&el, "data-poll").and_then(|s| s.parse::<i32>().ok()).unwrap_or(30).max(5);
        let id = el.id();
        interval(secs * 1000, move || {
            if !hidden_page() {
                refresh(id.clone(), false);
            }
        });
    }
    for el in all_doc("[data-scroll=bottom]") {
        el.set_scroll_top(el.scroll_height());
    }
    connect();
}
