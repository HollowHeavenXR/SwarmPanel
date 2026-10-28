//! CSV export (port of csv_export.lua): sensitive columns dropped, formula
//! injection neutralised, columns sorted.

use crate::http::Reply;
use serde_json::Value;

fn sensitive(k: &str) -> bool {
    let l = k.to_lowercase();
    ["password", "secret", "token", "api_key"].iter().any(|m| l.contains(m))
}

fn cell(v: Option<&Value>) -> String {
    let mut t = match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(o) => o.to_string(),
    };
    if t.starts_with(['=', '+', '-', '@']) {
        t = format!("'{t}");
    }
    format!("\"{}\"", t.replace('"', "\"\""))
}

pub fn to_csv(rows: &[Value]) -> String {
    let mut cols: Vec<String> = Vec::new();
    for r in rows {
        if let Some(m) = r.as_object() {
            for k in m.keys() {
                if !sensitive(k) && !cols.contains(k) {
                    cols.push(k.clone());
                }
            }
        }
    }
    cols.sort();
    let mut out = vec![cols.iter().map(|c| cell(Some(&Value::String(c.clone())))).collect::<Vec<_>>().join(",")];
    for r in rows {
        out.push(cols.iter().map(|c| cell(r.get(c))).collect::<Vec<_>>().join(","));
    }
    out.join("\r\n") + "\r\n"
}

pub fn safe_filename(n: &str) -> String {
    let mut s = String::new();
    for c in n.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            s.push(c);
        } else if !s.ends_with('_') {
            s.push('_');
        }
    }
    let s = s.trim_matches('_');
    crate::util::cut(if s.is_empty() { "export" } else { s }, 120)
}

pub fn response(rows: &[Value], name: &str) -> Reply {
    Reply::bytes(200, to_csv(rows).into_bytes(), "text/csv").header("Content-Disposition", format!("attachment; filename=\"{}.csv\"", safe_filename(name)))
}
