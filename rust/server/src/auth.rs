//! Password hashing + stateless session tokens (port of auth.lua).
//! Token = base64url(json) "." base64url(HMAC-SHA256(secret, base64url(json))),
//! byte-compatible with the Lua build so existing sessions keep working.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;
pub const PBKDF2_ITERATIONS: u32 = 260_000;

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn verification_code() -> String {
    format!("{:08}", rand::random_range(0..100_000_000u32))
}

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac key");
    m.update(msg);
    m.finalize().into_bytes().to_vec()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn password_hash(password: &str) -> String {
    let salt: [u8; 16] = rand::random();
    let mut dk = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ITERATIONS, &mut dk);
    format!("pbkdf2_sha256${PBKDF2_ITERATIONS}${}${}", STANDARD.encode(salt), STANDARD.encode(dk))
}

pub fn verify_password_hash(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2_sha256" {
        return false;
    }
    let (Ok(iter), Ok(salt), Ok(expected)) = (parts[1].parse::<u32>(), STANDARD.decode(parts[2]), STANDARD.decode(parts[3])) else { return false };
    if expected.is_empty() || iter == 0 {
        return false;
    }
    let mut dk = vec![0u8; expected.len()];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, iter, &mut dk);
    ct_eq(&dk, &expected)
}

/// CPU-heavy (260k rounds): run off the async threads.
pub async fn verify_password_async(password: String, stored: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password_hash(&password, &stored)).await.unwrap_or(false)
}
pub async fn password_hash_async(password: String) -> String {
    tokio::task::spawn_blocking(move || password_hash(&password)).await.unwrap_or_default()
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Auth {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string", skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub site_owner: bool,
    #[serde(default)]
    pub moderator: bool,
    #[serde(default)]
    pub admin_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub device: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default)]
    pub exp: i64,
}

fn de_opt_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s),
        other => Some(other.to_string()),
    })
}

impl Auth {
    pub fn role(&self) -> String {
        self.role.clone().unwrap_or_else(|| if self.guild_id.is_some() { "account".into() } else { "admin".into() })
    }
    pub fn is_admin(&self) -> bool {
        if !self.site_owner {
            return false;
        }
        self.admin_mode.unwrap_or_else(|| self.role().eq_ignore_ascii_case("admin") && self.guild_id.is_none())
    }
}

pub fn issue_token(secret: &str, mut payload: Value, ttl: i64) -> String {
    payload["exp"] = json!(crate::util::now() + ttl);
    let part = URL_SAFE_NO_PAD.encode(payload.to_string());
    let sig = URL_SAFE_NO_PAD.encode(hmac(secret.as_bytes(), part.as_bytes()));
    format!("{part}.{sig}")
}

pub fn verify_token(secret: &str, token: &str) -> Result<Value, &'static str> {
    if token.is_empty() {
        return Err("missing");
    }
    let Some((part, sig)) = token.split_once('.') else { return Err("malformed") };
    if part.is_empty() || sig.is_empty() || sig.contains('.') {
        return Err("malformed");
    }
    let expected = hmac(secret.as_bytes(), part.as_bytes());
    let given = URL_SAFE_NO_PAD.decode(sig.trim_end_matches('=')).map_err(|_| "bad_signature")?;
    if !ct_eq(&expected, &given) {
        return Err("bad_signature");
    }
    let raw = URL_SAFE_NO_PAD.decode(part.trim_end_matches('=')).map_err(|_| "malformed")?;
    let v: Value = serde_json::from_slice(&raw).map_err(|_| "malformed")?;
    if !v.is_object() {
        return Err("malformed");
    }
    if v.get("exp").and_then(Value::as_i64).is_some_and(|e| crate::util::now() > e) {
        return Err("expired");
    }
    Ok(v)
}

pub struct IssueOpts {
    pub role: String,
    pub guild_id: Option<String>,
    pub site_owner: bool,
    pub moderator: bool,
    pub admin_mode: Option<bool>,
    pub device: bool,
    pub client: Option<String>,
    pub ttl: i64,
}

pub fn issue_api_token(secret: &str, username: &str, o: IssueOpts) -> String {
    let admin_mode = o.admin_mode.unwrap_or(o.site_owner && o.role.eq_ignore_ascii_case("admin") && o.guild_id.is_none());
    let mut p = json!({ "username": username, "role": o.role, "site_owner": o.site_owner, "moderator": o.moderator, "admin_mode": admin_mode });
    if let Some(g) = o.guild_id.filter(|g| !g.is_empty()) {
        p["guild_id"] = json!(g);
    }
    if o.device {
        p["device"] = json!(true);
    }
    if let Some(c) = o.client.filter(|c| !c.is_empty()) {
        p["client"] = json!(crate::util::cut(&c, 20));
    }
    issue_token(secret, p, o.ttl)
}

pub fn verify_api_token(secret: &str, token: &str) -> Result<Auth, &'static str> {
    let v = verify_token(secret, token)?;
    serde_json::from_value(v).map_err(|_| "malformed")
}

pub fn bearer(header: Option<&str>) -> Option<String> {
    let h = header?.trim();
    let rest = h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))?;
    Some(rest.trim().to_string()).filter(|t| !t.is_empty())
}

pub fn verify_credentials(username: &str, password: &str, expected_user: &str, expected_pass: &str) -> bool {
    !expected_pass.is_empty() && ct_eq(username.as_bytes(), expected_user.as_bytes()) & ct_eq(password.as_bytes(), expected_pass.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_roundtrip() {
        let t = issue_api_token("s3cret", "bob", IssueOpts { role: "account".into(), guild_id: Some("123".into()), site_owner: false, moderator: false, admin_mode: None, device: false, client: Some("web".into()), ttl: 60 });
        let a = verify_api_token("s3cret", &t).unwrap();
        assert_eq!(a.username, "bob");
        assert_eq!(a.guild_id.as_deref(), Some("123"));
        assert!(!a.is_admin());
        assert!(verify_api_token("other", &t).is_err());
    }
    #[test]
    fn password() {
        let h = password_hash("hunter2");
        assert!(verify_password_hash("hunter2", &h));
        assert!(!verify_password_hash("nope", &h));
    }
}
