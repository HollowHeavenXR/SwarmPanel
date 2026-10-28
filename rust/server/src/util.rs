//! Small shared helpers.

/// First `n` bytes, cut on a char boundary (Lua's s:sub(1, n)).
pub fn cut(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn iso_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}


/// HTML-escape (for the few strings built outside maud).
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
