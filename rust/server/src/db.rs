//! Postgres access (port of db.lua). One lazy pool per database name, with
//! PANEL_DB_ROUTES sending some databases to another host.
//!
//! `P` is a loosely-typed parameter (like the Lua's string-interpolated
//! values): it encodes itself as whatever type Postgres inferred for the
//! placeholder, so `guild_id = $1` works whether the value came in as a
//! number or a string. Rows decode generically to JSON the way pgmoon did
//! (timestamps as Postgres text, numeric as a string).

use crate::config::Settings;
use bytes::{BufMut, BytesMut};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use postgres_types::{FromSql, IsNull, ToSql, Type, to_sql_checked};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_postgres::Row;

type BoxErr = Box<dyn Error + Sync + Send>;

// ---- parameters -------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub enum P {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
    Json(Value),
}

impl From<i64> for P {
    fn from(v: i64) -> Self {
        P::Int(v)
    }
}
impl From<i32> for P {
    fn from(v: i32) -> Self {
        P::Int(v as i64)
    }
}
impl From<u64> for P {
    fn from(v: u64) -> Self {
        P::Int(v as i64)
    }
}
impl From<usize> for P {
    fn from(v: usize) -> Self {
        P::Int(v as i64)
    }
}
impl From<f64> for P {
    fn from(v: f64) -> Self {
        P::Float(v)
    }
}
impl From<bool> for P {
    fn from(v: bool) -> Self {
        P::Bool(v)
    }
}
impl From<&str> for P {
    fn from(v: &str) -> Self {
        P::Text(v.to_string())
    }
}
impl From<String> for P {
    fn from(v: String) -> Self {
        P::Text(v)
    }
}
impl From<&String> for P {
    fn from(v: &String) -> Self {
        P::Text(v.clone())
    }
}
impl<T: Into<P>> From<Option<T>> for P {
    fn from(v: Option<T>) -> Self {
        v.map(Into::into).unwrap_or(P::Null)
    }
}
impl From<&Value> for P {
    fn from(v: &Value) -> Self {
        match v {
            Value::Null => P::Null,
            Value::Bool(b) => P::Bool(*b),
            Value::Number(n) => n.as_i64().map(P::Int).unwrap_or_else(|| P::Float(n.as_f64().unwrap_or(0.0))),
            Value::String(s) => P::Text(s.clone()),
            other => P::Json(other.clone()),
        }
    }
}
impl From<Value> for P {
    fn from(v: Value) -> Self {
        P::from(&v)
    }
}

fn as_text(p: &P) -> String {
    match p {
        P::Null => String::new(),
        P::Int(i) => i.to_string(),
        P::Float(f) => f.to_string(),
        P::Text(s) => s.clone(),
        P::Bool(b) => b.to_string(),
        P::Json(v) => v.to_string(),
    }
}
fn as_i64(p: &P) -> Result<i64, BoxErr> {
    match p {
        P::Int(i) => Ok(*i),
        P::Float(f) => Ok(f.trunc() as i64),
        P::Bool(b) => Ok(*b as i64),
        P::Text(s) => s.trim().parse::<i64>().or_else(|_| s.trim().parse::<f64>().map(|f| f.trunc() as i64)).map_err(|_| format!("not an integer: {s:?}").into()),
        _ => Err("not an integer".into()),
    }
}
fn as_f64(p: &P) -> Result<f64, BoxErr> {
    match p {
        P::Int(i) => Ok(*i as f64),
        P::Float(f) => Ok(*f),
        P::Text(s) => s.trim().parse().map_err(|_| format!("not a number: {s:?}").into()),
        _ => Err("not a number".into()),
    }
}

fn encode_numeric(s: &str, out: &mut BytesMut) -> Result<(), BoxErr> {
    let s = s.trim();
    let (neg, s) = if let Some(r) = s.strip_prefix('-') { (true, r) } else { (false, s.strip_prefix('+').unwrap_or(s)) };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) || (int.is_empty() && frac.is_empty()) {
        return Err(format!("bad numeric {s:?}").into());
    }
    let int = int.trim_start_matches('0');
    let dscale = frac.len() as i16;
    let pad_int = format!("{}{int}", "0".repeat((4 - int.len() % 4) % 4));
    let pad_frac = format!("{frac}{}", "0".repeat((4 - frac.len() % 4) % 4));
    let mut digits: Vec<i16> = pad_int.as_bytes().chunks(4).chain(pad_frac.as_bytes().chunks(4)).map(|c| std::str::from_utf8(c).unwrap().parse().unwrap()).collect();
    let mut weight = (pad_int.len() / 4) as i16 - 1;
    while digits.first() == Some(&0) {
        digits.remove(0);
        weight -= 1;
    }
    while digits.last() == Some(&0) {
        digits.pop();
    }
    if digits.is_empty() {
        weight = 0;
    }
    out.put_i16(digits.len() as i16);
    out.put_i16(weight);
    out.put_u16(if neg && !digits.is_empty() { 0x4000 } else { 0 });
    out.put_i16(dscale);
    for d in digits {
        out.put_i16(d);
    }
    Ok(())
}

impl ToSql for P {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> Result<IsNull, BoxErr> {
        if matches!(self, P::Null) {
            return Ok(IsNull::Yes);
        }
        match *ty {
            Type::INT2 => out.put_i16(as_i64(self)? as i16),
            Type::INT4 => out.put_i32(as_i64(self)? as i32),
            Type::INT8 | Type::OID => out.put_i64(as_i64(self)?),
            Type::FLOAT4 => out.put_f32(as_f64(self)? as f32),
            Type::FLOAT8 => out.put_f64(as_f64(self)?),
            Type::NUMERIC => encode_numeric(&as_text(self), out)?,
            Type::BOOL => out.put_u8(match self {
                P::Bool(b) => *b as u8,
                P::Int(i) => (*i != 0) as u8,
                P::Text(s) => matches!(s.to_lowercase().as_str(), "t" | "true" | "1" | "yes" | "on") as u8,
                _ => 0,
            }),
            Type::JSON => out.put_slice(match self {
                P::Json(v) => v.to_string(),
                other => json!(as_text(other)).to_string(),
            }.as_bytes()),
            Type::JSONB => {
                out.put_u8(1);
                let s = match self {
                    P::Json(v) => v.to_string(),
                    P::Text(t) => serde_json::from_str::<Value>(t).map(|_| t.clone()).unwrap_or_else(|_| json!(t).to_string()),
                    other => as_text(other),
                };
                out.put_slice(s.as_bytes());
            }
            Type::BYTEA => out.put_slice(as_text(self).as_bytes()),
            _ => out.put_slice(as_text(self).as_bytes()),
        }
        Ok(IsNull::No)
    }
    fn accepts(_: &Type) -> bool {
        true
    }
    to_sql_checked!();
}

// ---- generic decoding --------------------------------------------------------------------
struct Raw<'a>(Option<&'a [u8]>);
impl<'a> FromSql<'a> for Raw<'a> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxErr> {
        Ok(Raw(Some(raw)))
    }
    fn from_sql_null(_: &Type) -> Result<Self, BoxErr> {
        Ok(Raw(None))
    }
    fn accepts(_: &Type) -> bool {
        true
    }
}

fn be_i64(b: &[u8]) -> i64 {
    i64::from_be_bytes(b[..8].try_into().unwrap_or([0; 8]))
}

/// Postgres-style text for a timestamp in microseconds since 2000-01-01.
fn fmt_ts(micros: i64, tz: bool) -> String {
    let epoch = chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
    let t = epoch + chrono::Duration::microseconds(micros);
    let base = t.format("%Y-%m-%d %H:%M:%S").to_string();
    let us = micros.rem_euclid(1_000_000);
    let frac = if us == 0 { String::new() } else { format!(".{:06}", us).trim_end_matches('0').to_string() };
    format!("{base}{frac}{}", if tz { "+00" } else { "" })
}

fn decode_numeric(b: &[u8]) -> String {
    if b.len() < 8 {
        return "0".into();
    }
    let rd = |i: usize| i16::from_be_bytes([b[i], b[i + 1]]);
    let (n, weight, sign, dscale) = (rd(0) as usize, rd(2) as i32, u16::from_be_bytes([b[4], b[5]]), rd(6) as usize);
    if sign == 0xC000 {
        return "NaN".into();
    }
    let digits: Vec<i32> = (0..n).map(|i| rd(8 + i * 2) as i32).collect();
    let mut int = String::new();
    for w in 0..=weight.max(-1) {
        let d = digits.get(w as usize).copied().unwrap_or(0);
        if int.is_empty() { int.push_str(&d.to_string()) } else { int.push_str(&format!("{d:04}")) }
    }
    if int.is_empty() {
        int.push('0');
    }
    let mut frac = String::new();
    let mut idx = weight + 1;
    while frac.len() < dscale {
        let d = if idx < 0 { 0 } else { digits.get(idx as usize).copied().unwrap_or(0) };
        frac.push_str(&format!("{d:04}"));
        idx += 1;
    }
    frac.truncate(dscale);
    let s = if dscale > 0 { format!("{int}.{frac}") } else { int };
    if sign == 0x4000 { format!("-{s}") } else { s }
}

fn decode_array(b: &[u8], elem: &Type) -> Value {
    if b.len() < 12 {
        return json!([]);
    }
    let ndim = i32::from_be_bytes(b[0..4].try_into().unwrap()) as usize;
    if ndim == 0 {
        return json!([]);
    }
    if ndim != 1 || b.len() < 20 {
        return Value::Null;
    }
    let len = i32::from_be_bytes(b[12..16].try_into().unwrap()) as usize;
    let mut pos = 20;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        if pos + 4 > b.len() {
            break;
        }
        let l = i32::from_be_bytes(b[pos..pos + 4].try_into().unwrap());
        pos += 4;
        if l < 0 {
            out.push(Value::Null);
        } else {
            let end = (pos + l as usize).min(b.len());
            out.push(decode(elem, Some(&b[pos..end])));
            pos = end;
        }
    }
    Value::Array(out)
}

fn decode(ty: &Type, raw: Option<&[u8]>) -> Value {
    let Some(b) = raw else { return Value::Null };
    match *ty {
        Type::BOOL => json!(b.first() == Some(&1)),
        Type::INT2 if b.len() >= 2 => json!(i16::from_be_bytes([b[0], b[1]])),
        Type::INT4 | Type::OID if b.len() >= 4 => json!(i32::from_be_bytes(b[..4].try_into().unwrap())),
        Type::INT8 if b.len() >= 8 => json!(be_i64(b)),
        Type::FLOAT4 if b.len() >= 4 => json!(f32::from_be_bytes(b[..4].try_into().unwrap()) as f64),
        Type::FLOAT8 if b.len() >= 8 => json!(f64::from_be_bytes(b[..8].try_into().unwrap())),
        Type::NUMERIC => json!(decode_numeric(b)),
        Type::JSON => serde_json::from_slice(b).unwrap_or(Value::Null),
        Type::JSONB => serde_json::from_slice(b.get(1..).unwrap_or(&[])).unwrap_or(Value::Null),
        Type::TIMESTAMP if b.len() >= 8 => json!(fmt_ts(be_i64(b), false)),
        Type::TIMESTAMPTZ if b.len() >= 8 => json!(fmt_ts(be_i64(b), true)),
        Type::DATE if b.len() >= 4 => {
            let days = i32::from_be_bytes(b[..4].try_into().unwrap());
            let d = chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap() + chrono::Duration::days(days as i64);
            json!(d.format("%Y-%m-%d").to_string())
        }
        Type::UUID if b.len() == 16 => {
            let h = hex::encode(b);
            json!(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
        }
        Type::BYTEA => json!(format!("\\x{}", hex::encode(b))),
        _ => match ty.kind() {
            postgres_types::Kind::Array(elem) => decode_array(b, elem),
            _ => json!(String::from_utf8_lossy(b)),
        },
    }
}

/// Every column of a row as JSON (pgmoon-style).
pub fn row_json(row: &Row) -> Value {
    let mut m = Map::new();
    for (i, c) in row.columns().iter().enumerate() {
        let v = row.try_get::<_, Raw>(i).map(|r| decode(c.type_(), r.0)).unwrap_or(Value::Null);
        m.insert(c.name().to_string(), v);
    }
    Value::Object(m)
}

// ---- pools ----------------------------------------------------------------------------------
pub struct Db {
    settings: Arc<Settings>,
    pools: Mutex<HashMap<String, Pool>>,
    exists_cache: Mutex<HashMap<String, (Instant, bool)>>,
}

pub type Rows = Vec<Value>;

fn pg_err(e: &tokio_postgres::Error) -> String {
    e.as_db_error().map(|d| d.message().to_string()).unwrap_or_else(|| e.to_string())
}

impl Db {
    pub fn new(settings: Arc<Settings>) -> Self {
        Self { settings, pools: Mutex::default(), exists_cache: Mutex::default() }
    }

    fn pool(&self, dbname: &str) -> Result<Pool, String> {
        let mut pools = self.pools.lock().unwrap();
        if let Some(p) = pools.get(dbname) {
            return Ok(p.clone());
        }
        let s = &self.settings;
        let route = s.db_routes.get(dbname);
        let mut pg = tokio_postgres::Config::new();
        pg.host(route.map(|r| r.host.as_str()).unwrap_or(&s.db_host))
            .port(route.map(|r| r.port).unwrap_or(s.db_port))
            .user(&s.db_user)
            .password(&s.db_password)
            .dbname(route.and_then(|r| r.database.as_deref()).unwrap_or(dbname))
            .application_name("swarmpanel-rs")
            .connect_timeout(Duration::from_secs(8))
            .keepalives(true);
        let size = if dbname == "accountlogins" { s.db_pool_size_main } else { s.db_pool_size };
        let mgr = Manager::from_config(pg, tokio_postgres::NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast });
        let pool = Pool::builder(mgr).max_size(size).wait_timeout(Some(Duration::from_secs(10))).runtime(deadpool_postgres::Runtime::Tokio1).build().map_err(|e| e.to_string())?;
        pools.insert(dbname.to_string(), pool.clone());
        Ok(pool)
    }

    /// Rows as JSON objects; Err(message) on failure.
    pub async fn query(&self, dbname: &str, sql: &str, params: &[P]) -> Result<Rows, String> {
        let client = self.pool(dbname)?.get().await.map_err(|e| format!("database unavailable: {e}"))?;
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
        let rows = client.query(sql, &refs).await.map_err(|e| {
            let m = pg_err(&e);
            println!("[swarmpanel] db error ({dbname}): {m} | {}", crate::util::cut(&sql.split_whitespace().collect::<Vec<_>>().join(" "), 300));
            m
        })?;
        Ok(rows.iter().map(row_json).collect())
    }
    pub async fn all(&self, dbname: &str, sql: &str, params: &[P]) -> Rows {
        self.query(dbname, sql, params).await.unwrap_or_default()
    }
    pub async fn one(&self, dbname: &str, sql: &str, params: &[P]) -> Result<Option<Value>, String> {
        Ok(self.query(dbname, sql, params).await?.into_iter().next())
    }
    pub async fn opt(&self, dbname: &str, sql: &str, params: &[P]) -> Option<Value> {
        self.one(dbname, sql, params).await.ok().flatten()
    }
    /// Rows affected.
    pub async fn exec(&self, dbname: &str, sql: &str, params: &[P]) -> Result<u64, String> {
        let client = self.pool(dbname)?.get().await.map_err(|e| format!("database unavailable: {e}"))?;
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
        client.execute(sql, &refs).await.map_err(|e| {
            let m = pg_err(&e);
            println!("[swarmpanel] db error ({dbname}): {m} | {}", crate::util::cut(&sql.split_whitespace().collect::<Vec<_>>().join(" "), 300));
            m
        })
    }
    /// Run several statements in one transaction (each with its own params).
    pub async fn transaction(&self, dbname: &str, stmts: &[(&str, Vec<P>)]) -> Result<(), String> {
        let mut client = self.pool(dbname)?.get().await.map_err(|e| format!("database unavailable: {e}"))?;
        let tx = client.transaction().await.map_err(|e| pg_err(&e))?;
        for (sql, params) in stmts {
            let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
            tx.execute(*sql, &refs).await.map_err(|e| pg_err(&e))?;
        }
        tx.commit().await.map_err(|e| pg_err(&e))
    }
    pub async fn batch(&self, dbname: &str, sql: &str) -> Result<(), String> {
        let client = self.pool(dbname)?.get().await.map_err(|e| format!("database unavailable: {e}"))?;
        client.batch_execute(sql).await.map_err(|e| pg_err(&e))
    }

    pub async fn table_exists(&self, dbname: &str, table: &str) -> bool {
        let key = format!("{dbname}\0{table}");
        if let Some((t, v)) = self.exists_cache.lock().unwrap().get(&key) {
            if t.elapsed() < Duration::from_secs(60) {
                return *v;
            }
        }
        let v = self
            .opt(dbname, "SELECT 1 AS present FROM information_schema.tables WHERE table_schema='public' AND table_name=$1 LIMIT 1", &[table.into()])
            .await
            .is_some();
        self.exists_cache.lock().unwrap().insert(key, (Instant::now(), v));
        v
    }

    pub async fn existing_tables(&self, dbname: &str, names: &[String]) -> std::collections::HashSet<String> {
        if names.is_empty() {
            return Default::default();
        }
        let arr: Vec<String> = names.to_vec();
        let client = match self.pool(dbname) {
            Ok(p) => match p.get().await {
                Ok(c) => c,
                Err(_) => return Default::default(),
            },
            Err(_) => return Default::default(),
        };
        client
            .query("SELECT table_name::text FROM information_schema.tables WHERE table_schema='public' AND table_name = ANY($1)", &[&arr])
            .await
            .map(|rows| rows.iter().filter_map(|r| r.try_get::<_, String>(0).ok()).collect())
            .unwrap_or_default()
    }
}

// ---- JSON value helpers (the Lua's tonumber/tostring on row fields) -----------------------------
pub trait J {
    fn s(&self, k: &str) -> Option<String>;
    fn i(&self, k: &str) -> Option<i64>;
    fn f(&self, k: &str) -> Option<f64>;
    fn b(&self, k: &str) -> bool;
}
impl J for Value {
    fn s(&self, k: &str) -> Option<String> {
        match self.get(k)? {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }
    fn i(&self, k: &str) -> Option<i64> {
        match self.get(k)? {
            Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
            Value::String(s) => s.trim().parse::<i64>().ok().or_else(|| s.trim().parse::<f64>().ok().map(|f| f as i64)),
            Value::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }
    fn f(&self, k: &str) -> Option<f64> {
        match self.get(k)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    fn b(&self, k: &str) -> bool {
        match self.get(k) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64() == Some(1),
            Some(Value::String(s)) => matches!(s.as_str(), "t" | "true" | "1"),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn discord_ids_stay_exact() {
        assert_eq!(super::as_i64(&super::P::Text("1247394560007471134".into())).unwrap(), 1247394560007471134);
    }

    #[test]
    fn numeric_roundtrip() {
        for s in ["0", "12.5", "-3.14159", "100000", "0.0001", "123456789.123"] {
            let mut b = bytes::BytesMut::new();
            super::encode_numeric(s, &mut b).unwrap();
            let out = super::decode_numeric(&b);
            assert_eq!(out.parse::<f64>().unwrap(), s.parse::<f64>().unwrap(), "{s} -> {out}");
        }
    }
    #[test]
    fn ts_format() {
        assert_eq!(super::fmt_ts(0, false), "2000-01-01 00:00:00");
        assert_eq!(super::fmt_ts(310_000, true), "2000-01-01 00:00:00.31+00");
    }
}
